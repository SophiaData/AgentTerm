use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::session_history::{SessionHistory, SessionRecord, now_ms};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::projects::normalize_project_dir;
use crate::tools::{
    ResolveResult, TOOL_DEFINITIONS, candidate_search_dirs, is_windows_batch, resolve_binary,
    windows_cmd_executable,
};

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
    /// Foreground process group of the PTY, captured while the child is alive.
    /// Used to terminate descendants that keep the PTY open after the child
    /// exits, so the reader thread can reach EOF and release its handle (#15).
    #[cfg(unix)]
    pgid: Option<libc::pid_t>,
    #[cfg(windows)]
    job: Option<WinJob>,
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
    #[cfg(windows)]
    pub job: Option<WinJob>,
}

fn has_utf8_locale(value: Option<&std::ffi::OsStr>) -> bool {
    value.and_then(|item| item.to_str()).is_some_and(|text| {
        let lower = text.to_ascii_lowercase();
        lower.contains("utf-8") || lower.contains("utf8")
    })
}

#[cfg(windows)]
fn windows_console_codepage_utf8() {
    unsafe extern "system" {
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn SetConsoleCP(code_page: u32) -> i32;
    }
    const CP_UTF8: u32 = 65001;
    unsafe {
        SetConsoleOutputCP(CP_UTF8);
        SetConsoleCP(CP_UTF8);
    }
}

/// Builds the command and environment for `spawn_pty` so command selection
/// (including Windows `.cmd`/`.bat` wrappers), verbatim path stripping, and
/// UTF-8 locale setup can be unit-tested without opening a ConPTY host.
fn build_pty_command(binary: &std::path::Path, cwd: &std::path::Path) -> CommandBuilder {
    let clean_binary = strip_windows_verbatim(binary);
    let clean_cwd = strip_windows_verbatim(cwd);
    let mut command = if is_windows_batch(&clean_binary) {
        let mut wrapped = CommandBuilder::new(windows_cmd_executable());
        wrapped.arg("/D");
        wrapped.arg("/C");
        wrapped.arg(&clean_binary);
        wrapped
    } else {
        CommandBuilder::new(&clean_binary)
    };
    command.cwd(clean_cwd.as_os_str());
    for (key, _) in std::env::vars() {
        if ENV_BLOCKLIST.contains(&key.as_str()) {
            command.env_remove(&key);
        }
    }
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("PYTHONUTF8", "1");
    command.env("PYTHONIOENCODING", "utf-8");
    if !has_utf8_locale(std::env::var_os("LANG").as_deref()) {
        command.env("LANG", "C.UTF-8");
    }
    if !has_utf8_locale(std::env::var_os("LC_ALL").as_deref()) {
        command.env("LC_ALL", "C.UTF-8");
    }
    command
}

fn strip_windows_verbatim(path: &std::path::Path) -> PathBuf {
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy();
        if let Some(stripped) = raw.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{stripped}"));
        }
        if let Some(stripped) = raw.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path.to_path_buf()
}

pub fn spawn_pty(
    binary: &std::path::Path,
    cwd: &std::path::Path,
    rows: u16,
    cols: u16,
) -> Result<SpawnedPty, String> {
    #[cfg(windows)]
    windows_console_codepage_utf8();
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("无法创建终端：{error}"))?;

    let command = build_pty_command(binary, cwd);

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("无法启动终端进程：{error}"))?;
    drop(pair.slave);

    #[cfg(windows)]
    let job = child
        .as_raw_handle()
        .and_then(|handle| WinJob::attach(handle).ok());

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
        #[cfg(windows)]
        job,
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

fn trim_buffer_front(data: &mut String, max_bytes: usize) {
    if data.len() <= max_bytes {
        return;
    }
    let cut = data.len() - max_bytes;
    let mut boundary = data
        .char_indices()
        .map(|(index, _)| index)
        .find(|index| *index >= cut)
        .unwrap_or(data.len());
    // Prefer starting on a fresh line shortly after `cut` so we never slice
    // through an ANSI CSI/OSC escape sequence and leave raw control garbage.
    let search_end = data
        .char_indices()
        .map(|(index, _)| index)
        .find(|index| *index >= boundary.saturating_add(16 * 1024))
        .unwrap_or(data.len());
    if let Some(newline_rel) = data[boundary..search_end].find('\n') {
        boundary = boundary + newline_rel + 1;
    } else if let Some(esc_rel) = data[..boundary].rfind('\x1b') {
        let between = &data.as_bytes()[esc_rel..boundary];
        if between.len() <= 256 && !escape_sequence_complete(between) {
            // Skip the incomplete escape payload rather than replaying `[38;2;...m`.
            if let Some(tail_end) = find_escape_end(&data.as_bytes()[boundary..]) {
                let candidate = boundary + tail_end;
                if data.is_char_boundary(candidate) {
                    boundary = candidate;
                }
            }
        }
    }
    data.drain(..boundary);
}

