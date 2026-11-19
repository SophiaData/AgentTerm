#[allow(dead_code)] // Native-terminal launcher remains for its historical regression tests.
mod launcher;
mod projects;
// Managed PTY sessions are exposed through validated Tauri commands.
mod sessions;
mod tools;
mod tray;

use tauri::{Emitter, Manager, WindowEvent};

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
            sessions::stop_session,
            sessions::forget_session
        ])
        .setup(|app| {
            // A single Rust-level reaper must be ready before any managed PTY
            // can start. If its thread fails to boot, startup fails closed.
            let event_app = app.handle().clone();
            app.state::<sessions::SessionManager>()
                .start_reaper(move |exit| {
                    let _ = event_app.emit(&format!("session-exit-{}", exit.id), exit);
                })
                .map_err(std::io::Error::other)?;
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
            if let tauri::RunEvent::ExitRequested { api, .. } = event
                && let Err(error) = app.state::<sessions::SessionManager>().stop_all()
            {
                // Tauri 2.12 dispatches this before completing a normal exit.
                // Synchronous prevention is required; logging alone is unsafe.
                api.prevent_exit();
                tray::report_shutdown_error(app, &error);
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
