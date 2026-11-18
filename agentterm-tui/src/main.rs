//! AgentTerm TUI — a tiny terminal manager for AI CLIs.
//!
//! Flow: pick a project -> pick an installed tool -> we exec it in that
//! directory. When the tool exits, we come back to the list. No WebView, no PTY
//! proxy, no Node runtime: one small static binary.

mod app;
mod launch;
mod store;
mod tools;
mod ui;

use std::io::stdout;

use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("agentterm {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let mut state = app::App::new();

    if let Err(message) = run(&mut state) {
        let _ = restore_terminal();
        eprintln!("agentterm: {message}");
        std::process::exit(1);
    }
}

fn run(state: &mut app::App) -> Result<(), String> {
    enable_raw_mode().map_err(|e| format!("无法进入终端模式：{e}"))?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen).map_err(|e| format!("无法切换屏幕：{e}"))?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend).map_err(|e| format!("无法创建界面：{e}"))?;

    loop {
        terminal
            .draw(|frame| ui::render(frame, state))
            .map_err(|e| format!("无法绘制界面：{e}"))?;

        if let Some(request) = state.pending_launch.take() {
            // Hand the terminal to the real tool, then re-enter our UI.
            restore_terminal()?;
            let error = launch::exec(&request.program, &request.args, &request.dir);
            // If exec actually returned, it failed; surface the reason.
            enable_raw_mode().map_err(|e| format!("无法恢复终端模式：{e}"))?;
            terminal
                .backend_mut()
                .execute(EnterAlternateScreen)
                .map_err(|e| format!("无法恢复界面：{e}"))?;
            terminal.clear().map_err(|e| format!("无法清屏：{e}"))?;
            state.set_error(format!("启动 {} 失败：{error}", request.program));
            continue;
        }

        if !event::poll(std::time::Duration::from_millis(200))
            .map_err(|e| format!("无法读取按键：{e}"))?
        {
            continue;
        }

        let Event::Key(key) = event::read().map_err(|e| format!("无法读取按键：{e}"))?
        else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            break;
        }

        if state.handle_key(key.code) {
            break;
        }
    }

    restore_terminal()?;
    Ok(())
}

fn restore_terminal() -> Result<(), String> {
    disable_raw_mode().map_err(|e| format!("无法退出终端模式：{e}"))?;
    execute!(stdout(), LeaveAlternateScreen).map_err(|e| format!("无法恢复屏幕：{e}"))?;
    Ok(())
}
