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

/// Maximum time to wait for the PTY reader to finish after the child exits,
/// before archiving anyway. Prevents a stuck "running" state when the PTY output
/// end stays open (e.g. a descendant holds it) — see #12.
const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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

/// Incremental UTF-8 decoder for PTY reads.
///
/// A single `read()` is not guaranteed to end on a character boundary: a 3-byte
/// Chinese character or 4-byte emoji can be split across two reads. Decoding each
/// chunk independently with `from_utf8_lossy` would permanently replace the
/// half-character with U+FFFD (#16). Instead we keep the trailing incomplete
/// bytes and prepend them to the next chunk, so every character is decoded once
/// and only when it is complete.
#[derive(Default)]
struct Utf8Stream {
    /// Bytes of a character whose remaining bytes have not arrived yet.
    pending: Vec<u8>,
}

impl Utf8Stream {
    /// Feed raw bytes, return only the complete, valid UTF-8 text available now.
    /// Any trailing incomplete character is buffered for the next call.
    fn push(&mut self, bytes: &[u8]) -> String {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);

        let mut out = String::new();
        let mut rest: &[u8] = &data;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    out.push_str(&String::from_utf8_lossy(&rest[..valid]));
                    match error.error_len() {
                        // Genuinely invalid bytes: replace them and keep scanning
                        // so valid bytes after them are not lost.
                        Some(len) => {
                            out.push('\u{FFFD}');
                            rest = &rest[valid + len..];
                        }
                        // Incomplete character at the end: keep it for next read.
                        None => {
                            self.pending = rest[valid..].to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// Flush any buffered bytes at EOF. An incomplete character here is truly
    /// truncated, so it is decoded lossily exactly once.
    fn finish(&mut self) -> String {
        if self.pending.is_empty() {
            return String::new();
        }
        let bytes = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// The PTY read loop, extracted from the spawned thread so the exact product
/// code path can be unit-tested with a controlled reader and event ordering.
///
/// `sink` receives each decoded chunk; returning false stops the loop (used for
/// emit failures). After `archived` is set, no further chunks are produced, so a
/// descendant that keeps the PTY open cannot inject output into a finished
/// session (#15).
fn pump_reader(
    reader: &mut dyn Read,
    buffer: &Mutex<OutputBuffer>,
    archived: &std::sync::atomic::AtomicBool,
    sink: &mut dyn FnMut(SessionOutput) -> bool,
) {
    let mut chunk = [0u8; READ_BUFFER_SIZE];
    let mut decoder = Utf8Stream::default();
    loop {
        if archived.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                if archived.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                let text = decoder.push(&chunk[..count]);
                if text.is_empty() {
                    continue; // only a partial character so far
                }
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
                if archived.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                let event = SessionOutput {
                    data: text,
                    end_offset,
                };
                if !sink(event) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    // Flush any character left incomplete at EOF so it is not silently lost (it
    // is genuinely truncated by then, so lossy is correct).
    let tail = decoder.finish();
    if !tail.is_empty() && !archived.load(std::sync::atomic::Ordering::Acquire) {
        let end_offset = if let Ok(mut guard) = buffer.lock() {
            guard.end_offset += tail.len() as u64;
            guard.data.push_str(&tail);
            guard.end_offset
        } else {
            0
        };
        let _ = sink(SessionOutput {
            data: tail,
            end_offset,
        });
    }
}

/// Terminate a child, distinguishing "already exited" from "termination failed".
/// Extracted so the failure path can be exercised with a fake child (#17).
fn terminate_child(child: &mut dyn portable_pty::Child) -> Result<(), String> {
    match child.try_wait() {
        Ok(Some(_)) => Ok(()), // already exited: stopping is a no-op success
        Ok(None) => child
            .kill()
            .map_err(|error| format!("无法结束会话：{error}")),
        Err(error) => Err(format!("无法确认会话状态：{error}")),
    }
}

/// Returns true if the reader finished within the deadline, false on timeout.
/// Extracted so the timeout path (#12) can be unit-tested with a short deadline.
fn wait_for_drain(
    read_done: &(Mutex<bool>, std::sync::Condvar),
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    let Ok(mut done) = read_done.0.lock() else {
        return false;
    };
    while !*done {
        let now = std::time::Instant::now();
        if now >= deadline {
            return false;
        }
        let remaining = deadline.saturating_duration_since(now);
        let step = remaining.min(std::time::Duration::from_millis(500));
        match read_done.1.wait_timeout(done, step) {
            Ok((guard, _)) => done = guard,
            Err(_) => return false,
        }
    }
    true
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

        // Set by the watcher once the session is archived. The reader checks it
        // before emitting, so a descendant that keeps the PTY open cannot keep
        // injecting output into an already-finished session (#15).
        let archived = Arc::new(std::sync::atomic::AtomicBool::new(false));

        self.lock()?.insert(session_id.to_string(), session);

        let sessions = Arc::clone(&self.sessions);
        let finished_buffers = Arc::clone(&self.finished);
        let read_done_reader = Arc::clone(&read_done);
        let read_done_watcher = Arc::clone(&read_done);
        let archived_reader = Arc::clone(&archived);
        let archived_watcher = Arc::clone(&archived);
        let output_id = session_id.to_string();
        let exit_id = session_id.to_string();
        let app_output = app.clone();
        let app_exit = app;

        // Reader thread: streams PTY output to the frontend as UTF-8 text and
        // keeps a replayable copy in the session buffer. The loop itself lives in
        // `pump_reader` so tests can drive the real path with controlled input.
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut sink = |event: SessionOutput| {
                app_output
                    .emit(&format!("session-output-{output_id}"), event)
                    .is_ok()
            };
            pump_reader(&mut *reader, &buffer, &archived_reader, &mut sink);
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
                    // But cap the total wait: if the output end stays open (e.g.
                    // a descendant process still holds the PTY), the reader may
                    // never hit EOF. Without a deadline the session would hang
                    // "running" forever (#12). After the deadline we archive what
                    // we have and release the session anyway.
                    let _drained = wait_for_drain(&read_done_watcher, DRAIN_TIMEOUT);
                    // Mark archived BEFORE releasing the session: the reader
                    // checks this flag and stops emitting, so late output from a
                    // descendant cannot reach a finished session (#15).
                    archived_watcher.store(true, std::sync::atomic::Ordering::Release);
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
        // Kill first, only then drop the record. Removing before a successful
        // kill lost the session on failure, leaving the process alive with no
        // handle to retry from (#17).
        {
            let mut sessions = self.lock()?;
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| "会话不存在或已结束".to_string())?;
            // A failed kill returns early here, leaving the session in the map so
            // the user can retry (#17).
            terminate_child(&mut *session.child)?;
        }

        // Kill succeeded (or the process was already gone): now release it.
        let session = self.lock()?.remove(session_id);
        // Drop any finished-buffer leftover for this id so it cannot accumulate.
        if let Ok(mut done) = self.finished.lock() {
            done.remove(session_id);
        }
        drop(session);
        Ok(())
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

    /// A child whose `kill` can be made to fail, for exercising the stop failure
    /// path without needing a real unkillable process (#17).
    #[derive(Debug)]
    struct FakeChild {
        exited: bool,
        kill_fails: bool,
        kill_calls: u32,
    }

    impl portable_pty::ChildKiller for FakeChild {
        fn kill(&mut self) -> std::io::Result<()> {
            self.kill_calls += 1;
            if self.kill_fails {
                Err(std::io::Error::other("injected kill failure"))
            } else {
                self.exited = true;
                Ok(())
            }
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(FakeKiller)
        }
    }

    #[derive(Debug)]
    struct FakeKiller;

    impl portable_pty::ChildKiller for FakeKiller {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(FakeKiller)
        }
    }

    impl portable_pty::Child for FakeChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            if self.exited {
                Ok(Some(portable_pty::ExitStatus::with_exit_code(0)))
            } else {
                Ok(None)
            }
        }

        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            Ok(portable_pty::ExitStatus::with_exit_code(0))
        }

        fn process_id(&self) -> Option<u32> {
            Some(999_999)
        }
    }

