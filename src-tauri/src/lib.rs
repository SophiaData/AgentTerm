mod projects;
mod sessions;
mod task_commands;
pub mod task_document;
pub mod task_ledger;
mod task_process;
pub mod task_relay;
mod tools;
mod tray;

use tauri::{Manager, WindowEvent};

pub const PRODUCT_NAME: &str = "AgentTerm";

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(projects::ProjectStore::default())
        .manage(sessions::SessionManager::default())
        .manage(task_commands::TaskLedgerState::default())
        .invoke_handler(tauri::generate_handler![
            tools::detect_tools,
            projects::list_projects,
            projects::add_project,
            projects::remove_project,
            sessions::start_session,
            sessions::session_snapshot,
            sessions::write_session_input,
            sessions::resize_session,
            sessions::stop_session,
            sessions::forget_session,
            task_commands::task_ledger_initialize,
            task_commands::task_ledger_snapshot,
            task_commands::claim_main_task,
            task_commands::cancel_main_task_reservation,
            task_commands::recover_main_task_reservation,
            task_commands::workbench_snapshot,
            task_commands::create_workbench_task,
            task_commands::attach_task_plan,
            task_commands::reference_task_artifact,
            task_commands::preview_task_artifact
        ])
        .setup(|app| {
            tray::install_tray(app)?;

            // Fixed existing OS data directory: projects and windows cannot
            // select separate ledgers. No directory or ledger is auto-created.
            let directory = app.path().local_data_dir()?;
            app.state::<projects::ProjectStore>()
                .configure(directory.join("agentterm-projects.json"))
                .map_err(std::io::Error::other)?;
            app.state::<task_commands::TaskLedgerState>()
                .configure(directory.join("agentterm-main-task-ledger.log"))
                .map_err(std::io::Error::other)?;

            let window = app
                .get_webview_window("main")
                .expect("AgentTerm main window should exist");
            let close_window = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if let Err(error) = close_window.hide() {
                        eprintln!("Unable to hide AgentTerm after close: {error}");
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("AgentTerm should build")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                app.state::<sessions::SessionManager>().stop_all();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::PRODUCT_NAME;

    #[test]
    fn product_name_is_stable() {
        assert_eq!(PRODUCT_NAME, "AgentTerm");
    }
}
