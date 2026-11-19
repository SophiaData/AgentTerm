use crate::projects::ProjectStore;
use crate::task_ledger::{Ledger, Phase, Snapshot};
use serde::Serialize;
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
}