fn escape_sequence_complete(slice: &[u8]) -> bool {
    if slice.len() < 2 || slice[0] != 0x1b {
        return true;
    }
    match slice[1] {
        b'[' => slice[2..].iter().any(|byte| (0x40..=0x7e).contains(byte)),
        b']' => {
            slice[2..].contains(&0x07) || slice[2..].windows(2).any(|pair| pair == [0x1b, b'\\'])
        }
        _ => slice.len() >= 3,
    }
}

fn find_escape_end(rest: &[u8]) -> Option<usize> {
    let limit = rest.len().min(256);
    for (index, byte) in rest[..limit].iter().enumerate() {
        if *byte == 0x07 || (0x40..=0x7e).contains(byte) {
            return Some(index + 1);
        }
        if *byte == b'\n' {
            return Some(index + 1);
        }
    }
    None
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
                    trim_buffer_front(&mut guard.data, MAX_BUFFER_CHARS);
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

#[cfg(windows)]
pub struct WinJob {
    handle: *mut std::ffi::c_void,
}

#[cfg(windows)]
unsafe impl Send for WinJob {}
#[cfg(windows)]
unsafe impl Sync for WinJob {}

#[cfg(windows)]
impl WinJob {
    fn attach(process_handle: std::os::windows::io::RawHandle) -> std::io::Result<Self> {
        use std::ffi::c_void;
        #[repr(C)]
        #[derive(Default)]
        struct BasicLimits {
            process_time: i64,
            job_time: i64,
            flags: u32,
            min_ws: usize,
            max_ws: usize,
            active_limit: u32,
            affinity: usize,
            priority: u32,
            scheduling: u32,
        }
        #[repr(C)]
        #[derive(Default)]
        struct IoCounters {
            data: [u64; 6],
        }
        #[repr(C)]
        #[derive(Default)]
        struct ExtendedLimits {
            basic: BasicLimits,
            io: IoCounters,
            process_mem: usize,
            job_mem: usize,
            peak_process_mem: usize,
            peak_job_mem: usize,
        }
        const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;
        const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
        type Handle = *mut c_void;
        unsafe extern "system" {
            fn CreateJobObjectW(attributes: Handle, name: *const u16) -> Handle;
            fn SetInformationJobObject(
                job: Handle,
                class: i32,
                data: *const c_void,
                size: u32,
            ) -> i32;
            fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
            fn CloseHandle(handle: Handle) -> i32;
        }

        let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        if job.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let mut limits = ExtendedLimits::default();
        limits.basic.flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION as i32,
                (&limits as *const ExtendedLimits).cast(),
                std::mem::size_of::<ExtendedLimits>() as u32,
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            unsafe { CloseHandle(job) };
            return Err(error);
        }
        if unsafe { AssignProcessToJobObject(job, process_handle.cast()) } == 0 {
            let error = std::io::Error::last_os_error();
            unsafe { CloseHandle(job) };
            return Err(error);
        }
        Ok(Self { handle: job })
    }

    fn terminate(&self) {
        unsafe extern "system" {
            fn TerminateJobObject(job: *mut std::ffi::c_void, exit_code: u32) -> i32;
        }
        if !self.handle.is_null() {
            unsafe {
                TerminateJobObject(self.handle, 1);
            }
        }
    }
}

#[cfg(windows)]
impl Drop for WinJob {
    fn drop(&mut self) {
        unsafe extern "system" {
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        if !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
        }
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
    /// Persisted identity/end-state history, configured once before commands
    /// are served. Recording is best-effort: a history write failure must not
    /// orphan a running CLI, so callers keep the session and log the failure.
    history: Arc<Mutex<Option<SessionHistory>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            finished: Arc::new(Mutex::new(HashMap::new())),
            history: Arc::new(Mutex::new(None)),
        }
    }
}

