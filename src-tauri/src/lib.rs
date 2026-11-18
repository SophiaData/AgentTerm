mod projects;
mod sessions;
mod tools;
mod tray;

use tauri::{Manager, WindowEvent};

pub const PRODUCT_NAME: &str = "AgentTerm";

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(projects::ProjectStore::default())
        .manage(sessions::SessionManager::default())
        .invoke_handler(tauri::generate_handler![
            tools::detect_tools,
            projects::list_projects,
            projects::add_project,
            projects::remove_project,
            sessions::start_session,
            sessions::session_snapshot,
            sessions::write_session_input,
            sessions::resize_session,
            sessions::stop_session
        ])
        .setup(|app| {
            tray::install_tray(app)?;

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
