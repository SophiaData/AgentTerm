mod project_files;
mod projects;
mod schedule_mcp;
mod scheduled_tasks;
mod session_history;
mod sessions;
mod task_commands;
pub mod task_document;
pub mod task_ledger;
mod task_process;
pub mod task_relay;
mod tools;
mod tray;
mod updates;

use tauri::{Manager, WindowEvent};

pub const PRODUCT_NAME: &str = "AgentTerm";

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(projects::ProjectStore::default())
        .manage(sessions::SessionManager::default())
        .manage(task_commands::TaskLedgerState::default())
        .manage(scheduled_tasks::Scheduler::default())
        .manage(schedule_mcp::ScheduleMcp::default())
        .invoke_handler(tauri::generate_handler![
            tools::detect_tools,
            projects::list_projects,
            projects::add_project,
            projects::remove_project,
            project_files::list_project_files,
            project_files::preview_project_file,
            project_files::reveal_project_file,
            sessions::start_session,
            sessions::session_snapshot,
            sessions::write_session_input,
            sessions::resize_session,
            sessions::stop_session,
            sessions::forget_session,
            sessions::rename_session,
            sessions::session_history,
            task_commands::task_ledger_initialize,
            task_commands::task_ledger_snapshot,
            task_commands::claim_main_task,
            task_commands::cancel_main_task_reservation,
            task_commands::recover_main_task_reservation,
            task_commands::resume_main_task_after_recovery,
            task_commands::workbench_snapshot,
            task_commands::create_workbench_task,
            task_commands::attach_task_plan,
            task_commands::reference_task_artifact,
            task_commands::associate_task_session,
            task_commands::reassociate_task_session_after_recovery,
            task_commands::dispatch_controlled_task,
            task_commands::request_stop_controlled_task,
            task_commands::preview_task_artifact,
            scheduled_tasks::list_scheduled_tasks,
            scheduled_tasks::save_scheduled_task,
            scheduled_tasks::update_scheduled_task,
            scheduled_tasks::set_scheduled_task_enabled,
            scheduled_tasks::remove_scheduled_task,
            scheduled_tasks::run_scheduled_task_now,
            updates::check_for_update,
            updates::install_update
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
            app.state::<sessions::SessionManager>()
                .configure_history(directory.join("agentterm-session-history.json"))
                .map_err(std::io::Error::other)?;
            app.state::<scheduled_tasks::Scheduler>()
                .configure(directory.join("agentterm-scheduled-tasks.json"))
                .map_err(std::io::Error::other)?;
            let scheduler = app.state::<scheduled_tasks::Scheduler>().handle();
            app.state::<schedule_mcp::ScheduleMcp>()
                .configure(scheduler.handle(), app.handle().clone())
                .map_err(std::io::Error::other)?;
            std::thread::spawn(move || {
                loop {
                    if let Err(error) = scheduler.tick() {
                        eprintln!("AgentTerm scheduled task tick failed: {error}");
                    }
                    std::thread::sleep(std::time::Duration::from_secs(30));
                }
            });

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
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                match app.state::<sessions::SessionManager>().stop_all() {
                    Ok(()) => app.state::<schedule_mcp::ScheduleMcp>().cleanup(),
                    Err(error) => {
                        api.prevent_exit();
                        tray::report_shutdown_error(app, &error);
                    }
                }
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
