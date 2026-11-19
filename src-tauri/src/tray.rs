use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

const SHOW_WINDOW: &str = "show-window";
const HIDE_WINDOW: &str = "hide-window";
const QUIT_APP: &str = "quit-app";

#[derive(Debug, Eq, PartialEq)]
pub enum TrayCommand {
    Show,
    Hide,
    Quit,
}

pub fn parse_tray_command(id: &str) -> Option<TrayCommand> {
    match id {
        SHOW_WINDOW => Some(TrayCommand::Show),
        HIDE_WINDOW => Some(TrayCommand::Hide),
        QUIT_APP => Some(TrayCommand::Quit),
        _ => None,
    }
}

pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "AgentTerm main window is unavailable".to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

pub fn hide_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "AgentTerm main window is unavailable".to_string())?;
    window.hide().map_err(|error| error.to_string())
}

/// Report a blocked normal exit with a native, non-blocking dialog even when
/// the main GUI was hidden in the tray. Never silently report a safe shutdown.
pub fn report_shutdown_error<R: Runtime>(app: &AppHandle<R>, error: &str) {
    eprintln!("AgentTerm safe shutdown blocked: {error}");
    if let Err(show_error) = show_main_window(app) {
        eprintln!("Unable to show AgentTerm after failed shutdown: {show_error}");
    }
    app.dialog()
        .message(format!(
            "终端进程可能仍在运行，已取消退出。请重试退出或手动关闭会话。\n{error}"
        ))
        .title("AgentTerm 无法安全退出")
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

pub fn install_tray(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, SHOW_WINDOW, "显示 AgentTerm", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, HIDE_WINDOW, "隐藏窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT_APP, "退出 AgentTerm", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &quit])?;
    let icon = app
        .default_window_icon()
        .expect("AgentTerm default window icon should exist")
        .clone();

    TrayIconBuilder::new()
        .icon(icon)
        .tooltip("AgentTerm")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match parse_tray_command(event.id.as_ref()) {
            Some(TrayCommand::Show) => {
                if let Err(error) = show_main_window(app) {
                    eprintln!("Unable to show AgentTerm: {error}");
                }
            }
            Some(TrayCommand::Hide) => {
                if let Err(error) = hide_main_window(app) {
                    eprintln!("Unable to hide AgentTerm: {error}");
                }
            }
            Some(TrayCommand::Quit) => {
                // Preflight also protects the fallback branch of AppHandle::exit
                // when the runtime cannot enqueue ExitRequested.
                match app.state::<crate::sessions::SessionManager>().stop_all() {
                    Ok(()) => app.exit(0),
                    Err(error) => report_shutdown_error(app, &error),
                }
            }
            None => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) && let Err(error) = show_main_window(tray.app_handle())
            {
                eprintln!("Unable to open AgentTerm from tray: {error}");
            }
        })
        .build(app)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{TrayCommand, parse_tray_command};

    #[test]
    fn tray_menu_ids_map_to_the_expected_commands() {
        assert_eq!(parse_tray_command("show-window"), Some(TrayCommand::Show));
        assert_eq!(parse_tray_command("hide-window"), Some(TrayCommand::Hide));
        assert_eq!(parse_tray_command("quit-app"), Some(TrayCommand::Quit));
    }

    #[test]
    fn unknown_tray_menu_ids_are_ignored() {
        assert_eq!(parse_tray_command("unknown"), None);
        assert_eq!(parse_tray_command(""), None);
    }
}
