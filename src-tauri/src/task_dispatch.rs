use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::sessions::SessionInfo;
use crate::task_commands::{TaskLedgerState, WorkbenchView};
use crate::task_document::{AttemptState, Document, DocumentStore};
use crate::task_ledger::{Phase, Snapshot};
use crate::task_relay::{Cli, Plan as RelayPlan, Ticket};

pub(crate) struct ActiveDispatch {
    pub(crate) token: u128,
    pub(crate) task_id: String,
    pub(crate) generation: u64,
    pub(crate) control: crate::task_process::ProcessControl,
}

#[derive(Clone)]
pub(crate) struct DispatchRuntime {
    pub(crate) active: Arc<Mutex<Option<ActiveDispatch>>>,
}

impl Default for DispatchRuntime {
    fn default() -> Self {
        Self {
            active: Arc::new(Mutex::new(None)),
        }
    }
}

impl DispatchRuntime {
    fn lock_active(&self) -> std::sync::MutexGuard<'_, Option<ActiveDispatch>> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct DispatchLease {
    active: Arc<Mutex<Option<ActiveDispatch>>>,
    token: u128,
    task_id: String,
    generation: u64,
}

impl Drop for DispatchLease {
    fn drop(&mut self) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active.as_ref().is_some_and(|item| {
            item.token == self.token
                && item.task_id == self.task_id
                && item.generation == self.generation
        }) {
            *active = None;
        }
    }
}

impl TaskLedgerState {
    pub(crate) fn stop_controlled_dispatches(&self, timeout: Duration) -> Result<(), String> {
        let control = self
            .runtime
            .lock_active()
            .as_ref()
            .map(|dispatch| dispatch.control.clone());
        let Some(control) = control else {
            return Ok(());
        };
        control.request_stop();
        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            if self.runtime.lock_active().is_none() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Err("无法确认受控派发进程树已经停止，应用继续保持运行".into())
    }

    pub(crate) fn prepare_controlled_dispatch(
        &self,
        sessions: &crate::sessions::SessionManager,
        task_id: &str,
        generation: u64,
        revision: u64,
        session_id: &str,
    ) -> Result<(Ticket, RelayPlan, DocumentStore), String> {
        if self.runtime.lock_active().is_some() {
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

    pub(crate) fn start_controlled_dispatch(
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
            let mut active = self.runtime.lock_active();
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
        let task_id = ticket.task_id.clone();
        let generation = ticket.generation;
        let lease = DispatchLease {
            active: self.runtime.active.clone(),
            token,
            task_id: task_id.clone(),
            generation,
        };
        std::thread::Builder::new()
            .name("agentterm-controlled-relay".into())
            .spawn(move || {
                let _lease = lease;
                let _ = crate::task_relay::run_recorded_with_control(
                    &ledger, &ticket, &plan, &store, control,
                );
            })
            .map_err(|_| {
                let mut active = self.runtime.lock_active();
                if active.as_ref().is_some_and(|dispatch| {
                    dispatch.token == token
                        && dispatch.task_id == task_id
                        && dispatch.generation == generation
                }) {
                    *active = None;
                }
                "无法启动受控派发线程，任务继续占用".to_string()
            })?;
        Ok(())
    }

    pub(crate) fn request_controlled_stop(
        &self,
        task_id: &str,
        generation: u64,
        revision: u64,
    ) -> Result<WorkbenchView, String> {
        let control = {
            let active = self.runtime.lock_active();
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
            .find(|event| event.state == AttemptState::Running)
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

#[cfg(test)]
mod tests {
    use super::DispatchRuntime;

    #[test]
    fn dispatch_lock_recovers_after_poison() {
        let runtime = DispatchRuntime::default();
        let active = runtime.active.clone();
        let _ = std::thread::spawn(move || {
            let _guard = active.lock().expect("lock active dispatch");
            panic!("poison active dispatch lock");
        })
        .join();

        assert!(runtime.active.is_poisoned());
        assert!(runtime.lock_active().is_none());
    }
}

fn controlled_cli(tool_id: &str) -> Result<Cli, String> {
    match tool_id {
        "claude" => Ok(Cli::Claude),
        "cursor" | "cursor-agent" => Ok(Cli::Cursor),
        _ => Err("当前 CLI 暂不支持受控派发；请先使用 Claude Code 会话".into()),
    }
}

pub(crate) fn validate_controlled_dispatch(
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
