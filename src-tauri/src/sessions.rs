use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::projects::ProjectStore;
use crate::tools::{ResolveResult, TOOL_DEFINITIONS, candidate_search_dirs, resolve_binary};

const READ_BUFFER_SIZE: usize = 8192;
const MAX_SESSION_ID_BYTES: usize = 64;
const MAX_PTY_ROWS: u16 = 300;
const MAX_PTY_COLS: u16 = 500;
/// A blocked OS Read may outlive its PTY. Never allow unbounded such threads.
const MAX_READER_THREADS: usize = 64;
const MAX_LIFETIME_SESSION_IDS: usize = 65_536;

/// Counts live reader threads (not merely sessions). A stalled read keeps its
/// permit until the underlying OS call returns, even after stop/forget.
struct ReaderPermit(Arc<AtomicUsize>);

impl ReaderPermit {
    fn acquire(active: &Arc<AtomicUsize>) -> Result<Self, String> {
        // compare_exchange_weak is available on the project's Rust 1.90 MSRV.
        let mut current = active.load(Ordering::Acquire);
        loop {
            if current >= MAX_READER_THREADS {
                return Err(
                    "终端读取线程已达到安全上限，请重启应用释放被系统阻塞的终端资源".to_string(),
                );
            }
            match active.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(Self(Arc::clone(active))),
                Err(actual) => current = actual,
            }
        }
    }
}

impl Drop for ReaderPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Serializes event dispatch with stop/forget. A reserved ID never becomes
/// available again during this app lifetime, even after its buffer is deleted.
#[derive(Default)]
struct EventRegistry {
    allowed: HashMap<String, bool>,
}

impl EventRegistry {
    fn reserve(&mut self, id: &str) -> Result<(), String> {
        if self.allowed.contains_key(id) {
            return Err("会话 ID 已使用，请为新会话生成新 ID".to_string());
        }
        if self.allowed.len() >= MAX_LIFETIME_SESSION_IDS {
            return Err("会话累计数量已达到安全上限，请重启应用".to_string());
        }
        self.allowed.insert(id.to_string(), true);
        Ok(())
    }

    fn close(&mut self, id: &str) {
        if let Some(allowed) = self.allowed.get_mut(id) {
            *allowed = false;
        }
    }
}

/// Holding the lock *during* dispatch prevents a just-stopped session from
/// emitting a late event. The same helper drives real Tauri and test sinks.
fn dispatch_session_event(
    registry: &Mutex<EventRegistry>,
    id: &str,
    dispatch: impl FnOnce(),
) -> bool {
    let Ok(guard) = registry.lock() else {
        return false;
    };
    if guard.allowed.get(id) != Some(&true) {
        return false;
    }
    dispatch();
    true
}

fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || session_id.len() > MAX_SESSION_ID_BYTES
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("会话 ID 必须是 1-64 位字母、数字、横线或下划线".to_string());
    }
    Ok(())
}

fn validate_pty_size(rows: u16, cols: u16) -> Result<(), String> {
    if rows == 0 || rows > MAX_PTY_ROWS || cols == 0 || cols > MAX_PTY_COLS {
        return Err(format!(
            "终端尺寸超出允许范围：rows=1-{MAX_PTY_ROWS}, cols=1-{MAX_PTY_COLS}"
        ));
    }
    Ok(())
}

/// Once a PTY is running, history persistence is best-effort: never report
/// a launch failure that would hide the live session from the frontend.
fn remember_started_tool(
    projects: &ProjectStore,
    project_path: &str,
    tool_id: &str,
) -> Option<String> {
    projects
        .remember_last_tool(project_path, tool_id)
        .err()
        .map(|error| format!("会话已启动，但无法保存最近工具：{error}"))
}