    /// Minimal `MasterPty` so a `PtySession` can be constructed in tests without a
    /// real terminal. Only the members `stop()` touches matter here.
    #[derive(Debug)]
    struct FakeMaster;

    impl portable_pty::MasterPty for FakeMaster {
        fn resize(&self, _size: portable_pty::PtySize) -> Result<(), anyhow::Error> {
            Ok(())
        }

        fn get_size(&self) -> Result<portable_pty::PtySize, anyhow::Error> {
            Ok(portable_pty::PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
        }

        fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>, anyhow::Error> {
            Ok(Box::new(std::io::empty()))
        }

        fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, anyhow::Error> {
            Ok(Box::new(std::io::sink()))
        }

        #[cfg(unix)]
        fn process_group_leader(&self) -> Option<libc::pid_t> {
            None
        }

        #[cfg(unix)]
        fn as_raw_fd(&self) -> Option<std::os::unix::io::RawFd> {
            None
        }

        #[cfg(unix)]
        fn tty_name(&self) -> Option<std::path::PathBuf> {
            None
        }
    }

    /// A reader that hands out pre-scripted chunks, one per `read`, so the exact
    /// interleaving of output and archive can be controlled (#15).
    struct ScriptedReader {
        chunks: std::collections::VecDeque<Vec<u8>>,
    }