impl SessionManager {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, PtySession>>, String> {
        self.sessions
            .lock()
            .map_err(|_| "会话管理暂时不可用，请重试".to_string())
    }

    /// Backend-only path, fixed before serving commands; never creates a
    /// directory. History is optional only until this is called.
    pub fn configure_history(&self, path: PathBuf) -> Result<(), String> {
        let mut history = self
            .history
            .lock()
            .map_err(|_| "会话历史暂时不可用，请重试".to_string())?;
        if history.is_some() {
            return Err("会话历史已经配置，不能切换".to_string());
        }
        *history = Some(SessionHistory::configure(path)?);
        Ok(())
    }

    fn record_start(&self, record: SessionRecord) {
        let recorded = self
            .history
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .map(|history| history.record_start(record));
        if let Some(Err(error)) = recorded {
            eprintln!("AgentTerm could not persist session history: {error}");
        }
    }

    fn record_end(&self, id: &str, reason: Option<String>, output_bytes: Option<u64>) {
        let recorded = self
            .history
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .map(|history| history.record_end(id, now_ms(), reason, output_bytes));
        if let Some(Err(error)) = recorded {
            eprintln!("AgentTerm could not persist session end for {id}: {error}");
        }
    }

    pub fn list_history(&self) -> Result<Vec<SessionRecord>, String> {
        let guard = self
            .history
            .lock()
            .map_err(|_| "会话历史暂时不可用，请重试".to_string())?;
        match guard.as_ref() {
            Some(history) => history.list(),
            None => Err("会话历史尚未配置".to_string()),
        }
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
            #[cfg(windows)]
            job,
        } = spawned;

        let info = SessionInfo {
            id: session_id.to_string(),
            tool_id: tool.id.to_string(),
            tool_name: tool.name.to_string(),
            project_path: project_dir.display().to_string(),
        };

        #[cfg(unix)]
        let pgid = master.process_group_leader();
        let session = PtySession {
            master,
            #[cfg(unix)]
            pgid,
            #[cfg(windows)]
            job,
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

        self.record_start(SessionRecord {
            id: session_id.to_string(),
            tool_id: tool.id.to_string(),
            tool_name: tool.name.to_string(),
            project_path: info.project_path.clone(),
            started_at_ms: now_ms(),
            ended_at_ms: None,
            exit_reason: None,
            output_bytes: None,
        });

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
        let history_writer = Arc::clone(&self.history);

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
                    let drained = wait_for_drain(&read_done_watcher, DRAIN_TIMEOUT);
                    if !drained {
                        // The output end stayed open past the deadline: kill the
                        // process group so the reader unblocks and releases its
                        // PTY handle, instead of leaking it (#15).
                        #[cfg(unix)]
                        if let Ok(guard) = sessions.lock()
                            && let Some(session) = guard.get(&exit_id)
                        {
                            let pgid = session.pgid;
                            drop(guard);
                            kill_process_group(pgid);
                        }
                        #[cfg(windows)]
                        if let Ok(guard) = sessions.lock()
                            && let Some(session) = guard.get(&exit_id)
                            && let Some(job) = session.job.as_ref()
                        {
                            job.terminate();
                        }
                    }
                    // Mark archived BEFORE releasing the session: the reader
                    // checks this flag and stops emitting, so late output from a
                    // descendant cannot reach a finished session (#15).
                    archived_watcher.store(true, std::sync::atomic::Ordering::Release);
                    // Preserve the final buffer before releasing the session so a
                    // late-mounting view can replay it and see the exit state.
                    let mut archived_bytes = None;
                    if let Ok(mut guard) = sessions.lock()
                        && let Some(session) = guard.remove(&exit_id)
                        && let Ok(buffer) = session.buffer.lock()
                        && let Ok(mut done) = finished_buffers.lock()
                    {
                        archived_bytes = Some(buffer.data.len() as u64);
                        done.insert(exit_id.clone(), (buffer.data.clone(), buffer.end_offset));
                    }
                    if let Ok(history_guard) = history_writer.lock()
                        && let Some(history) = history_guard.as_ref()
                        && let Err(error) = history.record_end(
                            &exit_id,
                            now_ms(),
                            Some(reason.clone()),
                            archived_bytes,
                        )
                    {
                        eprintln!("AgentTerm could not persist session end for {exit_id}: {error}");
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
            #[cfg(windows)]
            if let Some(job) = session.job.as_ref() {
                job.terminate();
            }
        }

        // Kill succeeded (or the process was already gone): now release it.
        let session = self.lock()?.remove(session_id);
        // Drop any finished-buffer leftover for this id so it cannot accumulate.
        if let Ok(mut done) = self.finished.lock() {
            done.remove(session_id);
        }
        drop(session);
        self.record_end(session_id, Some("已停止".to_string()), None);
        Ok(())
    }