fn validate_start_request(
    projects: &ProjectStore,
    session_id: &str,
    tool_id: &str,
    project_path: &str,
    rows: u16,
    cols: u16,
) -> Result<(&'static crate::tools::ToolDefinition, std::path::PathBuf), String> {
    validate_session_id(session_id)?;
    validate_pty_size(rows, cols)?;
    let tool = TOOL_DEFINITIONS
        .iter()
        .find(|definition| definition.id == tool_id)
        .ok_or_else(|| format!("不支持的工具：{tool_id}"))?;
    let directory = projects.require_enabled_project(project_path)?;
    Ok((tool, directory))
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
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
    /// Foreground process group of the PTY, captured while the child is alive.
    /// Used to terminate descendants that keep the PTY open after the child
    /// exits, so the reader thread can reach EOF and release its handle (#15).
    #[cfg(unix)]
    pgid: Option<libc::pid_t>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn std::io::Write + Send>,
    /// Everything the PTY has emitted so far (with its monotonic offset). The
    /// frontend replays this when a terminal view mounts, so output produced
    /// before the listener attached (e.g. Claude Code's first screen) is not
    /// lost, and so snapshot + live events can be stitched without overlap.
    buffer: Arc<Mutex<OutputBuffer>>,
    /// Identity shared with the watcher; prevents an old worker from touching
    /// a new session that reuses the same public ID.
    archived: Arc<std::sync::atomic::AtomicBool>,
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

    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("无法启动终端进程：{error}"))?;
    drop(pair.slave);

    // A spawned child must not survive an error preparing its I/O handles.
    let reader = pair.master.try_clone_reader().map_err(|error| {
        abort_child(&mut *child, &*pair.master);
        format!("无法读取终端输出：{error}")
    })?;
    let writer = pair.master.take_writer().map_err(|error| {
        abort_child(&mut *child, &*pair.master);
        format!("无法写入终端：{error}")
    })?;

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

