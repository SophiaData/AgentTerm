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
    let mut terminal = enter_ui()?;

    loop {
        terminal
            .draw(|frame| ui::render(frame, state))
            .map_err(|e| format!("无法绘制界面：{e}"))?;

        if let Some(request) = state.pending_launch.take() {
            // Leave our full-screen UI, let the tool own the terminal, wait for
            // it to exit, then restore our UI. Because we spawn+wait (not exec),
            // the process survives and we always come back to the list.
            restore_terminal()?;
            // Drop the key that triggered the launch (and anything queued with
            // it) before the tool takes over the terminal.
            drain_pending_input();
            let outcome = launch::run(&request.program, &request.args, &request.dir);

            // Rebuild the terminal from scratch. Reusing the old backend after a
            // child process ran is what caused "cursor position could not be
            // read" and made us exit; a fresh Terminal avoids that stale state.
            terminal = enter_ui()?;

            // Drain any pending input the child may have left behind. Tools send
            // a trailing Enter/newline when they exit; without this, that stray
            // key press lands on our list and immediately relaunches a tool.
            drain_pending_input();

            match outcome {
                launch::Outcome::Finished { .. } => {
                    // Normal exit: just redraw the list, no scary error.
                    state.status = "已返回列表".to_string();
                }
                launch::Outcome::FailedToStart(reason) => {
                    state.set_error(format!("启动 {} 失败：{reason}", request.program));
                }
            }
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

fn enter_ui() -> Result<ratatui::Terminal<CrosstermBackend<std::io::Stdout>>, String> {
    enable_raw_mode().map_err(|e| format!("无法进入终端模式：{e}"))?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen).map_err(|e| format!("无法切换屏幕：{e}"))?;
    let backend = CrosstermBackend::new(out);
    Terminal::new(backend).map_err(|e| format!("无法创建界面：{e}"))
}

fn restore_terminal() -> Result<(), String> {
    // Best-effort: a tool that scrambled terminal state should not stop us from
    // getting out cleanly. Ignore individual failures so we never crash on exit.
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    Ok(())
}

/// Throw away any input events already queued. Called right after a child tool
/// exits and before we resume reading keys, so a trailing Enter/newline from the
/// tool cannot be mistaken for a key press in our list.
fn drain_pending_input() {
    while matches!(event::poll(std::time::Duration::from_millis(0)), Ok(true)) {
        if event::read().is_err() {
            break;
        }
    }
}
