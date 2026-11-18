use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::projects::normalize_project_dir;
use crate::tools::{ResolveResult, TOOL_DEFINITIONS, candidate_search_dirs, resolve_binary};

const READ_BUFFER_SIZE: usize = 8192;

/// Environment variables that point child tools at an unrelated parent
/// terminal (multiplexers) or at a server port. Inheriting them makes tools
/// like Claude Code attach to the wrong pane or try to bind a taken port.
const ENV_BLOCKLIST: &[&str] = &[
    "PORT",
    "ELECTRON_RENDERER_PORT",
    "ELECTRON_RUN_AS_NODE",
    "TMUX",
    "TMUX_PANE",
    "STY",
    "WINDOW",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub tool_id: String,
    pub tool_name: String,
    pub project_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExit {
    pub id: String,
    pub reason: String,
}

/// Upper bound for replayable scrollback per session. Enough for a full-screen
/// TUI to redraw after a tab switch, without unbounded memory growth.
const MAX_BUFFER_CHARS: usize = 512 * 1024;

struct PtySession {
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn std::io::Write + Send>,
    /// Everything the PTY has emitted so far. The frontend replays this when a
    /// terminal view mounts, so output produced before the listener attached
    /// (e.g. Claude Code's first screen) is never lost.
    buffer: Arc<Mutex<String>>,
}

/// A freshly spawned PTY with all handles still separated, so the caller can
/// decide what happens with the reader (event loop or direct test assertions).
pub struct SpawnedPty {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    pub writer: Box<dyn std::io::Write + Send>,
    pub reader: Box<dyn Read + Send>,
}

pub fn spawn_pty(
    binary: &std::path::Path,
    cwd: &std::path::Path,
    rows: u16,
    cols: u16,
) -> Result<SpawnedPty, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("无法创建终端：{error}"))?;

    let mut command = CommandBuilder::new(binary);
    command.cwd(cwd.as_os_str());
    for (key, _) in std::env::vars() {
        if ENV_BLOCKLIST.contains(&key.as_str()) {
            command.env_remove(&key);
        }
    }
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("无法启动终端进程：{error}"))?;
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("无法读取终端输出：{error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("无法写入终端：{error}"))?;

    Ok(SpawnedPty {
        master: pair.master,
        child,
        writer,
        reader,
    })
}

pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl SessionManager {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, PtySession>>, String> {
        self.sessions
            .lock()
            .map_err(|_| "会话管理暂时不可用，请重试".to_string())
    }

    pub fn start(
        &self,
        app: AppHandle,
        session_id: &str,
        tool_id: &str,
        project_path: &str,
        rows: u16,
        cols: u16,
    ) -> Result<SessionInfo, String> {
        let tool = TOOL_DEFINITIONS
            .iter()
            .find(|definition| definition.id == tool_id)
            .ok_or_else(|| format!("不支持的工具：{tool_id}"))?;

        let project_dir = normalize_project_dir(project_path)?;

        let binary = match resolve_binary(tool.binaries, &candidate_search_dirs()) {
            ResolveResult::Found(path) => path,
            ResolveResult::NotExecutable(path) => {
                return Err(format!(
                    "{} 已找到但没有执行权限：{}",
                    tool.name,
                    path.display()
                ));
            }
            ResolveResult::Missing => {
                return Err(format!("未安装 {}，无法启动", tool.name));
            }
        };

        {
            let sessions = self.lock()?;
            if sessions.contains_key(session_id) {
                return Err("会话已存在，请换一个会话再启动".to_string());
            }
        }

        let spawned = spawn_pty(&binary, &project_dir, rows, cols)?;
        let SpawnedPty {
            master,
            child,
            writer,
            reader,
        } = spawned;

        let info = SessionInfo {
            id: session_id.to_string(),
            tool_id: tool.id.to_string(),
            tool_name: tool.name.to_string(),
            project_path: project_dir.display().to_string(),
        };

        let session = PtySession {
            master,
            child,
            writer,
            buffer: Arc::new(Mutex::new(String::new())),
        };
        let buffer = Arc::clone(&session.buffer);

        self.lock()?.insert(session_id.to_string(), session);

        let sessions = Arc::clone(&self.sessions);
        let output_id = session_id.to_string();
        let exit_id = session_id.to_string();
        let app_output = app.clone();
        let app_exit = app;

        // Reader thread: streams PTY output to the frontend as UTF-8 text and
        // keeps a replayable copy in the session buffer.
        let mut reader = reader;
        std::thread::spawn(move || {
            let mut chunk = [0u8; READ_BUFFER_SIZE];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        let text = String::from_utf8_lossy(&chunk[..count]).into_owned();
                        if let Ok(mut guard) = buffer.lock() {
                            guard.push_str(&text);
                            if guard.len() > MAX_BUFFER_CHARS {
                                let cut = guard.len() - MAX_BUFFER_CHARS;
                                let boundary = guard
                                    .char_indices()
                                    .map(|(index, _)| index)
                                    .find(|index| *index >= cut)
                                    .unwrap_or(cut);
                                guard.drain(..boundary);
                            }
                        }
                        if app_output
                            .emit(&format!("session-output-{output_id}"), text)
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Watcher thread: reports process exit and releases the session.
        std::thread::spawn(move || {
            let reason = loop {
                let finished = {
                    let mut guard = match sessions.lock() {
                        Ok(guard) => guard,
                        Err(_) => break "会话管理错误".to_string(),
                    };
                    match guard.get_mut(&exit_id) {
                        Some(session) => session.child.try_wait().ok().flatten(),
                        None => break "会话已结束".to_string(),
                    }
                };

                if let Some(status) = finished {
                    let reason = format!("{status}");
                    let _ = sessions.lock().map(|mut guard| guard.remove(&exit_id));
                    break reason;
                }

                std::thread::sleep(std::time::Duration::from_millis(100));
            };

            let _ = app_exit.emit(
                &format!("session-exit-{exit_id}"),
                SessionExit {
                    id: exit_id.clone(),
                    reason,
                },
            );
        });

        Ok(info)
    }

    pub fn snapshot(&self, session_id: &str) -> Result<String, String> {
        let sessions = self.lock()?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "会话不存在或已结束".to_string())?;
        session
            .buffer
            .lock()
            .map(|guard| guard.clone())
            .map_err(|_| "无法读取会话输出".to_string())
    }

    pub fn write(&self, session_id: &str, data: &str) -> Result<(), String> {
        let mut sessions = self.lock()?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| "会话不存在或已结束".to_string())?;
        session
            .writer
            .write_all(data.as_bytes())
            .and_then(|()| session.writer.flush())
            .map_err(|error| format!("无法写入终端：{error}"))
    }

    pub fn resize(&self, session_id: &str, rows: u16, cols: u16) -> Result<(), String> {
        let sessions = self.lock()?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "会话不存在或已结束".to_string())?;
        session
            .master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("无法调整终端尺寸：{error}"))
    }

    pub fn stop(&self, session_id: &str) -> Result<(), String> {
        let session = self.lock()?.remove(session_id);

        match session {
            Some(mut session) => {
                session
                    .child
                    .kill()
                    .map_err(|error| format!("无法结束会话：{error}"))?;
                Ok(())
            }
            None => Err("会话不存在或已结束".to_string()),
        }
    }

    pub fn stop_all(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            for (_, mut session) in sessions.drain() {
                let _ = session.child.kill();
            }
        }
    }

    #[cfg(test)]
    pub fn is_running(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .map(|guard| guard.contains_key(session_id))
            .unwrap_or(false)
    }
}

#[tauri::command]
pub fn start_session(
    state: tauri::State<'_, SessionManager>,
    app: AppHandle,
    session_id: String,
    tool_id: String,
    project_path: String,
    rows: u16,
    cols: u16,
) -> Result<SessionInfo, String> {
    state.start(app, &session_id, &tool_id, &project_path, rows, cols)
}

