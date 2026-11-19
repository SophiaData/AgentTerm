use crate::projects::ProjectStore;
use crate::task_document::{ArtifactPreview, Document, DocumentStore, Step, TaskPlan};
use crate::task_ledger::{Ledger, Phase, Snapshot};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TaskLedgerState {
    path: Mutex<Option<PathBuf>>,
    owner_id: String,
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
        Ok(WorkbenchView {
            ledger: self.view(snapshot),
            document,
        })
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
    if active.phase != Phase::Reserved || active.project_id != input.project_id {
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
}
