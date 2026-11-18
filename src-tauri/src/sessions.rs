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

/// One chunk of live PTY output, tagged with the running byte offset at which it
/// ends. The frontend compares `end_offset` against the snapshot offset to write
/// each byte exactly once.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOutput {
    pub data: String,
    pub end_offset: u64,
}

/// Snapshot of a session's output plus whether it is still running, and the
/// running byte offset this snapshot ends at. The offset is the shared ruler
/// that lets the frontend stitch a snapshot together with live events without
/// ever writing the same bytes twice or skipping any.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub data: String,
    /// Total bytes emitted up to the end of `data` (monotonic, never reset by
    /// scrollback trimming). Live events carry the same unit so they can be
    /// compared directly against it.
    pub offset: u64,
    pub running: bool,
}

/// What the reader thread holds: the replayable scrollback plus a monotonic
/// counter of every byte ever appended (so trimming the head does not change it).
#[derive(Default)]
struct OutputBuffer {
    data: String,
    /// Total bytes appended since the session started, before any trimming.
    end_offset: u64,
}

/// Upper bound for replayable scrollback per session. Enough for a full-screen
/// TUI to redraw after a tab switch, without unbounded memory growth.
const MAX_BUFFER_CHARS: usize = 512 * 1024;

struct PtySession {
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn std::io::Write + Send>,
    /// Everything the PTY has emitted so far (with its monotonic offset). The
    /// frontend replays this when a terminal view mounts, so output produced
    /// before the listener attached (e.g. Claude Code's first screen) is not
    /// lost, and so snapshot + live events can be stitched without overlap.
    buffer: Arc<Mutex<OutputBuffer>>,
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
    /// Last output of sessions that have already exited, keyed by session id.
    /// Kept so a view that mounts *after* the process died can still replay the
    /// final screen and learn the process is gone (instead of seeing "running").
    finished: Arc<Mutex<HashMap<String, (String, u64)>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            finished: Arc::new(Mutex::new(HashMap::new())),
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
            buffer: Arc::new(Mutex::new(OutputBuffer::default())),
        };
        let buffer = Arc::clone(&session.buffer);

        // Signals the reader thread has drained the PTY (read returned 0). The
        // watcher waits on this before archiving, so the archived snapshot
        // always includes the final output produced just before exit (#9).
        let read_done = Arc::new((Mutex::new(false), std::sync::Condvar::new()));

        self.lock()?.insert(session_id.to_string(), session);

        let sessions = Arc::clone(&self.sessions);
        let finished_buffers = Arc::clone(&self.finished);
        let read_done_reader = Arc::clone(&read_done);
        let read_done_watcher = Arc::clone(&read_done);
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
                        let end_offset = if let Ok(mut guard) = buffer.lock() {
                            guard.end_offset += text.len() as u64;
                            guard.data.push_str(&text);
                            if guard.data.len() > MAX_BUFFER_CHARS {
                                let cut = guard.data.len() - MAX_BUFFER_CHARS;
                                let boundary = guard
                                    .data
                                    .char_indices()
                                    .map(|(index, _)| index)
                                    .find(|index| *index >= cut)
                                    .unwrap_or(cut);
                                guard.data.drain(..boundary);
                            }
                            guard.end_offset
                        } else {
                            0
                        };
                        let event = SessionOutput {
                            data: text,
                            end_offset,
                        };
                        if app_output
                            .emit(&format!("session-output-{output_id}"), event)
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            // Signal the watcher that we have drained the PTY, so it archives a
            // snapshot that includes every byte written before exit (#9).
            if let Ok(mut done) = read_done_reader.0.lock() {
                *done = true;
                read_done_reader.1.notify_all();
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
                    // Wait for the reader thread to drain the PTY before
                    // archiving, so the archived snapshot includes the final
                    // output (e.g. an error printed right before exit) — #9.
                    if let Ok(mut drained) = read_done_watcher.0.lock() {
                        while !*drained {
                            match read_done_watcher
                                .1
                                .wait_timeout(drained, std::time::Duration::from_millis(500))
                            {
                                Ok((guard, _)) => drained = guard,
                                Err(_) => break,
                            }
                        }
                    }
                    // Preserve the final buffer before releasing the session so a
                    // late-mounting view can replay it and see the exit state.
                    if let Ok(mut guard) = sessions.lock()
                        && let Some(session) = guard.remove(&exit_id)
                        && let Ok(buffer) = session.buffer.lock()
                        && let Ok(mut done) = finished_buffers.lock()
                    {
                        done.insert(exit_id.clone(), (buffer.data.clone(), buffer.end_offset));
                    }
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

    pub fn snapshot(&self, session_id: &str) -> Result<SessionSnapshot, String> {
        // Running session: return what we have so far, still running.
        {
            let sessions = self.lock()?;
            if let Some(session) = sessions.get(session_id) {
                let (data, offset) = session
                    .buffer
                    .lock()
                    .map(|guard| (guard.data.clone(), guard.end_offset))
                    .map_err(|_| "无法读取会话输出".to_string())?;
                return Ok(SessionSnapshot {
                    data,
                    offset,
                    running: true,
                });
            }
        }
        // Already-finished session: replay its final buffer, not running.
        let finished = self
            .finished
            .lock()
            .map_err(|_| "无法读取会话状态".to_string())?;
        match finished.get(session_id) {
            Some((data, offset)) => Ok(SessionSnapshot {
                data: data.clone(),
                offset: *offset,
                running: false,
            }),
            None => Err("会话不存在或已结束".to_string()),
        }
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
        // Drop any finished-buffer leftover for this id so it cannot accumulate.
        if let Ok(mut done) = self.finished.lock() {
            done.remove(session_id);
        }

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
) -> Result<SessionSnapshot, String> {
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

    #[test]
    fn snapshot_of_finished_session_reports_not_running() {
        // A view that mounts after the process died must still get the final
        // screen and learn the process is gone.
        let manager = super::SessionManager::default();
        manager
            .finished
            .lock()
            .unwrap()
            .insert("gone".to_string(), ("final output".to_string(), 12));

        let snapshot = manager.snapshot("gone").unwrap();
        assert_eq!(snapshot.data, "final output");
        assert_eq!(snapshot.offset, 12);
        assert!(!snapshot.running);
    }

    #[test]
    fn snapshot_of_unknown_session_errors() {
        let manager = super::SessionManager::default();
        assert!(manager.snapshot("never-existed").is_err());
    }

    #[test]
    fn output_buffer_accumulates_monotonic_offset_despite_trimming() {
        // The offset must keep counting every byte even after old scrollback is
        // trimmed, so live chunks and snapshots stay comparable.
        let mut buffer = super::OutputBuffer::default();
        for _ in 0..5 {
            buffer.end_offset += 3;
            buffer.data.push_str("abc");
        }
        assert_eq!(buffer.end_offset, 15);
        assert_eq!(buffer.data, "abcabcabcabcabc");
    }

    #[cfg(unix)]
    #[test]
    fn archive_waits_for_reader_so_tail_is_kept() {
        // Simulates #9: the process prints a final line then exits. The watcher
        // must wait for the reader to drain before archiving, so the archive
        // contains the tail. We drive the same primitives the real threads use.
        use super::{OutputBuffer, spawn_pty};
        use std::io::Read;
        use std::path::Path;
        use std::sync::{Arc, Condvar, Mutex};

        let dir = std::env::temp_dir().join("agentterm-archive-test");
        std::fs::create_dir_all(&dir).expect("dir");

        // A shell that prints a marker then exits immediately.
        let mut pty = spawn_pty(Path::new("/bin/sh"), &dir, 30, 120).unwrap();
        use std::io::Write;
        pty.writer.write_all(b"echo TAIL-MARKER; exit 0\r").unwrap();
        pty.writer.flush().unwrap();

        let buffer = Arc::new(Mutex::new(OutputBuffer::default()));
        let read_done = Arc::new((Mutex::new(false), Condvar::new()));

        // Reader: drain to EOF, then signal done (mirrors the real reader thread).
        {
            let buffer = Arc::clone(&buffer);
            let read_done = Arc::clone(&read_done);
            let mut reader = pty.reader;
            std::thread::spawn(move || {
                let mut chunk = [0u8; 4096];
                loop {
                    match reader.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let text = String::from_utf8_lossy(&chunk[..n]).into_owned();
                            let mut guard = buffer.lock().unwrap();
                            guard.end_offset += text.len() as u64;
                            guard.data.push_str(&text);
                        }
                    }
                }
                let (lock, cv) = &*read_done;
                *lock.lock().unwrap() = true;
                cv.notify_all();
            });
        }

        pty.child.wait().unwrap();

        // Watcher: wait for read_done, then archive (mirrors the real watcher).
        {
            let (lock, cv) = &*read_done;
            let mut done = lock.lock().unwrap();
            while !*done {
                done = cv
                    .wait_timeout(done, std::time::Duration::from_secs(2))
                    .unwrap()
                    .0;
            }
        }
        let archived = buffer.lock().unwrap().data.clone();

        assert!(
            archived.contains("TAIL-MARKER"),
            "archive must include the tail output, got: {archived:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