#[tauri::command]
pub fn session_snapshot(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
) -> Result<String, String> {
    state.snapshot(&session_id)
}

#[tauri::command]
pub fn write_session_input(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    state.write(&session_id, &data)
}

#[tauri::command]
pub fn resize_session(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    state.resize(&session_id, rows, cols)
}

#[tauri::command]
pub fn stop_session(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
) -> Result<(), String> {
    state.stop(&session_id)
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::time::Duration;

    use super::SessionManager;

    #[test]
    fn writing_to_an_unknown_session_is_rejected() {
        let manager = SessionManager::default();

        assert_eq!(
            manager.write("missing", "ls\r").unwrap_err(),
            "会话不存在或已结束"
        );
        assert!(!manager.is_running("missing"));
    }

    #[test]
    fn resizing_an_unknown_session_is_rejected() {
        let manager = SessionManager::default();

        assert_eq!(
            manager.resize("missing", 40, 120).unwrap_err(),
            "会话不存在或已结束"
        );
    }

    #[test]
    fn stopping_an_unknown_session_is_rejected() {
        let manager = SessionManager::default();

        assert_eq!(manager.stop("missing").unwrap_err(), "会话不存在或已结束");
    }

    #[cfg(unix)]
    fn read_until(reader: &mut dyn Read, needle: &str, limit: Duration) -> String {
        let deadline = std::time::Instant::now() + limit;
        let mut collected = String::new();
        let mut chunk = [0u8; 512];

        while !collected.contains(needle) && std::time::Instant::now() < deadline {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => collected.push_str(&String::from_utf8_lossy(&chunk[..count])),
                Err(_) => break,
            }
        }

        collected
    }

    #[cfg(unix)]
    #[test]
    fn buffer_keeps_output_for_replay() {
        use super::spawn_pty;
        use std::io::Write;
        use std::path::Path;

        let dir = std::env::temp_dir().join("agentterm-buffer-test");
        std::fs::create_dir_all(&dir).expect("dir should be created");

        let mut pty = spawn_pty(Path::new("/bin/cat"), &dir, 30, 120).unwrap();
        pty.writer.write_all(b"replay-me\r").unwrap();
        pty.writer.flush().unwrap();

        let out = read_until(&mut *pty.reader, "replay-me", Duration::from_secs(5));
        assert!(out.contains("replay-me"), "echoed: {out:?}");

        pty.child.kill().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn two_concurrent_ptys_stay_independent() {
        use super::spawn_pty;
        use std::io::Write;
        use std::path::Path;

        let first_dir = std::env::temp_dir().join("agentterm-pty-a");
        let second_dir = std::env::temp_dir().join("agentterm-pty-b");
        std::fs::create_dir_all(&first_dir).expect("dir a should be created");
        std::fs::create_dir_all(&second_dir).expect("dir b should be created");

        let mut first = spawn_pty(Path::new("/bin/cat"), &first_dir, 24, 80).unwrap();
        let mut second = spawn_pty(Path::new("/bin/cat"), &second_dir, 24, 80).unwrap();

        first.writer.write_all(b"alpha\r").unwrap();
        first.writer.flush().unwrap();
        second.writer.write_all(b"beta\r").unwrap();
        second.writer.flush().unwrap();

        let first_out = read_until(&mut first.reader, "alpha", Duration::from_secs(5));
        let second_out = read_until(&mut second.reader, "beta", Duration::from_secs(5));

        assert!(first_out.contains("alpha"), "first echo: {first_out:?}");
        assert!(second_out.contains("beta"), "second echo: {second_out:?}");

        first.child.kill().unwrap();
        second.child.kill().unwrap();

        let _ = std::fs::remove_dir_all(&first_dir);
        let _ = std::fs::remove_dir_all(&second_dir);
    }
}