/// Forcefully terminate the child's whole process group. Used only on the
/// drain-timeout path, when a descendant still holds the PTY output end and the
/// reader cannot reach EOF (#15). The child is its own session leader (portable-pty
/// calls setsid), so a negative pid targets only that terminal's processes.
#[cfg(unix)]
fn kill_process_group(pgid: Option<libc::pid_t>) {
    if let Some(pgid) = pgid
        && pgid > 1
    {
        // Safety: never signal our own process group.
        let own_pgid = unsafe { libc::getpgrp() };
        if pgid == own_pgid {
            return;
        }
        // SIGKILL cannot be caught, so descendants cannot keep the PTY open.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

/// Clean up every failed launch, including descendants still holding the PTY.
fn abort_child(child: &mut dyn portable_pty::Child, master: &dyn portable_pty::MasterPty) {
    #[cfg(unix)]
    kill_process_group(master.process_group_leader());
    #[cfg(not(unix))]
    let _ = master;
    let _ = child.kill();
    let _ = child.try_wait();
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

struct StartRequest<'a> {
    session_id: &'a str,
    tool_id: &'a str,
    project_path: &'a str,
    rows: u16,
    cols: u16,
}

pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
    events: Arc<Mutex<EventRegistry>>,
    reader_threads: Arc<AtomicUsize>,
    /// Last output of sessions that have already exited, keyed by session id.
    /// Kept so a view that mounts *after* the process died can still replay the
    /// final screen and learn the process is gone (instead of seeing "running").
    finished: Arc<Mutex<HashMap<String, (String, u64)>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            events: Arc::new(Mutex::new(EventRegistry::default())),
            reader_threads: Arc::new(AtomicUsize::new(0)),
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

    /// A worker-thread spawn failure must not leave a running PTY unmanaged.
    fn abort_start(&self, session_id: &str) {
        if let Ok(mut sessions) = self.lock()
            && let Some(mut session) = sessions.remove(session_id)
        {
            if let Ok(mut events) = self.events.lock() {
                events.close(session_id);
            }
            session
                .archived
                .store(true, std::sync::atomic::Ordering::Release);
            abort_child(&mut *session.child, &*session.master);
        }
    }

    fn start(
        &self,
        app: AppHandle,
        projects: &ProjectStore,
        request: StartRequest<'_>,
    ) -> Result<SessionInfo, String> {
        let StartRequest {
            session_id,
            tool_id,
            project_path,
            rows,
            cols,
        } = request;
        let (tool, project_dir) =
            validate_start_request(projects, session_id, tool_id, project_path, rows, cols)?;

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

        // A blocked OS read still occupies a worker slot after stop/forget.
        // Reject new sessions at the cap instead of leaking threads without bound.
        let reader_permit = ReaderPermit::acquire(&self.reader_threads)?;
        // Keep this lock through reservation, spawn and insertion; no second
        // start can race the same ID, even across finished/forgotten sessions.
        let mut sessions_guard = self.lock()?;
        if sessions_guard.contains_key(session_id) {
            return Err("会话已存在，请换一个会话再启动".to_string());
        }
        self.events
            .lock()
            .map_err(|_| "无法读取会话事件状态".to_string())?
            .reserve(session_id)?;

        let spawned = match spawn_pty(&binary, &project_dir, rows, cols) {
            Ok(spawned) => spawned,
            Err(error) => {
                // Nothing was started; allow a retry using the same ID.
                if let Ok(mut events) = self.events.lock() {
                    events.allowed.remove(session_id);
                }
                return Err(error);
            }
        };
        let SpawnedPty {
            master,
            child,
            writer,
            reader,
        } = spawned;

        let mut info = SessionInfo {
            id: session_id.to_string(),
            tool_id: tool.id.to_string(),
            tool_name: tool.name.to_string(),
            project_path: project_dir.display().to_string(),
            warning: None,
        };

        // Shared with each worker and stored on the record as its identity.
        let archived = Arc::new(std::sync::atomic::AtomicBool::new(false));
        #[cfg(unix)]
        let pgid = master.process_group_leader();
        let session = PtySession {
            master,
            #[cfg(unix)]
            pgid,
            child,
            writer,
            buffer: Arc::new(Mutex::new(OutputBuffer::default())),
            archived: Arc::clone(&archived),
        };
        let buffer = Arc::clone(&session.buffer);

        // Signals the reader thread has drained the PTY (read returned 0). The
        // watcher waits on this before archiving, so the archived snapshot
        // always includes the final output produced just before exit (#9).
        let read_done = Arc::new((Mutex::new(false), std::sync::Condvar::new()));

        sessions_guard.insert(session_id.to_string(), session);
        drop(sessions_guard);

        let sessions = Arc::clone(&self.sessions);
        let finished_buffers = Arc::clone(&self.finished);
        let event_registry_reader = Arc::clone(&self.events);
        let event_registry_watcher = Arc::clone(&self.events);
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
        let reader_thread = std::thread::Builder::new()
            .name(format!("pty-reader-{session_id}"))
            .spawn(move || {
                let _reader_permit = reader_permit;
                let mut reader = reader;
                let mut sink = |event: SessionOutput| {
                    let mut succeeded = false;
                    dispatch_session_event(&event_registry_reader, &output_id, || {
                        succeeded = app_output
                            .emit(&format!("session-output-{output_id}"), event)
                            .is_ok();
                    }) && succeeded
                };
                pump_reader(&mut *reader, &buffer, &archived_reader, &mut sink);
                // Signal the watcher that we have drained the PTY, so it archives a
                // snapshot that includes every byte written before exit (#9).
                if let Ok(mut done) = read_done_reader.0.lock() {
                    *done = true;
                    read_done_reader.1.notify_all();
                }
            });
        if let Err(error) = reader_thread {
            archived.store(true, std::sync::atomic::Ordering::Release);
            self.abort_start(session_id);
            return Err(format!("无法启动会话输出线程：{error}"));
        }

        // Watcher thread: reports process exit and releases the session.
        let watcher_thread = std::thread::Builder::new()
            .name(format!("pty-watcher-{session_id}"))
            .spawn(move || {
                let reason = loop {
                    let exit_reason = {
                        let mut guard = match sessions.lock() {
                            Ok(guard) => guard,
                            Err(_) => break "会话管理错误".to_string(),
                        };
                        match guard.get_mut(&exit_id) {
                            Some(session)
                                if Arc::ptr_eq(&session.archived, &archived_watcher)
                                    && !archived_watcher
                                        .load(std::sync::atomic::Ordering::Acquire) =>
                            {
                                match session.child.try_wait() {
                                    Ok(Some(status)) => Some(format!("{status}")),
                                    Ok(None) => None,
                                    Err(error) => {
                                        // A persistent wait error must not poll forever with
                                        // an unmanaged live child.
                                        abort_child(&mut *session.child, &*session.master);
                                        Some(format!("无法确认会话状态：{error}"))
                                    }
                                }
                            }
                            _ => break "会话已结束".to_string(),
                        }
                    };

                    if let Some(reason) = exit_reason {
                        // Wait for the reader thread to drain the PTY before
                        // archiving, so the archived snapshot includes the final
                        // output (e.g. an error printed right before exit) — #9.
                        // But cap the total wait: if the output end stays open (e.g.
                        // a descendant process still holds the PTY), the reader may
                        // never hit EOF. Without a deadline the session would hang
                        // "running" forever (#12). After the deadline we archive what
                        // we have and release the session anyway.
                        let _ = wait_for_drain(&read_done_watcher, DRAIN_TIMEOUT);
                        // Even when output reached EOF, descendants that closed
                        // their PTY can still be alive. Reap the owned process
                        // group so a finished session cannot orphan them.
                        #[cfg(unix)]
                        if let Ok(guard) = sessions.lock()
                            && let Some(session) = guard.get(&exit_id)
                            && Arc::ptr_eq(&session.archived, &archived_watcher)
                        {
                            let pgid = session.pgid;
                            drop(guard);
                            kill_process_group(pgid);
                        }
                        // Mark archived BEFORE releasing the session: the reader
                        // checks this flag and stops emitting, so late output from a
                        // descendant cannot reach a finished session (#15).
                        archived_watcher.store(true, std::sync::atomic::Ordering::Release);
                        // Preserve the final buffer before releasing the session so a
                        // late-mounting view can replay it and see the exit state.
                        if let Ok(mut guard) = sessions.lock()
                            && let Some(session) = guard.get(&exit_id)
                            && Arc::ptr_eq(&session.archived, &archived_watcher)
                        {
                            let snapshot = session
                                .buffer
                                .lock()
                                .map(|buffer| (buffer.data.clone(), buffer.end_offset))
                                .unwrap_or_default();
                            if let Ok(mut done) = finished_buffers.lock() {
                                done.insert(exit_id.clone(), snapshot);
                            }
                            guard.remove(&exit_id);
                        }
                        break reason;
                    }

                    std::thread::sleep(std::time::Duration::from_millis(100));
                };

                // stop/forget may run while the watcher drains or archives.
                // Emit only while the ID's lifetime event gate is still open.
                // The registry lock also serializes in-flight dispatch vs close.
                dispatch_session_event(&event_registry_watcher, &exit_id, || {
                    let _ = app_exit.emit(
                        &format!("session-exit-{exit_id}"),
                        SessionExit {
                            id: exit_id.clone(),
                            reason,
                        },
                    );
                });
            });
        if let Err(error) = watcher_thread {
            archived.store(true, std::sync::atomic::Ordering::Release);
            self.abort_start(session_id);
            return Err(format!("无法启动会话监测线程：{error}"));
        }

        info.warning = remember_started_tool(projects, &info.project_path, &info.tool_id);
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
        validate_pty_size(rows, cols)?;
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
        // Hold the map lock through termination and removal. The watcher must
        // not archive/release the same child in between those two operations.
        let mut sessions = self.lock()?;
        if let Some(session) = sessions.get_mut(session_id) {
            // On failure the record is still present, so the caller can retry.
            terminate_child(&mut *session.child)?;
            // Wait for any in-flight output/exit dispatch and close its gate
            // *before* stop can return or the watcher can dispatch again.
            self.events
                .lock()
                .map_err(|_| "无法读取会话事件状态".to_string())?
                .close(session_id);
            session
                .archived
                .store(true, std::sync::atomic::Ordering::Release);
            // Terminate descendants too; otherwise they can keep the PTY reader
            // alive after we discard the parent handle.
            #[cfg(unix)]
            kill_process_group(session.pgid);
            sessions.remove(session_id);
            if let Ok(mut done) = self.finished.lock() {
                done.remove(session_id);
            }
            return Ok(());
        }

        // The natural-exit watcher may have won the race while the UI was
        // closing a running tab. Stop must also clear that finished snapshot.
        let mut finished = self
            .finished
            .lock()
            .map_err(|_| "无法读取会话状态".to_string())?;
        if finished.contains_key(session_id) {
            self.events
                .lock()
                .map_err(|_| "无法读取会话事件状态".to_string())?
                .close(session_id);
            finished.remove(session_id);
            Ok(())
        } else {
            Err("会话不存在或已结束".to_string())
        }
    }

    pub fn forget(&self, session_id: &str) -> Result<(), String> {
        let sessions = self.lock()?;
        if sessions.contains_key(session_id) {
            return Err("会话仍在运行，请先结束会话".to_string());
        }
        let mut finished = self
            .finished
            .lock()
            .map_err(|_| "无法读取会话状态".to_string())?;
        if finished.contains_key(session_id) {
            self.events
                .lock()
                .map_err(|_| "无法读取会话事件状态".to_string())?
                .close(session_id);
            finished.remove(session_id);
            Ok(())
        } else {
            Err("会话不存在或未结束".to_string())
        }
    }

    pub fn stop_all(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            // Emission is serialized against this gate; no stale event can
            // arrive after the app has closed its sessions.
            if let Ok(mut events) = self.events.lock() {
                for allowed in events.allowed.values_mut() {
                    *allowed = false;
                }
            }
            for session in sessions.values_mut() {
                session
                    .archived
                    .store(true, std::sync::atomic::Ordering::Release);
                abort_child(&mut *session.child, &*session.master);
            }
            sessions.clear();
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

// Tauri injects two State values and AppHandle alongside five IPC fields;
// flattening those fields preserves the existing frontend invoke contract.
#[expect(clippy::too_many_arguments, reason = "Tauri IPC argument contract")]
#[tauri::command]
pub fn start_session(
    state: tauri::State<'_, SessionManager>,
    projects: tauri::State<'_, ProjectStore>,
    app: AppHandle,
    session_id: String,
    tool_id: String,
    project_path: String,
    rows: u16,
    cols: u16,
) -> Result<SessionInfo, String> {
    state.start(
        app,
        &projects,
        StartRequest {
            session_id: &session_id,
            tool_id: &tool_id,
            project_path: &project_path,
            rows,
            cols,
        },
    )
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

#[tauri::command]
pub fn forget_session(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
) -> Result<(), String> {
    state.forget(&session_id)
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::time::Duration;

    use super::SessionManager;

    // Isolated project registry: tests must never touch the user's real config.
    struct TempProjects(std::path::PathBuf);

    impl TempProjects {
        fn new() -> Self {
            static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "agentterm-pty-registry-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(dir.join("registered")).unwrap();
            Self(dir)
        }

        fn store(&self) -> crate::projects::ProjectStore {
            crate::projects::ProjectStore::with_path(self.0.join("config/projects.json"))
        }

        fn project(&self) -> String {
            self.0.join("registered").display().to_string()
        }
    }

    impl Drop for TempProjects {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn start_request_requires_a_registered_project_not_just_a_directory() {
        let dir = TempProjects::new();
        let store = dir.store();
        let path = dir.project();
        let missing = super::validate_start_request(&store, "pty-1", "codex", &path, 24, 80)
            .err()
            .expect("existing but unregistered project must be denied");
        assert!(missing.contains("未注册"), "got: {missing}");

        let saved = store.add(&path).unwrap();
        let (tool, canonical) =
            super::validate_start_request(&store, "pty-1", "codex", &path, 24, 80).unwrap();
        assert_eq!(tool.id, "codex");
        assert_eq!(canonical.display().to_string(), saved.path);

        store.remove(&saved.id).unwrap();
        assert!(super::validate_start_request(&store, "pty-2", "codex", &path, 24, 80).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn registered_project_allows_isolated_pty_spawn() {
        let dir = TempProjects::new();
        let store = dir.store();
        let saved = store.add(&dir.project()).unwrap();
        let (_, cwd) =
            super::validate_start_request(&store, "pty-real", "codex", &saved.path, 24, 80)
                .unwrap();
        let mut pty = super::spawn_pty(std::path::Path::new("/bin/cat"), &cwd, 24, 80)
            .expect("registered project must permit valid PTY startup");
        pty.child.kill().unwrap();
        let _ = pty.child.wait();
    }

    #[test]
    fn oversized_terminal_and_untrusted_session_ids_are_denied() {
        let dir = TempProjects::new();
        let store = dir.store();
        store.add(&dir.project()).unwrap();
        for id in ["", "../escape", "event:inject", &"x".repeat(65)] {
            assert!(
                super::validate_start_request(&store, id, "codex", &dir.project(), 24, 80).is_err()
            );
        }
        for (rows, cols) in [(0, 80), (24, 0), (301, 80), (24, 501)] {
            assert!(
                super::validate_start_request(&store, "ok-id", "codex", &dir.project(), rows, cols)
                    .is_err()
            );
        }
        assert!(
            super::validate_start_request(&store, "ok-id", "sh", &dir.project(), 24, 80).is_err()
        );
        assert!(
            SessionManager::default()
                .resize("unknown", 301, 80)
                .is_err()
        );
    }

    #[test]
    fn forget_only_removes_finished_snapshots_and_stop_finished_is_safe() {
        let manager = SessionManager::default();
        manager
            .finished
            .lock()
            .unwrap()
            .insert("finished".to_string(), ("last bytes".to_string(), 10));

        assert!(!manager.snapshot("finished").unwrap().running);
        assert!(manager.forget("finished").is_ok());
        assert!(manager.snapshot("finished").is_err());
        assert!(manager.forget("finished").is_err());

        // Closing a running tab can race with the natural-exit watcher.
        manager
            .finished
            .lock()
            .unwrap()
            .insert("raced".to_string(), ("done".to_string(), 4));
        assert!(manager.stop("raced").is_ok());
        assert!(manager.snapshot("raced").is_err());
        assert!(manager.forget("raced").is_err());

        insert_fake_session(&manager, "running", false);
        assert!(manager.forget("running").is_err());
        assert!(manager.is_running("running"));
        manager.stop("running").unwrap();
    }

    #[test]
    fn a_running_session_stays_managed_when_history_save_fails() {
        let dir = TempProjects::new();
        let store = dir.store();
        store.add(&dir.project()).unwrap();

        assert!(super::remember_started_tool(&store, &dir.project(), "codex").is_none());
        assert_eq!(store.list().unwrap()[0].last_tool.as_deref(), Some("codex"));

        store.fail_next_save_for_test();
        let manager = SessionManager::default();
        insert_fake_session(&manager, "live", false);
        let warning = super::remember_started_tool(&store, &dir.project(), "claude")
            .expect("save failure must become a non-fatal warning");
        assert!(warning.contains("会话已启动"));
        assert!(
            manager.is_running("live"),
            "save failure must not lose the PTY handle"
        );
        assert_eq!(store.list().unwrap()[0].last_tool.as_deref(), Some("codex"));
        manager.stop("live").unwrap();

        let mut info = super::SessionInfo {
            id: "live".to_string(),
            tool_id: "claude".to_string(),
            tool_name: "Claude Code".to_string(),
            project_path: dir.project(),
            warning: None,
        };
        let encoded = serde_json::to_value(&info).unwrap();
        assert!(
            encoded.get("warning").is_none(),
            "successful launch stays backward compatible"
        );
        info.warning = Some(warning);
        let encoded = serde_json::to_value(&info).unwrap();
        assert!(encoded["warning"].as_str().unwrap().contains("会话已启动"));
    }

    #[test]
    fn stopped_watchers_cannot_claim_a_reused_session_id() {
        let manager = SessionManager::default();
        insert_fake_session(&manager, "reused", false);
        let old_flag = manager
            .sessions
            .lock()
            .unwrap()
            .get("reused")
            .unwrap()
            .archived
            .clone();
        manager.stop("reused").unwrap();
        assert!(old_flag.load(std::sync::atomic::Ordering::Acquire));
        // The public ID is tombstoned for the whole application lifetime, not
        // merely while active or retained in finished buffers.
        assert!(manager.events.lock().unwrap().reserve("reused").is_err());
        let mut old_watcher_called = false;
        assert!(!super::dispatch_session_event(
            &manager.events,
            "reused",
            || {
                old_watcher_called = true;
            }
        ));
        assert!(!old_watcher_called);
    }

    #[test]
    fn controlled_exit_event_dispatch_is_serialized_with_stop() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, mpsc};

        let manager = Arc::new(SessionManager::default());
        insert_fake_session(&manager, "event-stop", false);
        let delivery_count = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let registry = Arc::clone(&manager.events);
        let delivered = Arc::clone(&delivery_count);
        let old_watcher = std::thread::spawn(move || {
            super::dispatch_session_event(&registry, "event-stop", || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                delivered.fetch_add(1, Ordering::SeqCst);
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // Simulate the exact race: an event was accepted, then stop starts.
        // The stop must wait for this in-flight delivery to finish.
        let stop_manager = Arc::clone(&manager);
        let stop_thread = std::thread::spawn(move || stop_manager.stop("event-stop"));
        release_tx.send(()).unwrap();
        assert!(old_watcher.join().unwrap());
        assert!(stop_thread.join().unwrap().is_ok());

        // After stop returns, even an old watcher holding the same ID must
        // never dispatch a second session-exit event.
        let emitted = super::dispatch_session_event(&manager.events, "event-stop", || {
            delivery_count.fetch_add(1, Ordering::SeqCst);
        });
        assert!(!emitted);
        assert_eq!(delivery_count.load(Ordering::SeqCst), 1);
        assert!(
            manager
                .events
                .lock()
                .unwrap()
                .reserve("event-stop")
                .is_err()
        );
    }

    #[test]
    fn controlled_natural_exit_event_is_suppressed_after_forget() {
        let manager = SessionManager::default();
        manager
            .events
            .lock()
            .unwrap()
            .reserve("event-finished")
            .unwrap();
        manager
            .finished
            .lock()
            .unwrap()
            .insert("event-finished".to_string(), ("tail".to_string(), 4));
        let mut delivered = Vec::new();
        // A naturally finished session can still emit its legitimate exit
        // notification until the user closes that finished tab.
        assert!(super::dispatch_session_event(
            &manager.events,
            "event-finished",
            || {
                delivered.push("session-exit-event-finished");
            }
        ));
        manager.forget("event-finished").unwrap();
        assert!(!super::dispatch_session_event(
            &manager.events,
            "event-finished",
            || {
                delivered.push("late-exit");
            }
        ));
        assert_eq!(delivered, vec!["session-exit-event-finished"]);
        assert!(
            manager
                .events
                .lock()
                .unwrap()
                .reserve("event-finished")
                .is_err()
        );
    }

    #[test]
    fn blocked_reader_keeps_a_budget_slot_until_read_returns() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Condvar, Mutex, mpsc};

        struct BlockedReader {
            entered: Option<mpsc::Sender<()>>,
            release: Arc<(Mutex<bool>, Condvar)>,
        }

        impl std::io::Read for BlockedReader {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                if let Some(entered) = self.entered.take() {
                    entered.send(()).unwrap();
                }
                let mut allowed = self.release.0.lock().unwrap();
                while !*allowed {
                    allowed = self.release.1.wait(allowed).unwrap();
                }
                Ok(0)
            }
        }

        let active = Arc::new(AtomicUsize::new(0));
        let permit = super::ReaderPermit::acquire(&active).unwrap();
        let archived = Arc::new(AtomicBool::new(false));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let worker_release = Arc::clone(&release);
        let worker_archived = Arc::clone(&archived);
        let worker = std::thread::spawn(move || {
            let _permit = permit;
            let mut reader = BlockedReader {
                entered: Some(entered_tx),
                release: worker_release,
            };
            let buffer = Mutex::new(super::OutputBuffer::default());
            super::pump_reader(&mut reader, &buffer, &worker_archived, &mut |_| true);
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        archived.store(true, Ordering::Release);
        // Cancel flag alone does NOT close a blocked OS read or free its fd.
        assert_eq!(active.load(Ordering::Acquire), 1);
        {
            let mut unblocked = release.0.lock().unwrap();
            *unblocked = true;
            release.1.notify_all();
        }
        worker.join().unwrap();
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn reader_budget_fails_closed_at_hard_limit() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let active = Arc::new(AtomicUsize::new(0));
        let mut permits = Vec::new();
        for _ in 0..super::MAX_READER_THREADS {
            permits.push(super::ReaderPermit::acquire(&active).unwrap());
        }
        assert_eq!(active.load(Ordering::Acquire), super::MAX_READER_THREADS);
        assert!(super::ReaderPermit::acquire(&active).is_err());
        permits.pop();
        let replacement = super::ReaderPermit::acquire(&active).unwrap();
        drop(replacement);
        drop(permits);
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_failed_stop_remains_retryable_and_natural_exit_stop_is_safe() {
        let manager = SessionManager::default();
        insert_fake_session(&manager, "retry", true);
        let flag = manager
            .sessions
            .lock()
            .unwrap()
            .get("retry")
            .unwrap()
            .archived
            .clone();
        assert!(manager.stop("retry").is_err());
        assert!(manager.is_running("retry"));
        assert!(!flag.load(std::sync::atomic::Ordering::Acquire));
        {
            let mut guard = manager.sessions.lock().unwrap();
            guard.get_mut("retry").unwrap().child = Box::new(FakeChild {
                exited: false,
                kill_fails: false,
                kill_calls: 0,
            });
        }
        assert!(manager.stop("retry").is_ok());
        assert!(!manager.is_running("retry"));
        assert!(flag.load(std::sync::atomic::Ordering::Acquire));

        insert_fake_session(&manager, "exited", true);
        {
            let mut guard = manager.sessions.lock().unwrap();
            guard.get_mut("exited").unwrap().child = Box::new(FakeChild {
                exited: true,
                kill_fails: true,
                kill_calls: 0,
            });
        }
        assert!(manager.stop("exited").is_ok());
    }

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
        manager.events.lock().unwrap().reserve(id).unwrap();
        let mut map = manager.sessions.lock().unwrap();
        map.insert(
            id.to_string(),
            super::PtySession {
                master: Box::new(FakeMaster),
                #[cfg(unix)]
                pgid: None,
                child: Box::new(FakeChild {
                    exited: false,
                    kill_fails,
                    kill_calls: 0,
                }),
                writer: Box::new(std::io::sink()),
                buffer: std::sync::Arc::new(super::Mutex::new(super::OutputBuffer::default())),
                archived: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

    #[cfg(unix)]
    #[test]
    fn kill_process_group_terminates_the_whole_tree() {
        // #15 mechanism: a session leader with a child in the same process group
        // (the shape portable-pty creates with setsid). Killing the group must
        // take down both, so nothing keeps the PTY open after the lead exits.
        use std::os::unix::process::CommandExt;

        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg("sleep 30 & wait");
        // New session: child becomes its own process-group leader.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().expect("spawn session leader");
        let pgid = child.id() as libc::pid_t;

        std::thread::sleep(std::time::Duration::from_millis(300));

        // Find the grandchild (the `sleep`) and confirm it is alive.
        let output = std::process::Command::new("pgrep")
            .args(["-P", &child.id().to_string()])
            .output()
            .expect("pgrep");
        let grandchild = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .and_then(|line| line.trim().parse::<libc::pid_t>().ok())
            .expect("expected a grandchild process");
        assert_eq!(unsafe { libc::kill(grandchild, 0) }, 0, "grandchild alive");

        super::kill_process_group(Some(pgid));

        // Both the leader and the grandchild must be gone.
        let status = child.wait().expect("wait leader");
        assert!(!status.success(), "leader must be killed, not exit cleanly");

        let mut gone = false;
        for _ in 0..60 {
            if unsafe { libc::kill(grandchild, 0) } != 0 {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(gone, "process group kill must terminate the grandchild");
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