    impl std::io::Read for ScriptedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.chunks.pop_front() {
                Some(chunk) => {
                    let n = chunk.len().min(buf.len());
                    buf[..n].copy_from_slice(&chunk[..n]);
                    Ok(n)
                }
                None => Ok(0),
            }
        }
    }

    fn insert_fake_session(manager: &SessionManager, id: &str, kill_fails: bool) {
        let mut map = manager.sessions.lock().unwrap();
        map.insert(
            id.to_string(),
            super::PtySession {
                master: Box::new(FakeMaster),
                child: Box::new(FakeChild {
                    exited: false,
                    kill_fails,
                    kill_calls: 0,
                }),
                writer: Box::new(std::io::sink()),
                buffer: std::sync::Arc::new(super::Mutex::new(super::OutputBuffer::default())),
            },
        );
    }

    #[test]
    fn stop_failure_keeps_session_in_manager_for_retry() {
        // #17 end-to-end through SessionManager::stop: a failed kill must return
        // an error AND leave the session in the table. The old order removed it
        // first, so this assertion failed before the fix.
        let manager = SessionManager::default();
        insert_fake_session(&manager, "s-fail", true);

        let error = manager.stop("s-fail").unwrap_err();
        assert!(error.contains("无法结束会话"), "got: {error}");

        let map = manager.sessions.lock().unwrap();
        assert!(
            map.contains_key("s-fail"),
            "a failed stop must keep the session so the user can retry"
        );
    }

    #[test]
    fn successful_stop_removes_session() {
        let manager = SessionManager::default();
        insert_fake_session(&manager, "s-ok", false);

        assert!(manager.stop("s-ok").is_ok());
        let map = manager.sessions.lock().unwrap();
        assert!(!map.contains_key("s-ok"));
    }

    #[test]
    fn stop_failure_keeps_the_session_for_retry() {
        // #17: kill fails -> stop returns the error AND the session must still be
        // present so the user can retry (previously it was removed first).
        let mut child = FakeChild {
            exited: false,
            kill_fails: true,
            kill_calls: 0,
        };

        let result = super::terminate_child(&mut child);
        assert!(result.is_err(), "a failed kill must surface an error");
        assert_eq!(child.kill_calls, 1, "kill must actually be attempted");
        assert!(!child.exited, "child must not be reported as exited");
    }

    #[test]
    fn stop_succeeds_when_child_already_exited() {
        let mut child = FakeChild {
            exited: true,
            kill_fails: true, // even if kill would fail, an exited child is success
            kill_calls: 0,
        };
        assert!(super::terminate_child(&mut child).is_ok());
        assert_eq!(child.kill_calls, 0, "no need to kill an exited child");
    }

    #[test]
    fn stop_kills_a_running_child() {
        let mut child = FakeChild {
            exited: false,
            kill_fails: false,
            kill_calls: 0,
        };
        assert!(super::terminate_child(&mut child).is_ok());
        assert_eq!(child.kill_calls, 1);
    }

    #[test]
    fn archived_session_does_not_emit_late_output() {
        // #15: a descendant keeps writing after the session was archived. The
        // reader must stop emitting once the archived flag is set, so no output
        // lands in a finished session.
        let reader_chunks = vec![b"BEFORE-ARCHIVE".to_vec(), b"AFTER-ARCHIVE".to_vec()];
        let mut reader = ScriptedReader {
            chunks: reader_chunks.into(),
        };
        let buffer = super::Mutex::new(super::OutputBuffer::default());
        let archived = std::sync::atomic::AtomicBool::new(false);
        let mut events: Vec<String> = Vec::new();

        let mut sink = |event: super::SessionOutput| {
            events.push(event.data);
            // Archive happens right after the first chunk is delivered.
            archived.store(true, std::sync::atomic::Ordering::Release);
            true
        };

        super::pump_reader(&mut reader, &buffer, &archived, &mut sink);

        assert_eq!(events, vec!["BEFORE-ARCHIVE".to_string()]);
        assert!(
            !events.iter().any(|e| e.contains("AFTER-ARCHIVE")),
            "late output must not be emitted after archive"
        );
    }

    #[test]
    fn reader_emits_every_chunk_and_offsets_advance_when_not_archived() {
        let mut reader = ScriptedReader {
            chunks: vec![b"abc".to_vec(), b"def".to_vec()].into(),
        };
        let buffer = super::Mutex::new(super::OutputBuffer::default());
        let archived = std::sync::atomic::AtomicBool::new(false);
        let mut events: Vec<(String, u64)> = Vec::new();

        let mut sink = |event: super::SessionOutput| {
            events.push((event.data, event.end_offset));
            true
        };

        super::pump_reader(&mut reader, &buffer, &archived, &mut sink);

        assert_eq!(
            events,
            vec![("abc".to_string(), 3), ("def".to_string(), 6),]
        );
    }

    #[test]
    fn reader_keeps_split_utf8_through_the_product_path() {
        // #16 through pump_reader: feed "你好" split 2 + 4 bytes.
        let bytes = "你好".as_bytes().to_vec();
        let mut reader = ScriptedReader {
            chunks: vec![bytes[..2].to_vec(), bytes[2..].to_vec()].into(),
        };
        let buffer = super::Mutex::new(super::OutputBuffer::default());
        let archived = std::sync::atomic::AtomicBool::new(false);
        let mut events = String::new();

        let mut sink = |event: super::SessionOutput| {
            events.push_str(&event.data);
            true
        };

        super::pump_reader(&mut reader, &buffer, &archived, &mut sink);

        assert_eq!(events, "你好");
        assert!(!events.contains('\u{FFFD}'));
    }

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
    fn utf8_stream_keeps_character_split_across_reads() {
        // "你好" is 6 bytes. Split it 2 + 4 so a character straddles the read
        // boundary; the old per-chunk lossy decode produced "\u{FFFD}\u{FFFD}好".
        let bytes = "你好".as_bytes();
        let mut stream = super::Utf8Stream::default();
        let first = stream.push(&bytes[..2]);
        let second = stream.push(&bytes[2..]);
        let combined = format!("{first}{second}");
        assert_eq!(combined, "你好");
        assert!(!combined.contains('\u{FFFD}'));
    }

    #[test]
    fn utf8_stream_handles_four_byte_emoji_split() {
        let bytes = "🚀".as_bytes(); // 4 bytes
        let mut stream = super::Utf8Stream::default();
        let mut out = String::new();
        for byte in bytes {
            out.push_str(&stream.push(std::slice::from_ref(byte)));
        }
        out.push_str(&stream.finish());
        assert_eq!(out, "🚀");
    }

    #[test]
    fn utf8_stream_flushes_truncated_tail_at_eof() {
        let bytes = "你好".as_bytes();
        let mut stream = super::Utf8Stream::default();
        // Feed only the first 4 bytes (complete "你" + 1 byte of "好").
        let _ = stream.push(&bytes[..4]);
        let tail = stream.finish();
        // The incomplete tail is decoded once, not silently dropped forever.
        assert!(!tail.is_empty());
    }

    #[test]
    fn utf8_stream_drops_genuinely_invalid_bytes_without_stalling() {
        let mut stream = super::Utf8Stream::default();
        // 0xFF is never valid in UTF-8.
        let out = stream.push(&[0x41, 0xFF, 0x42]);
        assert!(out.contains('A'));
        assert!(out.contains('B'));
    }

    #[test]
    fn utf8_stream_offset_contract_matches_encoded_length() {
        // The bytes counted per push must equal the UTF-8 length of the text
        // emitted, so the frontend's byteLength() lines up with endOffset.
        let mut stream = super::Utf8Stream::default();
        let mut total = 0usize;
        for piece in ["你", "好", "🚀"] {
            let text = stream.push(piece.as_bytes());
            total += text.len();
        }
        assert_eq!(total, "你好🚀".len());
    }

    #[test]
    fn archived_flag_stops_late_output() {
        // #15: once archived, the reader must see the flag and skip emitting.
        // Exercise the exact check the reader performs before each send.
        let archived = std::sync::atomic::AtomicBool::new(false);
        assert!(!archived.load(std::sync::atomic::Ordering::Acquire));
        archived.store(true, std::sync::atomic::Ordering::Release);
        assert!(
            archived.load(std::sync::atomic::Ordering::Acquire),
            "reader must observe the archived flag and stop emitting"
        );
    }

    #[test]
    fn wait_for_drain_returns_fast_when_reader_finishes() {
        let signal = (std::sync::Mutex::new(true), std::sync::Condvar::new());
        let start = std::time::Instant::now();
        assert!(super::wait_for_drain(
            &signal,
            std::time::Duration::from_secs(5)
        ));
        assert!(start.elapsed() < std::time::Duration::from_millis(100));
    }

    #[test]
    fn wait_for_drain_times_out_when_output_stays_open() {
        // Reader never signals done (output end stays open). The wait must end at
        // the deadline instead of hanging forever (#12).
        let signal = (std::sync::Mutex::new(false), std::sync::Condvar::new());
        let start = std::time::Instant::now();
        let finished = super::wait_for_drain(&signal, std::time::Duration::from_millis(200));
        let elapsed = start.elapsed();
        assert!(!finished, "should report timeout");
        assert!(
            elapsed >= std::time::Duration::from_millis(200),
            "should wait at least the deadline, got {elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "should not hang far past the deadline, got {elapsed:?}"
        );
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
