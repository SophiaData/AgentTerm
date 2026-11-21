use crate::projects::ProjectStore;
use crate::sessions::{SessionInfo, SessionManager};
use crate::task_document::{ArtifactPreview, Document, DocumentStore, SessionLink, Step, TaskPlan};
use crate::task_ledger::{Ledger, Phase, Snapshot};
use crate::task_relay::{Cli, Plan as RelayPlan, Ticket};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct ActiveDispatch {
    token: u128,
    task_id: String,
    generation: u64,
    control: crate::task_process::ProcessControl,
}

#[derive(Clone)]
struct DispatchRuntime {
    active: Arc<Mutex<Option<ActiveDispatch>>>,
}

impl Default for DispatchRuntime {
    fn default() -> Self {
        Self {
            active: Arc::new(Mutex::new(None)),
        }
    }
}

pub struct TaskLedgerState {
    path: Mutex<Option<PathBuf>>,
    owner_id: String,
    runtime: DispatchRuntime,
}

impl Default for TaskLedgerState {
    fn default() -> Self {
        Self {
            path: Mutex::new(None),
            // Identifies a backend lifetime, not an authorization credential.
            owner_id: format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            runtime: DispatchRuntime::default(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTaskView {
    pub task_id: String,
    pub project_id: String,
    pub phase: String,
    pub recovery_required: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerView {
    pub revision: u64,
    pub generation: u64,
    pub active: Option<ActiveTaskView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchView {
    pub ledger: TaskLedgerView,
    pub document: Option<Document>,
    pub recovery: Option<RecoveryView>,
    pub dispatch_active: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryView {
    pub attempts: Vec<RecoveryAttemptView>,
    pub all_exited: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryAttemptView {
    pub attempt_id: String,
    pub step_id: String,
    pub observation: String,
    pub safe_to_resume: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanInput {
    pub project_id: String,
    pub goal: String,
    pub owner: String,
    pub scope: String,
    /// Existing directory relative to the registered project, "." allowed.
    pub output_dir: String,
}

impl TaskLedgerState {
    /// Backend-only; the frontend cannot choose another ledger or owner ID.
    pub fn configure(&self, path: PathBuf) -> Result<(), String> {
        let mut configured = self
            .path
            .lock()
            .map_err(|_| "任务账本配置不可用".to_string())?;
        if configured.is_some() {
            return Err("任务账本路径已经绑定，不能切换".to_string());
        }
        Ledger::at(&path).map_err(|error| error.to_string())?;
        *configured = Some(path);
        Ok(())
    }

    fn ledger(&self) -> Result<Ledger, String> {
        let path = self
            .path
            .lock()
            .map_err(|_| "任务账本配置不可用".to_string())?
            .clone()
            .ok_or_else(|| "账本位置不可用，请检查本机数据目录".to_string())?;
        Ledger::at(&path).map_err(|error| error.to_string())
    }

    fn view(&self, snapshot: Snapshot) -> TaskLedgerView {
        TaskLedgerView {
            revision: snapshot.revision,
            generation: snapshot.generation,
            active: snapshot.active.map(|task| {
                let recovery_required =
                    task.owner_id != self.owner_id || task.phase == Phase::RecoveryRequired;
                ActiveTaskView {
                    task_id: task.task_id,
                    project_id: task.project_id,
                    phase: task.phase.name().to_string(),
                    recovery_required,
                }
            }),
        }
    }

    fn claim(
        &self,
        projects: &ProjectStore,
        task_id: &str,
        project_id: &str,
    ) -> Result<TaskLedgerView, String> {
        if !projects
            .list()?
            .iter()
            .any(|project| project.id == project_id)
        {
            return Err("项目未登记，请先选择已有项目".to_string());
        }
        self.ledger()?
            .claim(task_id, project_id, &self.owner_id)
            .map(|snapshot| self.view(snapshot))
            .map_err(|error| error.to_string())
    }

    fn document_store(&self, task_id: &str, generation: u64) -> Result<DocumentStore, String> {
        let configured = self
            .path
            .lock()
            .map_err(|_| "任务文档位置不可用".to_string())?;
        let directory = configured
            .as_ref()
            .and_then(|path| path.parent())
            .ok_or_else(|| "任务文档位置不可用".to_string())?;
        DocumentStore::at(directory, task_id, generation)
    }

    fn workbench(&self) -> Result<WorkbenchView, String> {
        let ledger = self.ledger()?;
        let snapshot = ledger.snapshot().map_err(|error| error.to_string())?;
        let document = if let Some(task) = &snapshot.active {
            let store = self.document_store(&task.task_id, snapshot.generation)?;
            if store.exists() {
                let document = store.snapshot()?;
                if document.plan.task_id != task.task_id
                    || document.plan.generation != snapshot.generation
                    || document.plan.project_id != task.project_id
                {
                    return Err("任务计划与当前账本不匹配，需核实".into());
                }
                Some(document)
            } else {
                None
            }
        } else {
            None
        };
        // Never combine an old document with a new task from another window.
        if ledger.snapshot().map_err(|error| error.to_string())? != snapshot {
            return Err("任务状态已变化，请刷新".into());
        }
        let recovery = match (&snapshot.active, &document) {
            (Some(active), Some(document))
                if active.phase == Phase::RecoveryRequired
                    || (active.owner_id != self.owner_id && !document.events.is_empty()) =>
            {
                let attempts = crate::task_relay::assess_recovery(document)
                    .into_iter()
                    .map(|attempt| RecoveryAttemptView {
                        attempt_id: attempt.attempt_id,
                        step_id: attempt.step_id,
                        observation: recovery_observation_name(attempt.observation).into(),
                        safe_to_resume: attempt.observation
                            == crate::task_relay::RecoveryObservation::Exited,
                    })
                    .collect::<Vec<_>>();
                let all_exited =
                    !attempts.is_empty() && attempts.iter().all(|attempt| attempt.safe_to_resume);
                Some(RecoveryView {
                    attempts,
                    all_exited,
                })
            }
            _ => None,
        };
        let dispatch_active = self
            .runtime
            .active
            .lock()
            .map_err(|_| "受控派发状态不可用".to_string())?
            .as_ref()
            .is_some_and(|dispatch| {
                snapshot.active.as_ref().is_some_and(|task| {
                    dispatch.task_id == task.task_id && dispatch.generation == snapshot.generation
                })
            });
        Ok(WorkbenchView {
            ledger: self.view(snapshot),
            document,
            recovery,
            dispatch_active,
        })
    }

    fn resume_after_recovery(
        &self,
        task_id: &str,
        generation: u64,
        revision: u64,
    ) -> Result<WorkbenchView, String> {
        let (snapshot, store) = self.current_store(task_id, generation, revision, false)?;
        let active = snapshot
            .active
            .as_ref()
            .ok_or_else(|| "没有当前主任务".to_string())?;
        if active.phase != Phase::RecoveryRequired && active.owner_id == self.owner_id {
            return Err("当前任务不在待核实状态，请先刷新".into());
        }
        if self
            .runtime
            .active
            .lock()
            .map_err(|_| "受控派发状态不可用".to_string())?
            .is_some()
        {
            return Err("受控执行仍在收尾，请稍后重新核对".into());
        }
        let document = store.snapshot()?;
        if document.plan.task_id != task_id
            || document.plan.generation != generation
            || document.plan.project_id != active.project_id
        {
            return Err("任务计划与当前账本不匹配，不能接回".into());
        }
        let assessment = crate::task_relay::assess_recovery(&document);
        if assessment.is_empty()
            || assessment.iter().any(|attempt| {
                attempt.observation != crate::task_relay::RecoveryObservation::Exited
            })
        {
            return Err("旧执行尝试尚未全部确认退出，任务继续待核实".into());
        }
        self.ledger()?
            .resume_after_recovery(&self.owner_id, task_id, generation, revision)
            .map_err(|error| error.to_string())?;
        self.workbench()
    }

    fn plan_input(
        &self,
        projects: &ProjectStore,
        input: PlanInput,
        task_id: String,
        generation: u64,
    ) -> Result<TaskPlan, String> {
        let project = projects
            .list()?
            .into_iter()
            .find(|item| item.id == input.project_id)
            .ok_or_else(|| "项目未登记，请先选择已有项目".to_string())?;
        let relative = std::path::Path::new(&input.output_dir);
        if relative.is_absolute()
            || relative.components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            return Err("产物目录须是项目内已有的相对目录".into());
        }
        let root = PathBuf::from(&project.path)
            .canonicalize()
            .map_err(|_| "项目目录不可用".to_string())?;
        if root.to_string_lossy() != project.id {
            return Err("项目真实路径已变化，请重新登记".into());
        }
        let output = root
            .join(relative)
            .canonicalize()
            .map_err(|_| "产物目录必须已经存在".to_string())?;
        if !output.is_dir() || !output.starts_with(&root) {
            return Err("产物目录不能超出项目".into());
        }
        let plan = TaskPlan {
            task_id,
            generation,
            project_id: project.id,
            goal: input.goal,
            owner: input.owner,
            scope: input.scope,
            output_dir: output.to_string_lossy().into_owned(),
            steps: vec![
                Step {
                    id: "worker".into(),
                    title: "执行与提交结果".into(),
                    assignee: "执行 Agent".into(),
                },
                Step {
                    id: "reviewer".into(),
                    title: "独立验收".into(),
                    assignee: "验收 Agent".into(),
                },
            ],
        };
        crate::task_document::validate_new_plan(&plan)?;
        Ok(plan)
    }

    fn create_task(
        &self,
        projects: &ProjectStore,
        input: PlanInput,
    ) -> Result<WorkbenchView, String> {
        let task_id = format!(
            "task-{}-{}",
            self.owner_id,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        // Validate all user input and paths before claiming occupancy.
        let mut plan = self.plan_input(projects, input, task_id, 1)?;
        let ledger = self.ledger()?;
        let snapshot = ledger
            .claim(&plan.task_id, &plan.project_id, &self.owner_id)
            .map_err(|error| error.to_string())?;
        plan.generation = snapshot.generation;
        // Failure retains the reservation; attach_task_plan repairs only a
        // missing document, never silently releases or overwrites an existing one.
        self.document_store(&plan.task_id, plan.generation)?
            .create(plan)?;
        self.workbench()
    }

    fn current_store(
        &self,
        task_id: &str,
        generation: u64,
        revision: u64,
        require_owner: bool,
    ) -> Result<(Snapshot, DocumentStore), String> {
        let snapshot = self
            .ledger()?
            .snapshot()
            .map_err(|error| error.to_string())?;
        let active = snapshot
            .active
            .as_ref()
            .ok_or_else(|| "没有当前主任务".to_string())?;
        if active.task_id != task_id
            || snapshot.generation != generation
            || snapshot.revision != revision
        {
            return Err("任务代次或版本已变化，请刷新".into());
        }
        if require_owner
            && (active.owner_id != self.owner_id || active.phase == Phase::RecoveryRequired)
        {
            return Err("任务归属或执行状态待核实，当前仅可查看".into());
        }
        let store = self.document_store(task_id, generation)?;
        Ok((snapshot, store))
    }

    fn recover(
        &self,
        projects: &ProjectStore,
        task_id: &str,
        generation: u64,
        revision: u64,
        release: bool,
    ) -> Result<WorkbenchView, String> {
        let (snapshot, store) = self.current_store(task_id, generation, revision, false)?;
        let active = snapshot
            .active
            .as_ref()
            .ok_or_else(|| "没有当前主任务".to_string())?;
        if !active.phase.is_reserved() {
            return Err("已有派发意图，不能使用未派发预留恢复；占用继续保留".into());
        }
        if store.exists() {
            let document = store.snapshot()?;
            if document.plan.task_id != task_id
                || document.plan.generation != generation
                || document.plan.project_id != active.project_id
                || !document.events.is_empty()
            {
                return Err("计划不匹配或已有执行记录；占用继续保留，需核实".into());
            }
        }
        if !release {
            let project = projects
                .list()?
                .into_iter()
                .find(|p| p.id == active.project_id)
                .ok_or_else(|| "请先重新登记该任务的已有项目，再接管预留".to_string())?;
            if crate::projects::normalize_project_dir(&project.path)?.to_string_lossy()
                != project.id
            {
                return Err("项目真实路径已变化，不能接管".into());
            }
        }
        // OS absence proof and ticket CAS run together under the ledger locks.
        // No process starts here; document/plan versions are left unchanged.
        self.ledger()?
            .recover_reservation(&self.owner_id, task_id, generation, revision, release)
            .map_err(|e| e.to_string())?;
        self.workbench()
    }

    fn prepare_controlled_dispatch(
        &self,
        sessions: &SessionManager,
        task_id: &str,
        generation: u64,
        revision: u64,
        session_id: &str,
    ) -> Result<(Ticket, RelayPlan, DocumentStore), String> {
        if self
            .runtime
            .active
            .lock()
            .map_err(|_| "受控派发状态不可用".to_string())?
            .is_some()
        {
            return Err("已有受控派发正在运行，请先核对当前状态".into());
        }
        let (snapshot, store) = self.current_store(task_id, generation, revision, true)?;
        let document = store.snapshot()?;
        let info = sessions
            .session_info(session_id)
            .map_err(|_| "关联的 CLI 会话不存在或已经结束，不能受控派发".to_string())?;
        let cli = validate_controlled_dispatch(
            &snapshot, &document, &info, session_id, task_id, generation, revision,
        )?;
        let project_dir = PathBuf::from(&info.project_path)
            .canonicalize()
            .map_err(|_| "项目目录不可用，不能受控派发".to_string())?;
        let ticket = Ticket {
            task_id: task_id.into(),
            owner_id: self.owner_id.clone(),
            generation,
            revision,
        };
        let plan = RelayPlan {
            project_dir,
            goal: document.plan.goal,
            worker: cli,
            reviewer: cli,
            timeout: Duration::from_secs(300),
        };
        Ok((ticket, plan, store))
    }

    fn start_controlled_dispatch(
        &self,
        ticket: Ticket,
        plan: RelayPlan,
        store: DocumentStore,
    ) -> Result<(), String> {
        let ledger = self.ledger()?;
        let control = crate::task_process::ProcessControl::new();
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        {
            let mut active = self
                .runtime
                .active
                .lock()
                .map_err(|_| "受控派发状态不可用".to_string())?;
            if active.is_some() {
                return Err("已有受控派发正在运行，请先核对当前状态".into());
            }
            *active = Some(ActiveDispatch {
                token,
                task_id: ticket.task_id.clone(),
                generation: ticket.generation,
                control: control.clone(),
            });
        }
        let runtime = self.runtime.clone();
        let task_id = ticket.task_id.clone();
        let generation = ticket.generation;
        std::thread::Builder::new()
            .name("agentterm-controlled-relay".into())
            .spawn(move || {
                let _ = crate::task_relay::run_recorded_with_control(
                    &ledger, &ticket, &plan, &store, control,
                );
                if let Ok(mut active) = runtime.active.lock()
                    && active.as_ref().is_some_and(|item| {
                        item.token == token
                            && item.task_id == task_id
                            && item.generation == generation
                    })
                {
                    *active = None;
                }
            })
            .map_err(|_| {
                if let Ok(mut active) = self.runtime.active.lock()
                    && active
                        .as_ref()
                        .is_some_and(|dispatch| dispatch.token == token)
                {
                    *active = None;
                }
                "无法启动受控派发线程，任务继续占用".to_string()
            })?;
        Ok(())
    }

    fn request_controlled_stop(
        &self,
        task_id: &str,
        generation: u64,
        revision: u64,
    ) -> Result<WorkbenchView, String> {
        let control = {
            let active = self
                .runtime
                .active
                .lock()
                .map_err(|_| "受控派发状态不可用".to_string())?;
            let dispatch = active
                .as_ref()
                .ok_or_else(|| "当前没有可停止的受控派发".to_string())?;
            if dispatch.task_id != task_id || dispatch.generation != generation {
                return Err("当前受控派发已变化，请刷新后重试".into());
            }
            dispatch.control.clone()
        };
        let (_, store) = self.current_store(task_id, generation, revision, true)?;
        let active = self
            .ledger()?
            .snapshot()
            .map_err(|error| error.to_string())?
            .active
            .ok_or_else(|| "没有当前主任务".to_string())?;
        if !matches!(
            active.phase,
            Phase::Running | Phase::Verifying | Phase::Pausing
        ) {
            return Err("当前任务不在可停止的受控执行阶段".into());
        }
        let document = store.snapshot()?;
        let running = document
            .events
            .iter()
            .rev()
            .find(|event| event.state == crate::task_document::AttemptState::Running)
            .ok_or_else(|| "尚未找到可停止的执行尝试，请刷新核对".to_string())?;
        store.record_stop_request(&running.id, "用户请求停止受控派发")?;
        control.request_stop();
        if active.phase != Phase::Pausing {
            let latest = self
                .ledger()?
                .snapshot()
                .map_err(|error| error.to_string())?;
            let latest_active = latest
                .active
                .as_ref()
                .ok_or_else(|| "任务在停止请求期间已变化，请重新核对".to_string())?;
            if latest.generation != generation
                || latest_active.task_id != task_id
                || latest_active.owner_id != self.owner_id
                || !matches!(
                    latest_active.phase,
                    Phase::Running | Phase::Verifying | Phase::Pausing
                )
            {
                return self.workbench();
            }
            if latest_active.phase != Phase::Pausing {
                self.ledger()?
                    .set_phase(
                        &self.owner_id,
                        task_id,
                        generation,
                        latest.revision,
                        Phase::Pausing,
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
        self.workbench()
    }
}

fn controlled_cli(tool_id: &str) -> Result<Cli, String> {
    match tool_id {
        "claude" => Ok(Cli::Claude),
        "cursor" | "cursor-agent" => Ok(Cli::Cursor),
        _ => Err("当前 CLI 暂不支持受控派发；请先使用 Claude Code 会话".into()),
    }
}

fn validate_controlled_dispatch(
    snapshot: &Snapshot,
    document: &Document,
    info: &SessionInfo,
    session_id: &str,
    task_id: &str,
    generation: u64,
    revision: u64,
) -> Result<Cli, String> {
    let active = snapshot
        .active
        .as_ref()
        .ok_or_else(|| "没有当前主任务，不能受控派发".to_string())?;
    if snapshot.revision != revision
        || snapshot.generation != generation
        || active.task_id != task_id
        || !active.phase.is_reserved()
    {
        return Err("任务代次或版本已变化，不能重复派发，请刷新".into());
    }
    if document.plan.task_id != task_id
        || document.plan.generation != generation
        || document.plan.project_id != active.project_id
    {
        return Err("任务计划与当前账本不匹配，不能受控派发".into());
    }
    if active.project_id != info.project_path {
        return Err("只能在任务所属项目的 CLI 会话中受控派发".into());
    }
    let link = document
        .session_link
        .as_ref()
        .ok_or_else(|| "请先把当前运行中的 CLI 会话关联到任务记录".to_string())?;
    if link.id != session_id
        || link.id != info.id
        || link.tool_id != info.tool_id
        || link.tool_name != info.tool_name
        || link.project_path != info.project_path
    {
        return Err("当前 CLI 会话不是任务记录已关联的会话".into());
    }
    controlled_cli(&info.tool_id)
}

fn recovery_observation_name(observation: crate::task_relay::RecoveryObservation) -> &'static str {
    match observation {
        crate::task_relay::RecoveryObservation::LaunchUncertain => "launchUncertain",
        crate::task_relay::RecoveryObservation::Exited => "exited",
        crate::task_relay::RecoveryObservation::Alive => "alive",
        crate::task_relay::RecoveryObservation::Reused => "reused",
        crate::task_relay::RecoveryObservation::Unknown => "unknown",
    }
}

#[tauri::command]
pub fn recover_main_task_reservation(
    state: tauri::State<'_, TaskLedgerState>,
    projects: tauri::State<'_, ProjectStore>,
    task_id: String,
    generation: u64,
    expected_revision: u64,
    release: bool,
) -> Result<WorkbenchView, String> {
    state.recover(&projects, &task_id, generation, expected_revision, release)
}

#[tauri::command]
pub fn resume_main_task_after_recovery(
    state: tauri::State<'_, TaskLedgerState>,
    task_id: String,
    generation: u64,
    expected_revision: u64,
) -> Result<WorkbenchView, String> {
    state.resume_after_recovery(&task_id, generation, expected_revision)
}

#[tauri::command]
pub fn workbench_snapshot(
    state: tauri::State<'_, TaskLedgerState>,
) -> Result<WorkbenchView, String> {
    state.workbench()
}

#[tauri::command]
pub fn create_workbench_task(
    state: tauri::State<'_, TaskLedgerState>,
    projects: tauri::State<'_, ProjectStore>,
    input: PlanInput,
) -> Result<WorkbenchView, String> {
    state.create_task(&projects, input)
}

#[tauri::command]
pub fn attach_task_plan(
    state: tauri::State<'_, TaskLedgerState>,
    projects: tauri::State<'_, ProjectStore>,
    task_id: String,
    generation: u64,
    expected_revision: u64,
    input: PlanInput,
) -> Result<WorkbenchView, String> {
    let (snapshot, store) = state.current_store(&task_id, generation, expected_revision, true)?;
    let active = snapshot
        .active
        .as_ref()
        .ok_or_else(|| "没有当前主任务".to_string())?;
    if !active.phase.is_reserved() || active.project_id != input.project_id {
        return Err("仅可为当前未派发任务补全计划".into());
    }
    let plan = state.plan_input(&projects, input, task_id, generation)?;
    store.create(plan)?;
    state.workbench()
}

#[tauri::command]
pub fn reference_task_artifact(
    state: tauri::State<'_, TaskLedgerState>,
    task_id: String,
    generation: u64,
    expected_revision: u64,
    relative_path: String,
    label: String,
) -> Result<WorkbenchView, String> {
    let (_, store) = state.current_store(&task_id, generation, expected_revision, true)?;
    store.reference(&relative_path, &label)?;
    state.workbench()
}

#[tauri::command]
pub fn associate_task_session(
    state: tauri::State<'_, TaskLedgerState>,
    sessions: tauri::State<'_, SessionManager>,
    task_id: String,
    generation: u64,
    expected_revision: u64,
    session_id: String,
) -> Result<WorkbenchView, String> {
    let info = sessions
        .session_info(&session_id)
        .map_err(|_| "CLI 会话不存在或已经结束，不能关联".to_string())?;
    let session = SessionLink {
        id: info.id,
        tool_id: info.tool_id,
        tool_name: info.tool_name,
        project_path: info.project_path,
    };
    let (snapshot, store) = state.current_store(&task_id, generation, expected_revision, true)?;
    let active = snapshot
        .active
        .as_ref()
        .ok_or_else(|| "没有当前主任务".to_string())?;
    if active.project_id != session.project_path {
        return Err("只能关联同一项目下的 CLI 会话".into());
    }
    let document = store.snapshot()?;
    if document.plan.project_id != active.project_id
        || document.plan.task_id != task_id
        || document.plan.generation != generation
    {
        return Err("任务计划与当前账本不匹配，不能关联会话".into());
    }
    store.associate_session(session)?;
    state.workbench()
}

#[tauri::command]
pub fn reassociate_task_session_after_recovery(
    state: tauri::State<'_, TaskLedgerState>,
    sessions: tauri::State<'_, SessionManager>,
    task_id: String,
    generation: u64,
    expected_revision: u64,
    previous_session_id: String,
    session_id: String,
) -> Result<WorkbenchView, String> {
    let info = sessions
        .session_info(&session_id)
        .map_err(|_| "当前 CLI 会话不存在或已结束".to_string())?;
    let (_, store) = state.current_store(&task_id, generation, expected_revision, true)?;
    state
        .ledger()?
        .while_recovered(
            &state.owner_id,
            &task_id,
            generation,
            expected_revision,
            || -> Result<(), String> {
                if previous_session_id != session_id
                    && sessions.session_info(&previous_session_id).is_ok()
                {
                    return Err("原关联会话仍在运行，请继续使用原会话或先结束它".into());
                }
                store.reassociate_session(
                    &previous_session_id,
                    SessionLink {
                        id: info.id,
                        tool_id: info.tool_id,
                        tool_name: info.tool_name,
                        project_path: info.project_path,
                    },
                )?;
                Ok(())
            },
        )
        .map_err(|error| error.to_string())??;
    state.workbench()
}

#[tauri::command]
pub fn dispatch_controlled_task(
    state: tauri::State<'_, TaskLedgerState>,
    sessions: tauri::State<'_, SessionManager>,
    task_id: String,
    generation: u64,
    expected_revision: u64,
    session_id: String,
) -> Result<WorkbenchView, String> {
    let (ticket, plan, store) = state.prepare_controlled_dispatch(
        &sessions,
        &task_id,
        generation,
        expected_revision,
        &session_id,
    )?;
    state.start_controlled_dispatch(ticket, plan, store)?;
    state.workbench()
}

#[tauri::command]
pub fn request_stop_controlled_task(
    state: tauri::State<'_, TaskLedgerState>,
    task_id: String,
    generation: u64,
    expected_revision: u64,
) -> Result<WorkbenchView, String> {
    state.request_controlled_stop(&task_id, generation, expected_revision)
}

#[tauri::command]
pub fn preview_task_artifact(
    state: tauri::State<'_, TaskLedgerState>,
    task_id: String,
    generation: u64,
    expected_revision: u64,
    artifact_id: String,
) -> Result<ArtifactPreview, String> {
    let (_, store) = state.current_store(&task_id, generation, expected_revision, false)?;
    store.preview(&artifact_id)
}

#[tauri::command]
pub fn task_ledger_initialize(
    state: tauri::State<'_, TaskLedgerState>,
) -> Result<TaskLedgerView, String> {
    state
        .ledger()?
        .initialize()
        .map(|snapshot| state.view(snapshot))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn task_ledger_snapshot(
    state: tauri::State<'_, TaskLedgerState>,
) -> Result<TaskLedgerView, String> {
    state
        .ledger()?
        .snapshot()
        .map(|snapshot| state.view(snapshot))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn claim_main_task(
    state: tauri::State<'_, TaskLedgerState>,
    projects: tauri::State<'_, ProjectStore>,
    task_id: String,
    project_id: String,
) -> Result<TaskLedgerView, String> {
    state.claim(&projects, &task_id, &project_id)
}

#[tauri::command]
pub fn cancel_main_task_reservation(
    state: tauri::State<'_, TaskLedgerState>,
    task_id: String,
    generation: u64,
    expected_revision: u64,
) -> Result<TaskLedgerView, String> {
    state
        .ledger()?
        .cancel_reservation(&state.owner_id, &task_id, generation, expected_revision)
        .map(|snapshot| state.view(snapshot))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_ledger::ActiveTask;

    #[test]
    fn restarted_backend_marks_existing_owner_as_needing_recovery() {
        let state = TaskLedgerState::default();
        let result = state.view(Snapshot {
            revision: 1,
            generation: 1,
            active: Some(ActiveTask {
                task_id: "task-1".to_string(),
                project_id: "project-1".to_string(),
                owner_id: "previous-process".to_string(),
                phase: Phase::Reserved,
            }),
        });
        assert!(result.active.unwrap().recovery_required);
    }

    #[test]
    fn backend_cannot_be_rebound_by_another_window() {
        let state = TaskLedgerState::default();
        let path =
            std::env::temp_dir().join(format!("agentterm-config-test-{}.log", state.owner_id));
        state.configure(path.clone()).unwrap();
        assert!(state.configure(path).is_err());
    }

    #[test]
    fn missing_directory_is_not_created() {
        let state = TaskLedgerState::default();
        let root = std::env::temp_dir().join(format!("agentterm-missing-{}", state.owner_id));
        assert!(state.configure(root.join("ledger.log")).is_err());
        assert!(!root.exists());
    }

    #[test]
    fn unregistered_project_cannot_claim_a_task() {
        let state = TaskLedgerState::default();
        assert_eq!(
            state
                .claim(&ProjectStore::default(), "task", "unknown")
                .unwrap_err(),
            "项目未登记，请先选择已有项目"
        );
    }
    fn configured() -> (TaskLedgerState, ProjectStore, String) {
        let state = TaskLedgerState::default();
        let root = std::env::temp_dir().canonicalize().unwrap();
        state
            .configure(root.join(format!("workbench-command-{}.log", state.owner_id)))
            .unwrap();
        state.ledger().unwrap().initialize().unwrap();
        let projects = ProjectStore::default();
        let project = projects.add(&root.to_string_lossy()).unwrap();
        (state, projects, project.id)
    }
    fn input(project: String) -> PlanInput {
        PlanInput {
            project_id: project,
            goal: "检查草稿".into(),
            owner: "owner".into(),
            scope: "local".into(),
            output_dir: ".".into(),
        }
    }
    #[test]
    fn create_task_saves_plan_and_refuses_another_task() {
        let (state, projects, project) = configured();
        let view = state
            .create_task(&projects, input(project.clone()))
            .unwrap();
        assert_eq!(view.document.as_ref().unwrap().plan.goal, "检查草稿");
        assert_eq!(view.ledger.active.as_ref().unwrap().phase, "reserved");
        assert!(state.create_task(&projects, input(project)).is_err());
        assert_eq!(
            state.workbench().unwrap().document.unwrap(),
            view.document.unwrap()
        );
    }
    #[test]
    fn invalid_plan_or_output_directory_does_not_claim_occupancy() {
        let (state, projects, project) = configured();
        let mut request = input(project.clone());
        request.goal = "".into();
        assert!(state.create_task(&projects, request).is_err());
        let mut request = input(project);
        request.output_dir = "../outside".into();
        assert!(state.create_task(&projects, request).is_err());
        assert!(state.ledger().unwrap().snapshot().unwrap().active.is_none());
    }
    #[test]
    fn restart_can_read_metadata_but_cannot_write_with_old_owner() {
        let (state, projects, project) = configured();
        let view = state.create_task(&projects, input(project)).unwrap();
        let restarted = TaskLedgerState::default();
        restarted
            .configure(state.path.lock().unwrap().clone().unwrap())
            .unwrap();
        let recovered = restarted.workbench().unwrap();
        assert!(recovered.ledger.active.as_ref().unwrap().recovery_required);
        let active = view.ledger.active.unwrap();
        assert!(
            restarted
                .current_store(
                    &active.task_id,
                    view.ledger.generation,
                    view.ledger.revision,
                    true
                )
                .is_err()
        );
        assert!(
            restarted
                .current_store(
                    &active.task_id,
                    view.ledger.generation,
                    view.ledger.revision,
                    false
                )
                .is_ok()
        );
        assert!(
            state
                .current_store(
                    &active.task_id,
                    view.ledger.generation,
                    view.ledger.revision + 1,
                    true
                )
                .is_err()
        );
    }

    fn exited_owner() -> String {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(child.wait().unwrap().success());
        format!("{pid}-1")
    }

    #[test]
    fn restart_recovery_preserves_plan_and_rejects_old_tickets() {
        let (mut old, projects, project) = configured();
        old.owner_id = exited_owner();
        let view = old.create_task(&projects, input(project)).unwrap();
        let task = view.ledger.active.unwrap().task_id;
        let new = TaskLedgerState::default();
        new.configure(old.path.lock().unwrap().clone().unwrap())
            .unwrap();
        let recovered = new.recover(&projects, &task, 1, 1, false).unwrap();
        assert_eq!(recovered.document, view.document);
        assert_eq!(recovered.ledger.revision, 2);
        let active = recovered.ledger.active.unwrap();
        assert!(!active.recovery_required);
        assert_eq!(active.phase, "recoveredReserved");
        assert!(old.current_store(&task, 1, 1, true).is_err());
        assert!(old.current_store(&task, 1, 2, true).is_err());
        assert!(new.recover(&projects, &task, 1, 1, true).is_err());
        new.ledger()
            .unwrap()
            .cancel_reservation(&new.owner_id, &task, 1, 2)
            .unwrap();
        assert!(new.workbench().unwrap().ledger.active.is_none());
    }

    #[test]
    fn live_owner_unknown_identity_and_dispatched_tasks_stay_occupied() {
        for owner in [format!("{}-1", std::process::id()), "legacy-unknown".into()] {
            let (mut old, projects, project) = configured();
            old.owner_id = owner;
            let view = old.create_task(&projects, input(project)).unwrap();
            let task = view.ledger.active.unwrap().task_id;
            let new = TaskLedgerState::default();
            new.configure(old.path.lock().unwrap().clone().unwrap())
                .unwrap();
            assert!(new.recover(&projects, &task, 1, 1, false).is_err());
            assert!(new.recover(&projects, &task, 1, 1, true).is_err());
            assert_eq!(old.ledger().unwrap().snapshot().unwrap().revision, 1);
            old.ledger()
                .unwrap()
                .set_phase(&old.owner_id, &task, 1, 1, Phase::Running)
                .unwrap();
            assert!(new.recover(&projects, &task, 1, 2, true).is_err());
        }
    }

    #[test]
    fn recovery_needs_registered_project_but_release_does_not_remove_documents() {
        let (mut old, projects, project) = configured();
        old.owner_id = exited_owner();
        let view = old.create_task(&projects, input(project)).unwrap();
        let task = view.ledger.active.unwrap().task_id;
        let new = TaskLedgerState::default();
        new.configure(old.path.lock().unwrap().clone().unwrap())
            .unwrap();
        let empty = ProjectStore::default();
        assert!(
            new.recover(&empty, &task, 1, 1, false)
                .unwrap_err()
                .contains("重新登记")
        );
        let released = new.recover(&empty, &task, 1, 1, true).unwrap();
        assert!(released.ledger.active.is_none());
        assert_eq!(
            old.document_store(&task, 1).unwrap().snapshot().unwrap(),
            view.document.unwrap()
        );
    }

    #[test]
    fn a_document_execution_event_blocks_reservation_recovery() {
        let (mut old, projects, project) = configured();
        old.owner_id = exited_owner();
        let view = old.create_task(&projects, input(project)).unwrap();
        let task = view.ledger.active.unwrap().task_id;
        old.document_store(&task, 1)
            .unwrap()
            .record(crate::task_document::AttemptEvent {
                id: "1:2:0:start".into(),
                sequence: 4,
                attempt_id: "1:2:0".into(),
                step_id: "worker".into(),
                state: crate::task_document::AttemptState::Running,
                summary: "intent".into(),
                process_identity: None,
                stop: None,
            })
            .unwrap();
        let new = TaskLedgerState::default();
        new.configure(old.path.lock().unwrap().clone().unwrap())
            .unwrap();
        assert!(
            new.recover(&projects, &task, 1, 1, true)
                .unwrap_err()
                .contains("执行记录")
        );
        assert_eq!(old.ledger().unwrap().snapshot().unwrap().revision, 1);
    }

    #[test]
    fn dispatched_recovery_exposes_process_evidence_without_releasing_occupancy() {
        let (state, projects, project) = configured();
        let view = state.create_task(&projects, input(project)).unwrap();
        let task = view.ledger.active.as_ref().unwrap().task_id.clone();
        state
            .ledger()
            .unwrap()
            .set_phase(&state.owner_id, &task, 1, 1, Phase::RecoveryRequired)
            .unwrap();
        state
            .document_store(&task, 1)
            .unwrap()
            .record(crate::task_document::AttemptEvent {
                id: "1:2:0:start".into(),
                sequence: 4,
                attempt_id: "1:2:0".into(),
                step_id: "worker".into(),
                state: crate::task_document::AttemptState::Running,
                summary: "intent".into(),
                process_identity: None,
                stop: None,
            })
            .unwrap();

        let current = state.workbench().unwrap();
        let recovery = current.recovery.unwrap();
        assert_eq!(recovery.attempts[0].observation, "launchUncertain");
        assert!(!recovery.all_exited);
        assert_eq!(current.ledger.active.unwrap().phase, "recoveryRequired");
        assert!(state.resume_after_recovery(&task, 1, 2).is_err());
    }

    #[test]
    fn restart_during_running_exposes_recovery_without_rewriting_the_ledger() {
        let (mut old, projects, project) = configured();
        old.owner_id = exited_owner();
        let view = old.create_task(&projects, input(project)).unwrap();
        let task = view.ledger.active.unwrap().task_id;
        old.ledger()
            .unwrap()
            .set_phase(&old.owner_id, &task, 1, 1, Phase::Running)
            .unwrap();
        old.document_store(&task, 1)
            .unwrap()
            .record(crate::task_document::AttemptEvent {
                id: "1:2:0:start".into(),
                sequence: 4,
                attempt_id: "1:2:0".into(),
                step_id: "worker".into(),
                state: crate::task_document::AttemptState::Running,
                summary: "intent".into(),
                process_identity: None,
                stop: None,
            })
            .unwrap();
        let restarted = TaskLedgerState::default();
        restarted
            .configure(old.path.lock().unwrap().clone().unwrap())
            .unwrap();
        let current = restarted.workbench().unwrap();
        assert!(current.ledger.active.unwrap().recovery_required);
        let recovery = current
            .recovery
            .expect("crashed running tasks need a recovery assessment");
        assert_eq!(recovery.attempts[0].observation, "launchUncertain");
        assert!(!recovery.all_exited);
        assert!(restarted.resume_after_recovery(&task, 1, 2).is_err());
        assert_eq!(old.ledger().unwrap().snapshot().unwrap().revision, 2);
    }

    #[test]
    fn dispatched_recovery_can_be_reclaimed_once_all_old_attempts_exited() {
        let (mut old, projects, project) = configured();
        old.owner_id = exited_owner();
        let view = old.create_task(&projects, input(project)).unwrap();
        let task = view.ledger.active.as_ref().unwrap().task_id.clone();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        old.ledger()
            .unwrap()
            .set_phase(&old.owner_id, &task, 1, 1, Phase::RecoveryRequired)
            .unwrap();
        old.document_store(&task, 1)
            .unwrap()
            .record(crate::task_document::AttemptEvent {
                id: "1:2:0:start".into(),
                sequence: 4,
                attempt_id: "1:2:0".into(),
                step_id: "worker".into(),
                state: crate::task_document::AttemptState::Running,
                summary: "intent".into(),
                process_identity: Some(crate::task_document::ProcessIdentity {
                    pid,
                    executable: std::env::current_exe().unwrap().to_string_lossy().into(),
                    start_token: None,
                }),
                stop: None,
            })
            .unwrap();
        let recovered = TaskLedgerState::default();
        recovered
            .configure(old.path.lock().unwrap().clone().unwrap())
            .unwrap();
        let current = recovered.workbench().unwrap();
        assert!(current.recovery.unwrap().all_exited);
        let resumed = recovered.resume_after_recovery(&task, 1, 2).unwrap();
        assert_eq!(resumed.ledger.revision, 3);
        assert_eq!(resumed.ledger.active.unwrap().phase, "recoveredReserved");
        assert!(recovered.resume_after_recovery(&task, 1, 2).is_err());
    }

    #[test]
    fn crashed_coordinator_can_be_reclaimed_from_running_verifying_or_pausing() {
        for phase in [
            Phase::Running,
            Phase::Verifying,
            Phase::Pausing,
            Phase::WaitingInput,
        ] {
            let (mut old, projects, project) = configured();
            old.owner_id = exited_owner();
            let view = old.create_task(&projects, input(project)).unwrap();
            let task = view.ledger.active.unwrap().task_id;
            old.ledger()
                .unwrap()
                .set_phase(&old.owner_id, &task, 1, 1, phase)
                .unwrap();
            let pid = old.owner_id.split('-').next().unwrap().parse().unwrap();
            let store = old.document_store(&task, 1).unwrap();
            store
                .record(crate::task_document::AttemptEvent {
                    id: "1:2:0:start".into(),
                    sequence: 4,
                    attempt_id: "1:2:0".into(),
                    step_id: "worker".into(),
                    state: crate::task_document::AttemptState::Running,
                    summary: "intent".into(),
                    process_identity: Some(crate::task_document::ProcessIdentity {
                        pid,
                        executable: "fixture".into(),
                        start_token: None,
                    }),
                    stop: None,
                })
                .unwrap();
            let before = store.snapshot().unwrap();
            let restarted = TaskLedgerState::default();
            restarted
                .configure(old.path.lock().unwrap().clone().unwrap())
                .unwrap();
            assert!(restarted.workbench().unwrap().recovery.unwrap().all_exited);
            let resumed = restarted.resume_after_recovery(&task, 1, 2).unwrap();
            assert_eq!(resumed.ledger.active.unwrap().phase, "recoveredReserved");
            assert_eq!(resumed.ledger.revision, 3);
            assert_eq!(store.snapshot().unwrap(), before);
            assert!(
                old.ledger()
                    .unwrap()
                    .set_phase(&old.owner_id, &task, 1, 2, Phase::Running)
                    .is_err()
            );
            assert!(restarted.resume_after_recovery(&task, 1, 2).is_err());
        }
    }

    fn controlled_dispatch_fixture(link: Option<SessionLink>) -> (Snapshot, Document, SessionInfo) {
        let project = std::env::temp_dir().canonicalize().unwrap();
        let project_path = project.to_string_lossy().into_owned();
        (
            Snapshot {
                revision: 2,
                generation: 1,
                active: Some(ActiveTask {
                    task_id: "task-controlled".into(),
                    project_id: project_path.clone(),
                    owner_id: "owner".into(),
                    phase: Phase::Reserved,
                }),
            },
            Document {
                revision: 1,
                plan: TaskPlan {
                    task_id: "task-controlled".into(),
                    generation: 1,
                    project_id: project_path.clone(),
                    goal: "核对本地材料".into(),
                    owner: "owner".into(),
                    scope: "只读".into(),
                    output_dir: project_path.clone(),
                    steps: vec![],
                },
                events: vec![],
                artifacts: vec![],
                session_link: link,
                session_link_history: Vec::new(),
            },
            SessionInfo {
                id: "session-controlled".into(),
                tool_id: "claude".into(),
                tool_name: "Claude Code".into(),
                project_path: project_path,
            },
        )
    }

    #[test]
    fn controlled_dispatch_requires_a_matching_live_session_link() {
        let (snapshot, document, info) = controlled_dispatch_fixture(None);
        let error = validate_controlled_dispatch(
            &snapshot,
            &document,
            &info,
            &info.id,
            "task-controlled",
            1,
            2,
        )
        .unwrap_err();
        assert!(error.contains("关联"));

        let link = SessionLink {
            id: info.id.clone(),
            tool_id: info.tool_id.clone(),
            tool_name: info.tool_name.clone(),
            project_path: info.project_path.clone(),
        };
        let (snapshot, document, info) = controlled_dispatch_fixture(Some(link));
        assert_eq!(
            validate_controlled_dispatch(
                &snapshot,
                &document,
                &info,
                &info.id,
                "task-controlled",
                1,
                2,
            )
            .unwrap(),
            Cli::Claude
        );
    }

    #[test]
    fn controlled_dispatch_rejects_stale_revision_cross_project_and_unsupported_tool() {
        let link = SessionLink {
            id: "session-controlled".into(),
            tool_id: "claude".into(),
            tool_name: "Claude Code".into(),
            project_path: std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into(),
        };
        let (mut snapshot, document, info) = controlled_dispatch_fixture(Some(link));
        assert!(
            validate_controlled_dispatch(
                &snapshot,
                &document,
                &info,
                &info.id,
                "task-controlled",
                1,
                1,
            )
            .unwrap_err()
            .contains("版本")
        );

        snapshot.revision = 2;
        let mut other = info.clone();
        other.project_path = "C:\\other-project".into();
        assert!(
            validate_controlled_dispatch(
                &snapshot,
                &document,
                &other,
                &other.id,
                "task-controlled",
                1,
                2,
            )
            .unwrap_err()
            .contains("所属项目")
        );

        let mut unsupported = info;
        unsupported.tool_id = "codex".into();
        unsupported.tool_name = "Codex".into();
        let mut unsupported_document = document;
        unsupported_document.session_link.as_mut().unwrap().tool_id = "codex".into();
        unsupported_document
            .session_link
            .as_mut()
            .unwrap()
            .tool_name = "Codex".into();
        assert!(
            validate_controlled_dispatch(
                &snapshot,
                &unsupported_document,
                &unsupported,
                &unsupported.id,
                "task-controlled",
                1,
                2,
            )
            .unwrap_err()
            .contains("暂不支持")
        );
    }
}