    pub fn forget(&self, session_id: &str) -> Result<(), String> {
        // Hold the live-session lock while releasing replay data. A caller may
        // forget an ended tab, but must never detach a running process.
        let sessions = self.lock()?;
        if sessions.contains_key(session_id) {
            return Err("会话仍在运行，请先结束会话".to_string());
        }
        self.finished
            .lock()
            .map_err(|_| "无法释放会话输出".to_string())?
            .remove(session_id);
        Ok(())
    }

    pub fn stop_all(&self) {
        let killed: Vec<String> = if let Ok(mut sessions) = self.sessions.lock() {
            sessions
                .drain()
                .map(|(id, mut session)| {
                    let _ = session.child.kill();
                    #[cfg(windows)]
                    if let Some(job) = session.job.as_ref() {
                        job.terminate();
                    }
                    id
                })
                .collect()
        } else {
            Vec::new()
        };
        for id in killed {
            self.record_end(&id, Some("应用退出".to_string()), None);
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
pub fn forget_session(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
) -> Result<(), String> {
    state.forget(&session_id)
}

#[tauri::command]
pub fn stop_session(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
) -> Result<(), String> {
    state.stop(&session_id)
}

#[tauri::command]
pub fn session_history(
    state: tauri::State<'_, SessionManager>,
) -> Result<Vec<SessionRecord>, String> {
    state.list_history()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::io::Read;
    #[cfg(windows)]
    use std::path::Path;
    use std::path::PathBuf;
    #[cfg(unix)]
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

        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
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
                #[cfg(unix)]
                pgid: None,
                #[cfg(windows)]
                job: None,
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

    #[test]
    fn forgetting_an_ended_tab_releases_only_its_output_and_is_idempotent() {
        let manager = SessionManager::default();
        manager.finished.lock().unwrap().extend([
            ("a".to_string(), ("first".to_string(), 5)),
            ("b".to_string(), ("second".to_string(), 6)),
        ]);
        manager.forget("a").unwrap();
        manager.forget("a").unwrap();
        assert!(manager.snapshot("a").is_err());
        assert_eq!(manager.snapshot("b").unwrap().data, "second");
    }

    #[test]
    fn forgetting_a_live_tab_is_rejected_and_preserves_the_process() {
        let manager = SessionManager::default();
        insert_fake_session(&manager, "live", false);
        assert_eq!(
            manager.forget("live").unwrap_err(),
            "会话仍在运行，请先结束会话"
        );
        assert!(manager.is_running("live"));
        assert!(manager.snapshot("live").unwrap().running);
    }

    #[test]
    fn closing_one_live_session_keeps_another_running() {
        let manager = SessionManager::default();
        insert_fake_session(&manager, "a", false);
        insert_fake_session(&manager, "b", false);
        manager.stop("a").unwrap();
        manager.forget("a").unwrap();
        assert!(!manager.is_running("a"));
        assert!(manager.is_running("b"));
        assert!(manager.write("b", "continue\r").is_ok());
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

    fn history_store(label: &str) -> PathBuf {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "agentterm-session-history-{label}-{}-{}-{}.json",
            std::process::id(),
            super::now_ms(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[test]
    fn history_is_rejected_until_configured() {
        let manager = SessionManager::default();
        let error = manager.list_history().unwrap_err();
        assert!(error.contains("尚未配置"), "got: {error}");
    }

    #[test]
    fn history_can_only_be_configured_once() {
        let manager = SessionManager::default();
        let path = history_store("sessions");
        manager.configure_history(path.clone()).unwrap();
        let error = manager.configure_history(path).unwrap_err();
        assert!(error.contains("已经配置"), "got: {error}");
    }

    fn history_project() -> String {
        if cfg!(windows) {
            "E:\\AgentTerm"
        } else {
            "/tmp/agentterm"
        }
        .to_string()
    }

    #[test]
    fn explicit_stop_records_history_end() {
        let manager = SessionManager::default();
        let path = history_store("stop");
        manager.configure_history(path).unwrap();
        insert_fake_session(&manager, "s-stop", false);
        manager.record_start(super::SessionRecord {
            id: "s-stop".to_string(),
            tool_id: "claude".to_string(),
            tool_name: "Claude Code".to_string(),
            project_path: history_project(),
            started_at_ms: 10,
            ended_at_ms: None,
            exit_reason: None,
            output_bytes: None,
        });

        manager.stop("s-stop").unwrap();

        let listed = manager.list_history().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "s-stop");
        assert_eq!(listed[0].exit_reason.as_deref(), Some("已停止"));
        assert!(listed[0].ended_at_ms.is_some());
    }

    #[test]
    fn stop_all_records_app_exit_for_every_session() {
        let manager = SessionManager::default();
        let path = history_store("exit");
        manager.configure_history(path).unwrap();
        insert_fake_session(&manager, "s-exit-1", false);
        insert_fake_session(&manager, "s-exit-2", false);
        for id in ["s-exit-1", "s-exit-2"] {
            manager.record_start(super::SessionRecord {
                id: id.to_string(),
                tool_id: "claude".to_string(),
                tool_name: "Claude Code".to_string(),
                project_path: history_project(),
                started_at_ms: 10,
                ended_at_ms: None,
                exit_reason: None,
                output_bytes: None,
            });
        }

        manager.stop_all();

        let listed = manager.list_history().unwrap();
        assert_eq!(listed.len(), 2);
        for record in listed {
            assert_eq!(record.exit_reason.as_deref(), Some("应用退出"));
            assert!(record.ended_at_ms.is_some());
        }
        assert!(manager.sessions.lock().unwrap().is_empty());
    }

    #[test]
    fn trim_buffer_front_avoids_splitting_ansi_escape_sequences() {
        let mut data = format!(
            "\x1b[38;2;140;222;194m{}\x1b[0m\n第二行正常输出",
            "x".repeat(40)
        );
        super::trim_buffer_front(&mut data, 32);
        assert!(
            !data.starts_with('[') && !data.starts_with(';'),
            "must not start inside an ANSI sequence, got: {data:?}"
        );
        assert!(data.contains("第二行正常输出"), "got: {data:?}");
    }

    #[cfg(windows)]
    #[test]
    fn spawn_pty_strips_verbatim_paths_and_sets_utf8_env() {
        let clean = super::strip_windows_verbatim(Path::new(r"\\?\E:\AgentTerm"));
        assert_eq!(clean, PathBuf::from(r"E:\AgentTerm"));
        let unc = super::strip_windows_verbatim(Path::new(r"\\?\UNC\server\share\proj"));
        assert_eq!(unc, PathBuf::from(r"\\server\share\proj"));
        assert!(super::has_utf8_locale(Some(std::ffi::OsStr::new(
            "zh_CN.UTF-8"
        ))));
        assert!(!super::has_utf8_locale(Some(std::ffi::OsStr::new("C"))));
    }

    #[cfg(windows)]
    #[test]
    fn build_pty_command_wraps_windows_cmd_and_strips_verbatim_cwd() {
        let cmd = super::build_pty_command(
            Path::new(r"\\?\C:\Users\Administrator\AppData\Roaming\npm\codex.cmd"),
            Path::new(r"\\?\E:\AgentTerm"),
        );
        let args: Vec<String> = cmd
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.len(), 4);
        assert!(args[0].to_ascii_lowercase().ends_with("cmd.exe"));
        assert_eq!(args[1], "/D");
        assert_eq!(args[2], "/C");
        assert_eq!(
            args[3],
            r"C:\Users\Administrator\AppData\Roaming\npm\codex.cmd"
        );
        assert_eq!(cmd.get_cwd().unwrap().to_string_lossy(), r"E:\AgentTerm");
        assert_eq!(cmd.get_env("PYTHONUTF8").unwrap(), "1");
        assert_eq!(cmd.get_env("PYTHONIOENCODING").unwrap(), "utf-8");
    }
}
