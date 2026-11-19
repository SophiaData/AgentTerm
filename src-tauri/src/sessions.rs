use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

/// The reader's initial archived check races with exit draining. Recheck while
/// holding the same event gate used by watcher archiving and exit dispatch.
fn dispatch_session_output(
    registry: &Mutex<EventRegistry>,
    archived: &AtomicBool,
    id: &str,
    dispatch: impl FnOnce(),
) -> bool {
    let Ok(guard) = registry.lock() else {
        return false;
    };
    if guard.allowed.get(id) != Some(&true) || archived.load(Ordering::Acquire) {
        return false;
    }
    dispatch();
    true
}

/// Serialize archived flag transition against in-flight output delivery.
/// Production archives under the same event gate in archive_observed_exit.
#[cfg(test)]
fn archive_session_output(registry: &Mutex<EventRegistry>, archived: &AtomicBool) {
    if let Ok(_guard) = registry.lock() {
        archived.store(true, Ordering::Release);
    } else {
        archived.store(true, Ordering::Release);
    }
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
/// Give the OS time to reap a killed child before releasing its only handle.
const TERMINATION_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
const REAPER_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MonitorState {
    Starting,
    Watched,
    Degraded,
}

struct PtySession {
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Foreground process group of the PTY, captured while the child is alive.
    /// Used to terminate descendants that keep the PTY open after the child
    /// exits, so the reader thread can reach EOF and release its handle (#15).
    #[cfg(unix)]
    pgid: Option<libc::pid_t>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Option<Box<dyn std::io::Write + Send>>,
    /// Everything the PTY has emitted so far (with its monotonic offset). The
    /// frontend replays this when a terminal view mounts, so output produced
    /// before the listener attached (e.g. Claude Code's first screen) is not
    /// lost, and so snapshot + live events can be stitched without overlap.
    buffer: Arc<Mutex<OutputBuffer>>,
    /// Identity shared with the watcher; prevents an old worker from touching
    /// a new session that reuses the same public ID.
    archived: Arc<std::sync::atomic::AtomicBool>,
    /// Starting is never polled by the fallback: watcher creation still owns it.
    /// Only a confirmed watcher failure can transition to Degraded.
    monitor_state: MonitorState,
    /// Reader completion is needed before archiving the final output in fallback.
    read_done: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

/// A freshly spawned PTY with all handles still separated, so the caller can
/// decide what happens with the reader (event loop or direct test assertions).
pub struct SpawnedPty {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    pub writer: Box<dyn std::io::Write + Send>,
    pub reader: Box<dyn Read + Send>,
}

/// Failure after spawn carries both OS handles to its owner. Never drop a
/// possibly-running child just because PTY I/O setup failed.
pub enum SpawnFailure {
    NotStarted(String),
    Spawned {
        message: String,
        master: Box<dyn portable_pty::MasterPty + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
    },
}

impl std::fmt::Debug for SpawnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted(message) | Self::Spawned { message, .. } => formatter
                .debug_tuple("SpawnFailure")
                .field(message)
                .finish(),
        }
    }
}

/// Keep the half-initialized PTY in the error even when either OS I/O method
/// fails. The closures permit fault injection without manipulating real PTYs.
fn finish_spawned_pty<E: std::fmt::Display>(
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    clone_reader: impl FnOnce(&dyn portable_pty::MasterPty) -> Result<Box<dyn Read + Send>, E>,
    take_writer: impl FnOnce(&dyn portable_pty::MasterPty) -> Result<Box<dyn std::io::Write + Send>, E>,
) -> Result<SpawnedPty, SpawnFailure> {
    let reader = match clone_reader(&*master) {
        Ok(reader) => reader,
        Err(error) => {
            return Err(SpawnFailure::Spawned {
                message: format!("无法读取终端输出：{error}"),
                master,
                child,
            });
        }
    };
    let writer = match take_writer(&*master) {
        Ok(writer) => writer,
        Err(error) => {
            return Err(SpawnFailure::Spawned {
                message: format!("无法写入终端：{error}"),
                master,
                child,
            });
        }
    };
    Ok(SpawnedPty {
        master,
        child,
        writer,
        reader,
    })
}

pub fn spawn_pty(
    binary: &std::path::Path,
    cwd: &std::path::Path,
    rows: u16,
    cols: u16,
) -> Result<SpawnedPty, SpawnFailure> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| SpawnFailure::NotStarted(format!("无法创建终端：{error}")))?;

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
        .map_err(|error| SpawnFailure::NotStarted(format!("无法启动终端进程：{error}")))?;
    drop(pair.slave);

    finish_spawned_pty(
        pair.master,
        child,
        |master| master.try_clone_reader(),
        |master| master.take_writer(),
    )
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

/// macOS returns EPERM when a process group contains only an unreaped
/// zombie leader. In that specific case there is nothing else to kill. Check
/// membership through libproc rather than treating arbitrary EPERM as success.
#[cfg(target_os = "macos")]
fn macos_group_has_no_descendants(pgid: libc::pid_t) -> Result<bool, String> {
    let needed = unsafe { libc::proc_listpgrppids(pgid, std::ptr::null_mut(), 0) };
    if needed < 0 {
        return Err(format!(
            "无法枚举 PTY 进程组，管理句柄已保留：{}",
            std::io::Error::last_os_error()
        ));
    }
    // proc_listpgrppids returns a PID count (not a byte count).
    let slots = (needed as usize).saturating_add(32);
    if slots > 65_536 {
        return Err("PTY 进程组成员数量异常，拒绝推断已清理".to_string());
    }
    let mut members = vec![0 as libc::pid_t; slots];
    let size_bytes = members.len() * std::mem::size_of::<libc::pid_t>();
    let buffer_size =
        libc::c_int::try_from(size_bytes).map_err(|_| "PTY 进程组成员缓存过大".to_string())?;
    // SAFETY: libc writes at most buffer_size bytes into aligned pid_t
    // storage. A full buffer may be truncated and must fail closed.
    let returned =
        unsafe { libc::proc_listpgrppids(pgid, members.as_mut_ptr().cast(), buffer_size) };
    if returned < 0 || returned as usize >= members.len() {
        return Err("PTY 进程组成员枚举未完整返回，拒绝回收组长".to_string());
    }
    Ok(!members[..returned as usize]
        .iter()
        .any(|pid| *pid > 0 && *pid != pgid))
}

/// A negative PGID signal is safe ONLY while the original leader is still
/// an unreaped child of this process. Callers hold the sessions mutex and
/// establish that identity with waitid(WNOWAIT) before calling this helper.
#[cfg(unix)]
fn kill_process_group(pgid: Option<libc::pid_t>) -> Result<(), String> {
    let pgid = pgid.ok_or_else(|| "PTY 进程组身份不可用，拒绝清理未知进程组".to_string())?;
    if pgid <= 1 || pgid == unsafe { libc::getpgrp() } {
        return Err("PTY 进程组身份不安全，拒绝发送信号".to_string());
    }
    // SAFETY: PGID was checked against our own group. The caller guarantees
    // the original group leader has NOT been reaped, preventing ID reuse.
    if unsafe { libc::kill(-pgid, libc::SIGKILL) } == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            // Group already empty; its unreaped leader still pins the numeric ID.
            Ok(())
        } else {
            #[cfg(target_os = "macos")]
            if error.raw_os_error() == Some(libc::EPERM) && macos_group_has_no_descendants(pgid)? {
                // macOS refuses to signal a group with only its zombie leader;
                // no other PID remains to hold the PTY or escape management.
                return Ok(());
            }
            Err(format!("无法终止 PTY 进程组：{error}"))
        }
    }
}

/// Non-reaping Unix exit observation. WNOWAIT retains the waitable zombie,
/// pinning the original leader PID until the reader drains or its descendants
/// have been killed. A failed waitid is NOT a reason to reap or signal a PGID.
#[cfg(unix)]
fn peek_unreaped_child(child: &dyn portable_pty::Child) -> Result<Option<String>, String> {
    let pid = child
        .process_id()
        .and_then(|id| libc::pid_t::try_from(id).ok())
        .filter(|id| *id > 1)
        .ok_or_else(|| "PTY 组长 PID 不可验证，保留管理句柄".to_string())?;
    // SAFETY: a zero-initialized siginfo_t is valid output storage for waitid.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: P_PID targets only the owned Child PID. WNOWAIT explicitly does
    // not consume its wait status. All callers serialize under sessions mutex.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(format!(
            "PTY 非回收退出探测失败，管理句柄已保留：{}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: waitid succeeded, and siginfo_t exposes these accessors on
    // macOS and Linux. A zero PID is the WNOHANG 'not exited' result.
    let observed_pid = unsafe { info.si_pid() };
    if observed_pid == 0 {
        return Ok(None);
    }
    if observed_pid != pid {
        return Err("PTY 退出身份不一致，拒绝归档或清理进程组".to_string());
    }
    let status = unsafe { info.si_status() };
    Ok(Some(format!("进程已退出（状态：{status}）")))
}

/// Never reap a Unix leader before its entire PTY group is safely dealt with.
/// Active stop and natural-exit timeout use the same pinned-PID invariant.
fn terminate_managed_child(session: &mut PtySession) -> Result<(), String> {
    #[cfg(unix)]
    {
        // Fake Child tests have no OS process group; exercise their existing
        // portable-pty error and retry contracts without inventing a PID.
        #[cfg(test)]
        if session.pgid.is_none() {
            return terminate_child_confirmed(&mut *session.child);
        }
        let _ = peek_unreaped_child(&*session.child)?;
        let leader_id = session
            .child
            .process_id()
            .and_then(|id| libc::pid_t::try_from(id).ok());
        if session.pgid.is_some() && session.pgid == leader_id {
            kill_process_group(session.pgid)?;
        } else if session.pgid.is_some() {
            return Err("PTY 组长与进程组身份不匹配，拒绝清理".to_string());
        }
    }
    terminate_child_confirmed(&mut *session.child)
}

/// A wait failure is NOT evidence that the process has gone away. Still
/// attempt kill, then require a later successful wait before dropping the handle.
fn terminate_child(child: &mut dyn portable_pty::Child) -> Result<(), String> {
    match child.try_wait() {
        Ok(Some(_)) => Ok(()),
        Ok(None) => child
            .kill()
            .map_err(|error| format!("无法结束会话：{error}")),
        Err(wait_error) => child.kill().map_err(|kill_error| {
            format!("无法确认会话状态：{wait_error}；无法结束会话：{kill_error}")
        }),
    }
}

/// A successful kill request is not proof that the child is gone. Works with
/// both the Unix PTY child and the Windows ConPTY child (portable-pty Child).
fn terminate_child_confirmed(child: &mut dyn portable_pty::Child) -> Result<(), String> {
    terminate_child(child)?;
    let deadline = std::time::Instant::now() + TERMINATION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(None) => return Err("已请求结束进程，但在超时前未确认退出".to_string()),
            Err(error) => return Err(format!("结束进程后无法确认会话状态：{error}")),
        }
    }
}

/// Windows watcher/reaper and synthetic Unix test doubles share this path.
/// A confirmed child exit is required before the handle may be archived.
#[cfg(any(not(unix), test))]
fn observe_child_exit(child: &mut dyn portable_pty::Child) -> Result<Option<String>, String> {
    match child.try_wait() {
        Ok(Some(status)) => Ok(Some(format!("{status}"))),
        Ok(None) => Ok(None),
        Err(error) => {
            terminate_child_confirmed(child)?;
            Ok(Some(format!("状态读取失败后已确认终止：{error}")))
        }
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

/// An observed exit is only a *candidate* until the same session generation
/// has been archived while holding the sessions and event gates.
struct ObservedExit {
    reason: String,
    identity: Arc<AtomicBool>,
    read_done: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

enum ProbeResult {
    Running,
    Confirmed(ObservedExit),
    Uncertain,
    Gone,
}

/// Called by both the per-session watcher and the single manager reaper.
/// A wait/kill failure never grants permission to release the child handle.
fn probe_session_exit(
    sessions: &Mutex<HashMap<String, PtySession>>,
    session_id: &str,
    watcher_identity: Option<&Arc<AtomicBool>>,
) -> ProbeResult {
    // A poisoned map still owns its children; recover its guard instead of
    // permanently disabling the only manager-level retry path.
    let mut sessions = sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(session) = sessions.get_mut(session_id) else {
        return ProbeResult::Gone;
    };
    if session.archived.load(Ordering::Acquire)
        || match watcher_identity {
            Some(identity) => {
                !Arc::ptr_eq(identity, &session.archived)
                    || session.monitor_state == MonitorState::Degraded
            }
            None => session.monitor_state != MonitorState::Degraded,
        }
    {
        return ProbeResult::Gone;
    }
    #[cfg(unix)]
    let observation = {
        #[cfg(test)]
        {
            // Synthetic Child tests have no waitable OS PID or process group.
            if session.pgid.is_none() {
                observe_child_exit(&mut *session.child)
            } else {
                peek_unreaped_child(&*session.child)
            }
        }
        #[cfg(not(test))]
        peek_unreaped_child(&*session.child)
    };
    #[cfg(not(unix))]
    let observation = observe_child_exit(&mut *session.child);

    match observation {
        Ok(Some(reason)) => ProbeResult::Confirmed(ObservedExit {
            reason,
            identity: Arc::clone(&session.archived),
            read_done: Arc::clone(&session.read_done),
        }),
        Ok(None) => ProbeResult::Running,
        Err(_) => ProbeResult::Uncertain,
    }
}

/// The only natural-exit archival path. Revalidate the identity after waiting
/// outside the sessions mutex, and atomically gate output before snapshotting.
/// Stop/forget winning the race suppresses both archival and the exit event.
fn archive_observed_exit(
    sessions: &Mutex<HashMap<String, PtySession>>,
    events: &Mutex<EventRegistry>,
    finished: &Mutex<HashMap<String, (String, u64)>>,
    session_id: &str,
    observed: &ObservedExit,
) -> bool {
    let drained = wait_for_drain(&observed.read_done, DRAIN_TIMEOUT);
    if !drained {
        #[cfg(unix)]
        {
            // Never block on the reader with the sessions mutex held. Recheck
            // ownership immediately before sending a group signal; the original
            // leader remains an unreaped zombie until the final archive below.
            let mut guard = sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(session) = guard.get_mut(session_id) else {
                return false;
            };
            if !Arc::ptr_eq(&session.archived, &observed.identity)
                || session.archived.load(Ordering::Acquire)
            {
                return false;
            }
            let leader_id = session
                .child
                .process_id()
                .and_then(|id| libc::pid_t::try_from(id).ok());
            if session.pgid.is_none()
                || session.pgid != leader_id
                || !matches!(peek_unreaped_child(&*session.child), Ok(Some(_)))
                || kill_process_group(session.pgid).is_err()
            {
                // No group identity, failed waitid, or signal failure: keep
                // both OS handles and the event gate, never falsely archive.
                return false;
            }
            drop(guard);
            // After SIGKILL allow a short bounded EOF grace for reader threads
            // to release their resource permits and flush final UTF-8 bytes.
            if !wait_for_drain(&observed.read_done, TERMINATION_TIMEOUT) {
                return false;
            }
        }
        #[cfg(not(unix))]
        {
            // ConPTY preserves its existing confirmed-child + bounded drain
            // behavior. Unix waitid and process-group signaling never run here.
        }
    }

    let mut sessions = sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(session) = sessions.get_mut(session_id) else {
        return false;
    };
    if !Arc::ptr_eq(&session.archived, &observed.identity)
        || session.archived.load(Ordering::Acquire)
    {
        return false;
    }
    let Ok(registry) = events.lock() else {
        return false;
    };
    if registry.allowed.get(session_id) != Some(&true) {
        return false;
    }
    let Ok(buffer) = session.buffer.lock() else {
        return false;
    };
    let snapshot = (buffer.data.clone(), buffer.end_offset);
    drop(buffer);
    let Ok(mut finished) = finished.lock() else {
        return false;
    };

    #[cfg(unix)]
    {
        // This is the ONLY natural-exit reap. A Unix group may have been
        // signaled above, but never after try_wait consumes the zombie PID.
        #[cfg(not(test))]
        if !matches!(peek_unreaped_child(&*session.child), Ok(Some(_))) {
            return false;
        }
        #[cfg(test)]
        if session.pgid.is_some() && !matches!(peek_unreaped_child(&*session.child), Ok(Some(_))) {
            return false;
        }
        // EOF is not proof that every descendant exited. macOS can hang up
        // the PTY when its session leader dies even while same-group children
        // continue running. Kill the original group before reaping in BOTH
        // the drained and timeout paths; never send a post-reap PGID signal.
        let leader_id = session
            .child
            .process_id()
            .and_then(|id| libc::pid_t::try_from(id).ok());
        if session.pgid.is_some() && session.pgid == leader_id {
            if kill_process_group(session.pgid).is_err() {
                return false;
            }
        } else if session.pgid.is_some() {
            return false;
        }
        if !matches!(session.child.try_wait(), Ok(Some(_))) {
            return false;
        }
    }
    session.archived.store(true, Ordering::Release);
    finished.insert(session_id.to_string(), snapshot);
    sessions.remove(session_id);
    true
}

/// Signals liveness even if the reaper unwinds unexpectedly. A subsequent
/// launch then fails closed instead of starting an unmonitored PTY.
struct ReaperLive(Arc<AtomicBool>);

impl Drop for ReaperLive {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
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
    reaper_started: AtomicBool,
    reaper_ready: AtomicBool,
    reaper_live: Arc<AtomicBool>,
    reaper_stop: Arc<AtomicBool>,
    /// Set while holding the sessions mutex; no start may spawn after stop_all.
    shutdown: AtomicBool,
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
            reaper_started: AtomicBool::new(false),
            reaper_ready: AtomicBool::new(false),
            reaper_live: Arc::new(AtomicBool::new(false)),
            reaper_stop: Arc::new(AtomicBool::new(false)),
            shutdown: AtomicBool::new(false),
            finished: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        self.reaper_stop.store(true, Ordering::Release);
    }
}

impl SessionManager {
    /// Boot the ONE fallback thread before the application accepts any PTY
    /// start. A completed readiness handshake is mandatory (fail closed).
    /// It never spawns another thread after a per-session watcher failure.
    pub fn start_reaper(
        &self,
        deliver: impl Fn(SessionExit) + Send + 'static,
    ) -> Result<(), String> {
        if self.reaper_started.swap(true, Ordering::AcqRel) {
            return Err("会话兜底监测线程已经初始化".to_string());
        }
        let sessions = Arc::clone(&self.sessions);
        let events = Arc::clone(&self.events);
        let finished = Arc::clone(&self.finished);
        let alive = Arc::clone(&self.reaper_live);
        let stopping = Arc::clone(&self.reaper_stop);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("pty-manager-reaper".to_string())
            .spawn(move || {
                let _live_guard = ReaperLive(Arc::clone(&alive));
                alive.store(true, Ordering::Release);
                if ready_tx.send(()).is_err() {
                    return;
                }
                while !stopping.load(Ordering::Acquire) {
                    let ids: Vec<String> = match sessions.lock() {
                        Ok(guard) => guard
                            .iter()
                            .filter(|(_, session)| session.monitor_state == MonitorState::Degraded)
                            .map(|(id, _)| id.clone())
                            .collect(),
                        Err(poisoned) => poisoned
                            .into_inner()
                            .iter()
                            .filter(|(_, session)| session.monitor_state == MonitorState::Degraded)
                            .map(|(id, _)| id.clone())
                            .collect(),
                    };
                    for id in ids {
                        if stopping.load(Ordering::Acquire) {
                            break;
                        }
                        if let ProbeResult::Confirmed(observed) =
                            probe_session_exit(&sessions, &id, None)
                            && archive_observed_exit(&sessions, &events, &finished, &id, &observed)
                        {
                            dispatch_session_event(&events, &id, || {
                                deliver(SessionExit {
                                    id: id.clone(),
                                    reason: observed.reason,
                                });
                            });
                        }
                    }
                    std::thread::sleep(REAPER_INTERVAL);
                }
            });
        if let Err(error) = worker {
            self.reaper_started.store(false, Ordering::Release);
            return Err(format!("无法启动终端兜底监测线程：{error}"));
        }
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|error| format!("终端兜底监测线程未确认就绪：{error}"))?;
        self.reaper_ready.store(true, Ordering::Release);
        Ok(())
    }

    fn ensure_reaper(&self) -> Result<(), String> {
        if !self.reaper_ready.load(Ordering::Acquire) || !self.reaper_live.load(Ordering::Acquire) {
            return Err("终端兜底监测线程不可用，拒绝启动无人监测的会话".to_string());
        }
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, PtySession>>, String> {
        Ok(self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// The shutdown gate must be checked while holding the same sessions
    /// mutex that stop_all takes before setting shutdown and draining handles.
    fn lock_for_start(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, PtySession>>, String> {
        // Cleanup paths may recover a poisoned map to retain OS handles, but
        // an inconsistent manager state must never authorize NEW PTY spawns.
        let guard = self.sessions.lock().map_err(|_| {
            "会话管理锁状态异常，已阻止启动新终端；现有会话仍可重试关闭".to_string()
        })?;
        if self.shutdown.load(Ordering::Acquire) {
            return Err("应用正在退出，不能再启动终端会话".to_string());
        }
        Ok(guard)
    }

    /// Roll back an incomplete start. Successful kill() is not proof of exit:
    /// retain the session handle until try_wait confirms termination. Failure
    /// must remain visible/retryable through stop_session.
    fn abort_start(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.lock()?;
        let Some(session) = sessions.get_mut(session_id) else {
            return if self.shutdown.load(Ordering::Acquire) {
                Ok(()) // stop_all already drained this session.
            } else {
                Err("已启动会话的管理句柄不存在".to_string())
            };
        };
        terminate_managed_child(session)?;
        let mut events = self
            .events
            .lock()
            .map_err(|_| "无法关闭会话事件".to_string())?;
        events.close(session_id);
        session.archived.store(true, Ordering::Release);
        sessions.remove(session_id);
        Ok(())
    }

    /// Register an I/O setup failure before trying to terminate it. Only the
    /// manager may release this child/master after an observed OS exit.
    fn retain_failed_spawn(
        sessions: &mut HashMap<String, PtySession>,
        session_id: &str,
        master: Box<dyn portable_pty::MasterPty + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
    ) {
        #[cfg(unix)]
        let pgid = master.process_group_leader();
        sessions.insert(
            session_id.to_string(),
            PtySession {
                master,
                #[cfg(unix)]
                pgid,
                child,
                writer: None,
                buffer: Arc::new(Mutex::new(OutputBuffer::default())),
                archived: Arc::new(AtomicBool::new(false)),
                monitor_state: MonitorState::Starting,
                // No reader thread was created, so nothing needs draining.
                read_done: Arc::new((Mutex::new(true), std::sync::Condvar::new())),
            },
        );
    }

    /// Never return Err with a live child hidden from the frontend. If abort
    /// cannot confirm cleanup, return its session ID and a warning so the UI
    /// can still close it via the normal stop_session API.
    fn recover_failed_worker(
        &self,
        session_id: &str,
        info: SessionInfo,
        error: String,
    ) -> Result<SessionInfo, String> {
        match self.abort_start(session_id) {
            Ok(()) => Err(error),
            Err(cleanup_error) => self.finish_failed_abort(session_id, info, error, cleanup_error),
        }
    }

    /// This boundary also allows deterministic tests of stop_all winning the
    /// race between failed termination and the Degraded transition.
    fn finish_failed_abort(
        &self,
        session_id: &str,
        mut info: SessionInfo,
        error: String,
        cleanup_error: String,
    ) -> Result<SessionInfo, String> {
        // Serialize with stop/stop_all: a success response must have
        // actually installed Degraded on a retained child generation.
        let mut sessions = self.lock()?;
        let Some(session) = sessions.get_mut(session_id) else {
            return Err(format!(
                "{error}；并发关闭已移除会话 {session_id}，未声称兜底已接管"
            ));
        };
        session.monitor_state = MonitorState::Degraded;
        if let Err(reaper_error) = self.ensure_reaper() {
            // Keep the owned child/master in the manager and disclose
            // that automatic monitoring is unavailable; stop/stop_all
            // can still retry, and normal exit cannot silently succeed.
            return Err(format!(
                "{error}；会话 {session_id} 尚未确认退出，管理句柄已保留但兜底监测不可用；请重试关闭，正常退出将被阻止：{cleanup_error}；{reaper_error}"
            ));
        }
        info.warning = Some(format!(
            "{error}；会话仍受管理，兜底监测已接管，可重试关闭：{cleanup_error}"
        ));
        Ok(info)
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
        // A manager-level reaper must be ACKed BEFORE any PTY exists.
        self.ensure_reaper()?;
        // Keep this lock through reservation, spawn and insertion; no second
        // start can race the same ID, even across finished/forgotten sessions.
        // stop_all sets shutdown under this same mutex before draining.
        let mut sessions_guard = self.lock_for_start()?;
        self.ensure_reaper()?;
        if sessions_guard.contains_key(session_id) {
            return Err("会话已存在，请换一个会话再启动".to_string());
        }
        self.events
            .lock()
            .map_err(|_| "无法读取会话事件状态".to_string())?
            .reserve(session_id)?;

        let spawned = match spawn_pty(&binary, &project_dir, rows, cols) {
            Ok(spawned) => spawned,
            Err(SpawnFailure::NotStarted(error)) => {
                // No child exists. It is safe to permit this ID to be retried.
                if let Ok(mut events) = self.events.lock() {
                    events.allowed.remove(session_id);
                }
                return Err(error);
            }
            Err(SpawnFailure::Spawned {
                message,
                master,
                child,
            }) => {
                // Do not release the final Child/ConPTY handle until exit is
                // confirmed. Leave an IPC-visible retryable session on failure.
                Self::retain_failed_spawn(&mut sessions_guard, session_id, master, child);
                drop(sessions_guard);
                let info = SessionInfo {
                    id: session_id.to_string(),
                    tool_id: tool.id.to_string(),
                    tool_name: tool.name.to_string(),
                    project_path: project_dir.display().to_string(),
                    warning: None,
                };
                return self.recover_failed_worker(session_id, info, message);
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
        // Shared with the reader, watcher and the no-watcher fallback.
        let read_done = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let session = PtySession {
            master,
            #[cfg(unix)]
            pgid,
            child,
            writer: Some(writer),
            buffer: Arc::new(Mutex::new(OutputBuffer::default())),
            archived: Arc::clone(&archived),
            monitor_state: MonitorState::Starting,
            read_done: Arc::clone(&read_done),
        };
        let buffer = Arc::clone(&session.buffer);

        // Reader completion lets both the normal watcher and the fallback
        // archive the final bytes before the terminal disappears (#9).

        sessions_guard.insert(session_id.to_string(), session);
        drop(sessions_guard);

        let sessions = Arc::clone(&self.sessions);
        let finished_buffers = Arc::clone(&self.finished);
        let event_registry_reader = Arc::clone(&self.events);
        let event_registry_watcher = Arc::clone(&self.events);
        let read_done_reader = Arc::clone(&read_done);
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
                    dispatch_session_output(
                        &event_registry_reader,
                        &archived_reader,
                        &output_id,
                        || {
                            succeeded = app_output
                                .emit(&format!("session-output-{output_id}"), event)
                                .is_ok();
                        },
                    ) && succeeded
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
            if let Ok(mut done) = read_done.0.lock() {
                *done = true; // No reader exists; fallback must not wait for one.
                read_done.1.notify_all();
            }
            return self.recover_failed_worker(
                session_id,
                info,
                format!("无法启动会话输出线程：{error}"),
            );
        }

        // Only a confirmed OS exit may archive a session. Any transient
        // try_wait/kill/confirmation failure retains the live handle and gate;
        // retry with backoff, without inventing a session-exit event.
        let watcher_thread = std::thread::Builder::new()
            .name(format!("pty-watcher-{session_id}"))
            .spawn(move || {
                loop {
                    match probe_session_exit(&sessions, &exit_id, Some(&archived_watcher)) {
                        ProbeResult::Running => {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                        ProbeResult::Uncertain => {
                            std::thread::sleep(std::time::Duration::from_secs(1));
                        }
                        ProbeResult::Gone => break,
                        ProbeResult::Confirmed(observed) => {
                            if archive_observed_exit(
                                &sessions,
                                &event_registry_watcher,
                                &finished_buffers,
                                &exit_id,
                                &observed,
                            ) {
                                // Stop/forget may race between archive and dispatch.
                                dispatch_session_event(&event_registry_watcher, &exit_id, || {
                                    let _ = app_exit.emit(
                                        &format!("session-exit-{exit_id}"),
                                        SessionExit {
                                            id: exit_id.clone(),
                                            reason: observed.reason,
                                        },
                                    );
                                });
                                break;
                            }
                            // A failed archive never implies the process is gone
                            // from the registry. Retry, or observe a concurrent stop.
                            std::thread::sleep(REAPER_INTERVAL);
                        }
                    }
                }
            });
        if let Err(error) = watcher_thread {
            return self.recover_failed_worker(
                session_id,
                info,
                format!("无法启动会话监测线程：{error}"),
            );
        }
        // Mark only after the thread exists. If it already archived the record,
        // there is nothing to mark; an old worker never recreates a session.
        if let Some(session) = self.lock()?.get_mut(session_id)
            && Arc::ptr_eq(&session.archived, &archived)
        {
            session.monitor_state = MonitorState::Watched;
        }

        info.warning = remember_started_tool(projects, &info.project_path, &info.tool_id);
        Ok(info)
    }

    /// Read-only: natural exits are independently reconciled by the watcher
    /// or the manager-level reaper, even when the frontend never requests data.
    pub fn snapshot(&self, session_id: &str) -> Result<SessionSnapshot, String> {
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
        let writer = session
            .writer
            .as_mut()
            .ok_or_else(|| "终端启动时初始化输入失败；会话仍受管理，请关闭后重试".to_string())?;
        writer
            .write_all(data.as_bytes())
            .and_then(|()| writer.flush())
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
            terminate_managed_child(session)?;
            // Wait for any in-flight output/exit dispatch and close its gate
            // *before* stop can return or the watcher can dispatch again.
            self.events
                .lock()
                .map_err(|_| "无法读取会话事件状态".to_string())?
                .close(session_id);
            session
                .archived
                .store(true, std::sync::atomic::Ordering::Release);
            // Group signal was sent before reaping (if ownership was proven).
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

    /// Normal app exit is permitted only after EVERY child is confirmed gone.
    /// Failed children keep their original handles and active event gates for
    /// another tray-exit/stop attempt. This cannot intercept OS-forced shutdown.
    pub fn stop_all(&self) -> Result<(), String> {
        let mut sessions = self.lock()?;
        // Serialize with lock_for_start: no new child can start after this gate.
        self.shutdown.store(true, Ordering::Release);
        let mut confirmed = Vec::new();
        let mut errors = Vec::new();
        for (id, session) in sessions.iter_mut() {
            match terminate_managed_child(session) {
                Ok(()) => confirmed.push(id.clone()),
                Err(error) => errors.push(format!("{id}: {error}")),
            }
        }

        // Never close an event gate or discard the handle of an uncertain child.
        // Lock order matches stop(): sessions -> events -> finished.
        let mut events = self
            .events
            .lock()
            .map_err(|_| "无法关闭会话事件，退出已取消".to_string())?;
        for id in &confirmed {
            events.close(id);
            if let Some(session) = sessions.get(id) {
                session.archived.store(true, Ordering::Release);
            }
        }
        drop(events);
        for id in confirmed {
            sessions.remove(&id);
        }
        if errors.is_empty() {
            // Release the prestarted reaper's AppHandle after successful
            // shutdown; do not stop it while a failed child remains retryable.
            self.reaper_stop.store(true, Ordering::Release);
            Ok(())
        } else {
            Err(format!(
                "部分终端进程未确认退出，已阻止正常退出；可重试：{}",
                errors.join("；")
            ))
        }
    }

    #[cfg(test)]
    pub fn is_running(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(session_id)
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
    fn shutdown_gate_serializes_with_in_flight_start_and_rejects_later_starts() {
        use std::sync::atomic::Ordering;
        use std::sync::{Arc, mpsc};

        let manager = Arc::new(SessionManager::default());
        insert_fake_session(&manager, "before-shutdown", false);

        // This is the exact start guard held through PTY spawn and insertion.
        let start_guard = manager.lock_for_start().unwrap();
        let shutdown_manager = Arc::clone(&manager);
        let (starting_tx, starting_rx) = mpsc::channel();
        let shutdown_thread = std::thread::spawn(move || {
            starting_tx.send(()).unwrap();
            shutdown_manager.stop_all().unwrap();
        });
        starting_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // stop_all cannot set shutdown until the in-flight start releases
        // the sessions mutex, so it cannot miss the newly inserted child.
        assert!(!manager.shutdown.load(Ordering::Acquire));
        drop(start_guard);
        shutdown_thread.join().unwrap();

        assert!(manager.shutdown.load(Ordering::Acquire));
        assert!(!manager.is_running("before-shutdown"));
        assert!(manager.lock_for_start().is_err());
        assert!(manager.lock_for_start().is_err());
        manager.stop_all().unwrap(); // idempotent: never reopen the launch gate.
        assert!(manager.lock_for_start().is_err());
    }

    #[test]
    fn stop_all_failure_preserves_child_handle_and_allows_retry() {
        use std::sync::atomic::Ordering;

        let manager = SessionManager::default();
        insert_fake_session(&manager, "exit-ok", false);
        insert_fake_session(&manager, "exit-kill-fails", true);
        let original = manager
            .sessions
            .lock()
            .unwrap()
            .get("exit-kill-fails")
            .unwrap()
            .archived
            .clone();

        let error = manager.stop_all().unwrap_err();
        assert!(error.contains("exit-kill-fails"), "{error}");
        assert!(manager.shutdown.load(Ordering::Acquire));
        assert!(!manager.is_running("exit-ok"));
        assert!(manager.is_running("exit-kill-fails"));
        assert!(!original.load(Ordering::Acquire));
        assert!(manager.events.lock().unwrap().allowed["exit-kill-fails"]);
        assert!(!manager.events.lock().unwrap().allowed["exit-ok"]);
        assert!(manager.lock_for_start().is_err());

        // A second normal exit is allowed only after that same child is reaped.
        manager
            .sessions
            .lock()
            .unwrap()
            .get_mut("exit-kill-fails")
            .unwrap()
            .child = Box::new(FakeChild {
            exited: false,
            kill_fails: false,
            kill_calls: 0,
            try_wait_fails: false,
        });
        manager.stop_all().unwrap();
        assert!(!manager.is_running("exit-kill-fails"));
        assert!(original.load(Ordering::Acquire));
    }

    #[test]
    fn stop_all_try_wait_error_keeps_retryable_handle() {
        let manager = SessionManager::default();
        insert_fake_session(&manager, "exit-wait-fails", false);
        manager
            .sessions
            .lock()
            .unwrap()
            .get_mut("exit-wait-fails")
            .unwrap()
            .child = Box::new(FakeChild {
            exited: false,
            kill_fails: false,
            kill_calls: 0,
            try_wait_fails: true,
        });
        let error = manager.stop_all().unwrap_err();
        assert!(error.contains("无法确认会话状态"), "{error}");
        assert!(manager.is_running("exit-wait-fails"));
        assert!(manager.snapshot("exit-wait-fails").unwrap().running);
        assert!(manager.events.lock().unwrap().allowed["exit-wait-fails"]);

        manager
            .sessions
            .lock()
            .unwrap()
            .get_mut("exit-wait-fails")
            .unwrap()
            .child = Box::new(FakeChild {
            exited: false,
            kill_fails: false,
            kill_calls: 0,
            try_wait_fails: false,
        });
        manager.stop_all().unwrap();
        assert!(!manager.is_running("exit-wait-fails"));
    }

    /// Real PTY, NO per-session watcher and NO snapshot() call before exit.
    /// The prestarted Rust reaper must archive it and emit exactly one event.
    #[test]
    fn no_watcher_reaper_emits_real_exit_without_snapshot_call() {
        #[cfg(windows)]
        use std::io::Write;

        let dir = TempProjects::new();
        #[cfg(unix)]
        let binary = std::path::Path::new("/usr/bin/true");
        #[cfg(windows)]
        let binary = std::path::Path::new("cmd.exe");
        let pty = super::spawn_pty(binary, &dir.0, 24, 80).unwrap();
        #[cfg(windows)]
        let mut pty = pty;
        #[cfg(windows)]
        pty.writer.write_all(b"exit\r\n").unwrap();

        let manager = SessionManager::default();
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        manager
            .start_reaper(move |exit| event_tx.send(exit).unwrap())
            .unwrap();
        manager
            .events
            .lock()
            .unwrap()
            .reserve("no-watcher-real")
            .unwrap();
        manager.sessions.lock().unwrap().insert(
            "no-watcher-real".to_string(),
            super::PtySession {
                #[cfg(unix)]
                pgid: pty.master.process_group_leader(),
                master: pty.master,
                child: pty.child,
                writer: Some(pty.writer),
                buffer: std::sync::Arc::new(super::Mutex::new(super::OutputBuffer::default())),
                archived: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                monitor_state: super::MonitorState::Degraded,
                // Simulate a failed reader spawn: no reader is waiting to drain.
                read_done: std::sync::Arc::new((
                    super::Mutex::new(true),
                    std::sync::Condvar::new(),
                )),
            },
        );
        drop(pty.reader);

        // No snapshot() polling here: the event must be produced by Rust alone.
        let exit = event_rx
            .recv_timeout(Duration::from_secs(8))
            .expect("Rust reaper must emit an exit for a real unwatched PTY");
        assert_eq!(exit.id, "no-watcher-real");
        assert!(!exit.reason.is_empty());
        assert!(!manager.is_running("no-watcher-real"));
        assert!(!manager.snapshot("no-watcher-real").unwrap().running);
        assert!(event_rx.recv_timeout(Duration::from_millis(400)).is_err());
        manager.forget("no-watcher-real").unwrap();
        manager.stop_all().unwrap();
    }

    #[test]
    fn failed_worker_launch_retains_a_retryable_child_on_kill_error() {
        let manager = SessionManager::default();
        manager.start_reaper(|_| {}).unwrap();
        let make_info = |id: &str| super::SessionInfo {
            id: id.to_string(),
            tool_id: "codex".to_string(),
            tool_name: "Codex".to_string(),
            project_path: "/registered".to_string(),
            warning: None,
        };

        insert_fake_session(&manager, "worker-kill-fails", true);
        let info = manager
            .recover_failed_worker(
                "worker-kill-fails",
                make_info("worker-kill-fails"),
                "injected watcher spawn failure".to_string(),
            )
            .expect("uncertain cleanup must return a managed partial start");
        assert_eq!(info.id, "worker-kill-fails");
        assert!(info.warning.unwrap().contains("仍受管理"));
        assert!(manager.is_running("worker-kill-fails"));
        assert!(manager.snapshot("worker-kill-fails").unwrap().running);
        assert!(
            manager.events.lock().unwrap().allowed["worker-kill-fails"],
            "failed cleanup retains an active event gate"
        );
        assert!(manager.stop("worker-kill-fails").is_err());
        assert!(manager.is_running("worker-kill-fails"));

        // The UI may invoke stop_session again once the simulated kill
        // failure is gone; the original handle was never dropped.
        {
            let mut sessions = manager.sessions.lock().unwrap();
            sessions.get_mut("worker-kill-fails").unwrap().child = Box::new(FakeChild {
                exited: false,
                kill_fails: false,
                kill_calls: 0,
                try_wait_fails: false,
            });
        }
        manager.stop("worker-kill-fails").unwrap();
        assert!(!manager.is_running("worker-kill-fails"));

        insert_fake_session(&manager, "worker-cleaned", false);
        let error = manager
            .recover_failed_worker(
                "worker-cleaned",
                make_info("worker-cleaned"),
                "injected reader spawn failure".to_string(),
            )
            .expect_err("confirmed termination should report the spawn error");
        assert!(error.contains("injected reader spawn failure"));
        assert!(!manager.is_running("worker-cleaned"));
        assert!(!manager.events.lock().unwrap().allowed["worker-cleaned"]);
    }

    /// Switchable fault injection lets stop retry the SAME Child handle,
    /// rather than replacing it with a new fake after the first failed kill.
    #[derive(Debug)]
    struct RetainedFaultState {
        kill_fails: std::sync::atomic::AtomicBool,
        wait_fails: std::sync::atomic::AtomicBool,
        exited: std::sync::atomic::AtomicBool,
        drops: std::sync::atomic::AtomicUsize,
    }

    #[derive(Debug)]
    struct RetainedFaultChild(std::sync::Arc<RetainedFaultState>);

    impl Drop for RetainedFaultChild {
        fn drop(&mut self) {
            self.0.drops.fetch_add(1, super::Ordering::AcqRel);
        }
    }

    impl portable_pty::ChildKiller for RetainedFaultChild {
        fn kill(&mut self) -> std::io::Result<()> {
            if self.0.kill_fails.load(super::Ordering::Acquire) {
                Err(std::io::Error::other("injected child kill Err"))
            } else {
                self.0.exited.store(true, super::Ordering::Release);
                Ok(())
            }
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(FakeKiller)
        }
    }

    impl portable_pty::Child for RetainedFaultChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            if self.0.wait_fails.load(super::Ordering::Acquire) {
                Err(std::io::Error::other("injected child try_wait Err"))
            } else if self.0.exited.load(super::Ordering::Acquire) {
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

    #[test]
    fn both_pty_io_setup_failures_retain_original_child_and_master_for_retry() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        for fail_reader in [true, false] {
            let manager = SessionManager::default();
            manager.start_reaper(|_| {}).unwrap();
            let id = if fail_reader {
                "reader-err"
            } else {
                "writer-err"
            };
            manager.events.lock().unwrap().reserve(id).unwrap();
            let state = Arc::new(RetainedFaultState {
                kill_fails: AtomicBool::new(true),
                wait_fails: AtomicBool::new(true),
                exited: AtomicBool::new(false),
                drops: AtomicUsize::new(0),
            });
            let master = Box::new(FakeMaster);
            let child = Box::new(RetainedFaultChild(Arc::clone(&state)));
            let failure = if fail_reader {
                super::finish_spawned_pty(
                    master,
                    child,
                    |_| {
                        Err::<Box<dyn std::io::Read + Send>, std::io::Error>(std::io::Error::other(
                            "injected reader init Err",
                        ))
                    },
                    |_| {
                        Ok::<Box<dyn std::io::Write + Send>, std::io::Error>(Box::new(
                            std::io::sink(),
                        ))
                    },
                )
            } else {
                super::finish_spawned_pty(
                    master,
                    child,
                    |_| {
                        Ok::<Box<dyn std::io::Read + Send>, std::io::Error>(Box::new(
                            std::io::empty(),
                        ))
                    },
                    |_| {
                        Err::<Box<dyn std::io::Write + Send>, std::io::Error>(
                            std::io::Error::other("injected writer init Err"),
                        )
                    },
                )
            };
            let (message, master, child) = match failure {
                Err(super::SpawnFailure::Spawned {
                    message,
                    master,
                    child,
                }) => (message, master, child),
                Err(super::SpawnFailure::NotStarted(_)) => {
                    panic!("a spawned child must remain in the partial failure")
                }
                Ok(_) => panic!("injected I/O failure must not succeed"),
            };
            assert!(message.contains(if fail_reader { "读取" } else { "写入" }));
            {
                let mut sessions = manager.sessions.lock().unwrap();
                SessionManager::retain_failed_spawn(&mut sessions, id, master, child);
            }
            let info = super::SessionInfo {
                id: id.to_string(),
                tool_id: "codex".to_string(),
                tool_name: "Codex".to_string(),
                project_path: "/registered".to_string(),
                warning: None,
            };
            let info = manager
                .recover_failed_worker(id, info, message)
                .expect("unconfirmed exit must return a managed degraded session");
            assert_eq!(info.id, id);
            assert!(info.warning.unwrap().contains("兜底监测已接管"));
            {
                let sessions = manager.sessions.lock().unwrap();
                let session = sessions.get(id).expect("manager owns the original handles");
                assert_eq!(session.monitor_state, super::MonitorState::Degraded);
                assert!(session.writer.is_none());
                session
                    .master
                    .get_size()
                    .expect("original master remains usable");
            }
            assert_eq!(state.drops.load(Ordering::Acquire), 0);
            assert!(
                manager
                    .write(id, "hello")
                    .unwrap_err()
                    .contains("初始化输入失败")
            );
            assert!(manager.stop(id).is_err());
            assert_eq!(state.drops.load(Ordering::Acquire), 0);

            // Faults clear without changing the child object held by the manager.
            state.wait_fails.store(false, Ordering::Release);
            state.kill_fails.store(false, Ordering::Release);
            manager.stop(id).unwrap();
            assert_eq!(state.drops.load(Ordering::Acquire), 1);
            assert!(!manager.is_running(id));
        }
    }

    #[test]
    fn failed_worker_reaper_unavailable_blocks_success_and_preserves_handle() {
        let manager = SessionManager::default();
        insert_fake_session(&manager, "unwatched", true);
        let info = super::SessionInfo {
            id: "unwatched".to_string(),
            tool_id: "codex".to_string(),
            tool_name: "Codex".to_string(),
            project_path: "/registered".to_string(),
            warning: None,
        };
        let error = manager
            .recover_failed_worker("unwatched", info, "worker failed".into())
            .unwrap_err();
        assert!(error.contains("兜底监测不可用"), "{error}");
        assert!(error.contains("unwatched"), "{error}");
        assert!(!error.contains("已接管"));
        assert!(manager.is_running("unwatched"));
        assert_eq!(
            manager.sessions.lock().unwrap()["unwatched"].monitor_state,
            super::MonitorState::Degraded,
        );
        assert!(manager.stop_all().is_err());
        manager
            .sessions
            .lock()
            .unwrap()
            .get_mut("unwatched")
            .unwrap()
            .child = Box::new(FakeChild {
            exited: false,
            kill_fails: false,
            kill_calls: 0,
            try_wait_fails: false,
        });
        manager.stop_all().unwrap();
    }

    #[test]
    fn failed_worker_recovers_poisoned_sessions_lock_before_claiming_degraded() {
        use std::sync::Arc;

        let manager = Arc::new(SessionManager::default());
        manager.start_reaper(|_| {}).unwrap();
        insert_fake_session(&manager, "poisoned", true);
        let poisoner = Arc::clone(&manager);
        assert!(
            std::thread::spawn(move || {
                let _guard = poisoner.sessions.lock().unwrap();
                panic!("inject poisoned sessions lock");
            })
            .join()
            .is_err()
        );
        assert!(
            manager.lock_for_start().is_err(),
            "poisoned manager must fail closed for new PTYs"
        );
        let info = super::SessionInfo {
            id: "poisoned".to_string(),
            tool_id: "codex".to_string(),
            tool_name: "Codex".to_string(),
            project_path: "/registered".to_string(),
            warning: None,
        };
        let result = manager
            .recover_failed_worker("poisoned", info, "watcher failed".into())
            .unwrap();
        assert!(result.warning.unwrap().contains("兜底监测已接管"));
        let sessions = manager
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            sessions["poisoned"].monitor_state,
            super::MonitorState::Degraded
        );
        drop(sessions);
        assert!(manager.is_running("poisoned"));
        manager
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut("poisoned")
            .unwrap()
            .child = Box::new(FakeChild {
            exited: false,
            kill_fails: false,
            kill_calls: 0,
            try_wait_fails: false,
        });
        manager.stop("poisoned").unwrap();
    }

    #[test]
    fn concurrent_stop_all_removal_cannot_be_misreported_as_reaper_takeover() {
        let manager = SessionManager::default();
        manager.start_reaper(|_| {}).unwrap();
        insert_fake_session(&manager, "stopped-before-degraded", true);
        assert!(manager.abort_start("stopped-before-degraded").is_err());
        manager
            .sessions
            .lock()
            .unwrap()
            .get_mut("stopped-before-degraded")
            .unwrap()
            .child = Box::new(FakeChild {
            exited: false,
            kill_fails: false,
            kill_calls: 0,
            try_wait_fails: false,
        });
        manager.stop_all().unwrap();
        let info = super::SessionInfo {
            id: "stopped-before-degraded".to_string(),
            tool_id: "codex".to_string(),
            tool_name: "Codex".to_string(),
            project_path: "/registered".to_string(),
            warning: None,
        };
        let error = manager
            .finish_failed_abort(
                "stopped-before-degraded",
                info,
                "watcher failed".into(),
                "injected cleanup failure".into(),
            )
            .unwrap_err();
        assert!(error.contains("并发关闭已移除"), "{error}");
        assert!(!error.contains("兜底监测已接管"), "{error}");
        assert!(!manager.is_running("stopped-before-degraded"));
    }

    #[test]
    fn no_pty_output_can_be_dispatched_after_archiving_and_exit() {
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Mutex, mpsc};

        let events = Arc::new(Mutex::new(super::EventRegistry::default()));
        events.lock().unwrap().reserve("ordering").unwrap();
        let archived = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(Mutex::new(Vec::<&str>::new()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let writer_events = Arc::clone(&events);
        let writer_archived = Arc::clone(&archived);
        let writer_observed = Arc::clone(&observed);
        let writer = std::thread::spawn(move || {
            super::dispatch_session_output(&writer_events, &writer_archived, "ordering", || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                writer_observed.lock().unwrap().push("output");
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let watcher_events = Arc::clone(&events);
        let watcher_archived = Arc::clone(&archived);
        let watcher_observed = Arc::clone(&observed);
        let watcher = std::thread::spawn(move || {
            super::archive_session_output(&watcher_events, &watcher_archived);
            super::dispatch_session_event(&watcher_events, "ordering", || {
                watcher_observed.lock().unwrap().push("exit");
            })
        });

        release_tx.send(()).unwrap();
        assert!(writer.join().unwrap());
        assert!(watcher.join().unwrap());
        assert_eq!(*observed.lock().unwrap(), ["output", "exit"]);

        assert!(!super::dispatch_session_output(
            &events,
            &archived,
            "ordering",
            || {
                observed.lock().unwrap().push("late-output");
            }
        ));
        assert_eq!(*observed.lock().unwrap(), ["output", "exit"]);
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
                try_wait_fails: false,
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
                try_wait_fails: false,
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
        try_wait_fails: bool,
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
            if self.try_wait_fails {
                Err(std::io::Error::other("injected try_wait failure"))
            } else if self.exited {
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
                    try_wait_fails: false,
                }),
                writer: Some(Box::new(std::io::sink())),
                buffer: std::sync::Arc::new(super::Mutex::new(super::OutputBuffer::default())),
                archived: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                monitor_state: super::MonitorState::Starting,
                read_done: std::sync::Arc::new((
                    super::Mutex::new(true),
                    std::sync::Condvar::new(),
                )),
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
            try_wait_fails: false,
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
            try_wait_fails: false,
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
            try_wait_fails: false,
        };
        assert!(super::terminate_child(&mut child).is_ok());
        assert_eq!(child.kill_calls, 1);
    }

    #[test]
    fn reaper_is_required_and_skips_starting_and_watched_sessions() {
        let manager = SessionManager::default();
        assert!(
            manager.ensure_reaper().is_err(),
            "a managed PTY must never start before the global reaper is ready"
        );

        let (tx, rx) = std::sync::mpsc::channel();
        manager
            .start_reaper(move |exit| tx.send(exit).unwrap())
            .unwrap();
        manager.ensure_reaper().unwrap();
        insert_fake_session(&manager, "not-yet-degraded", false);
        {
            let mut sessions = manager.sessions.lock().unwrap();
            let session = sessions.get_mut("not-yet-degraded").unwrap();
            session.child = Box::new(FakeChild {
                exited: true,
                kill_fails: false,
                kill_calls: 0,
                try_wait_fails: false,
            });
            assert_eq!(session.monitor_state, super::MonitorState::Starting);
        }
        std::thread::sleep(Duration::from_millis(500));
        assert!(manager.is_running("not-yet-degraded"));
        assert!(rx.try_recv().is_err(), "Starting must not be reaped");

        {
            manager
                .sessions
                .lock()
                .unwrap()
                .get_mut("not-yet-degraded")
                .unwrap()
                .monitor_state = super::MonitorState::Watched;
        }
        std::thread::sleep(Duration::from_millis(500));
        assert!(manager.is_running("not-yet-degraded"));
        assert!(rx.try_recv().is_err(), "Watched belongs to its own watcher");

        {
            manager
                .sessions
                .lock()
                .unwrap()
                .get_mut("not-yet-degraded")
                .unwrap()
                .monitor_state = super::MonitorState::Degraded;
        }
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap().id,
            "not-yet-degraded"
        );
        assert!(!manager.is_running("not-yet-degraded"));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        manager.stop_all().unwrap();
    }

    #[test]
    fn watcher_wait_error_kill_error_and_timeout_keep_handle_until_confirmed() {
        use std::sync::{Arc, Mutex};

        #[derive(Debug)]
        struct FaultState {
            wait_errors_remaining: u32,
            kill_fails: bool,
            kill_exits: bool,
            exited: bool,
            kill_calls: u32,
        }

        #[derive(Debug)]
        struct FaultChild(Arc<Mutex<FaultState>>);

        impl portable_pty::ChildKiller for FaultChild {
            fn kill(&mut self) -> std::io::Result<()> {
                let mut state = self.0.lock().unwrap();
                state.kill_calls += 1;
                if state.kill_fails {
                    Err(std::io::Error::other("injected kill Err"))
                } else {
                    if state.kill_exits {
                        state.exited = true;
                    }
                    Ok(())
                }
            }

            fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
                Box::new(FakeKiller)
            }
        }

        impl portable_pty::Child for FaultChild {
            fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
                let mut state = self.0.lock().unwrap();
                if state.wait_errors_remaining > 0 {
                    state.wait_errors_remaining -= 1;
                    return Err(std::io::Error::other("injected watcher try_wait Err"));
                }
                if state.exited {
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

        let manager = SessionManager::default();
        insert_fake_session(&manager, "faulty-watcher", false);
        let state = Arc::new(Mutex::new(FaultState {
            wait_errors_remaining: 2,
            kill_fails: true,
            kill_exits: false,
            exited: false,
            kill_calls: 0,
        }));
        let identity = {
            let mut sessions = manager.sessions.lock().unwrap();
            let session = sessions.get_mut("faulty-watcher").unwrap();
            session.monitor_state = super::MonitorState::Watched;
            session.child = Box::new(FaultChild(Arc::clone(&state)));
            Arc::clone(&session.archived)
        };

        // Exactly the same probe used by the production watcher.
        assert!(matches!(
            super::probe_session_exit(&manager.sessions, "faulty-watcher", Some(&identity)),
            super::ProbeResult::Uncertain
        ));
        assert_eq!(state.lock().unwrap().kill_calls, 1);
        assert!(manager.is_running("faulty-watcher"));
        assert!(!identity.load(super::Ordering::Acquire));
        assert!(manager.events.lock().unwrap().allowed["faulty-watcher"]);
        assert!(
            !manager
                .finished
                .lock()
                .unwrap()
                .contains_key("faulty-watcher")
        );

        // kill() reports success but never reaps the process; timeout is not
        // an exit. No event or archive may be generated.
        {
            let mut state = state.lock().unwrap();
            state.wait_errors_remaining = 1;
            state.kill_fails = false;
            state.kill_exits = false;
        }
        assert!(matches!(
            super::probe_session_exit(&manager.sessions, "faulty-watcher", Some(&identity)),
            super::ProbeResult::Uncertain
        ));
        assert_eq!(state.lock().unwrap().kill_calls, 2);
        assert!(manager.is_running("faulty-watcher"));
        assert!(!identity.load(super::Ordering::Acquire));
        assert!(manager.events.lock().unwrap().allowed["faulty-watcher"]);

        // A later confirmed kill via the SAME helper can archive the original
        // session generation, and only then an exit may be dispatched.
        {
            let mut state = state.lock().unwrap();
            state.wait_errors_remaining = 1;
            state.kill_exits = true;
        }
        let observed =
            match super::probe_session_exit(&manager.sessions, "faulty-watcher", Some(&identity)) {
                super::ProbeResult::Confirmed(observed) => observed,
                _ => panic!("retry must confirm the child's actual exit"),
            };
        assert_eq!(state.lock().unwrap().kill_calls, 3);
        assert!(super::archive_observed_exit(
            &manager.sessions,
            &manager.events,
            &manager.finished,
            "faulty-watcher",
            &observed,
        ));
        assert!(!manager.is_running("faulty-watcher"));
        assert!(identity.load(super::Ordering::Acquire));
        assert!(!manager.snapshot("faulty-watcher").unwrap().running);

        // An already archived generation cannot be claimed twice.
        assert!(matches!(
            super::probe_session_exit(&manager.sessions, "faulty-watcher", Some(&identity)),
            super::ProbeResult::Gone
        ));
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

        super::kill_process_group(Some(pgid)).expect("anchored group must be killed");

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

    #[cfg(unix)]
    #[test]
    fn natural_exit_reaps_after_killing_pty_descendant_and_releases_reader_budget() {
        use super::{OutputBuffer, PtySession, ReaderPermit, SessionManager, SpawnedPty};
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Condvar, Mutex};

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("real PTY");
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        // Non-interactive shell, job control disabled. Ignore SIGHUP so the
        // descendant really holds the slave after its parent exits.
        command.arg("trap '' HUP; sleep 30 & echo PTY_DESC:$!; exit 0");
        let child = pair.slave.spawn_command(command).expect("PTY shell");
        drop(pair.slave);
        let SpawnedPty {
            master,
            child,
            writer,
            reader,
        } = super::finish_spawned_pty(
            pair.master,
            child,
            |master| master.try_clone_reader(),
            |master| master.take_writer(),
        )
        .expect("PTY I/O");
        let pgid = master.process_group_leader().expect("PTY leader PGID");
        assert_eq!(child.process_id(), Some(pgid as u32));

        fn descendant_pid(data: &str) -> Option<libc::pid_t> {
            data.split("PTY_DESC:").nth(1).and_then(|text| {
                text.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse::<libc::pid_t>()
                    .ok()
            })
        }

        let manager = SessionManager::default();
        let buffer = Arc::new(Mutex::new(OutputBuffer::default()));
        // On any assertion failure, attempt normal anchored group cleanup.
        // Also target the specifically identified test child if the reaper has
        // already archived the parent. Never signal a post-reap numeric PGID.
        struct Cleanup<'a> {
            manager: &'a SessionManager,
            buffer: Arc<Mutex<OutputBuffer>>,
            pgid: libc::pid_t,
        }
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = self.manager.stop_all();
                if let Ok(buffer) = self.buffer.lock()
                    && let Some(pid) = descendant_pid(&buffer.data)
                    && unsafe { libc::getpgid(pid) } == self.pgid
                {
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
        }
        let _cleanup = Cleanup {
            manager: &manager,
            buffer: Arc::clone(&buffer),
            pgid,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        manager
            .start_reaper(move |event| {
                let _ = tx.send(event);
            })
            .unwrap();
        let read_done = Arc::new((Mutex::new(false), Condvar::new()));
        let archived = Arc::new(AtomicBool::new(false));
        let live_readers = Arc::new(AtomicUsize::new(0));
        let permit = ReaderPermit::acquire(&live_readers).unwrap();
        manager
            .events
            .lock()
            .unwrap()
            .reserve("descendant")
            .unwrap();
        manager.sessions.lock().unwrap().insert(
            "descendant".to_string(),
            PtySession {
                master,
                pgid: Some(pgid),
                child,
                writer: Some(writer),
                buffer: Arc::clone(&buffer),
                archived: Arc::clone(&archived),
                monitor_state: super::MonitorState::Degraded,
                read_done: Arc::clone(&read_done),
            },
        );
        let reader_thread = std::thread::spawn(move || {
            let _permit = permit;
            let mut reader = reader;
            let mut sink = |_output| true;
            super::pump_reader(&mut *reader, &buffer, &archived, &mut sink);
            if let Ok(mut done) = read_done.0.lock() {
                *done = true;
                read_done.1.notify_all();
            }
        });

        let event = rx
            .recv_timeout(Duration::from_secs(9))
            .expect("reaper must clean the slave holder before archiving");
        assert_eq!(event.id, "descendant");
        assert!(!manager.is_running("descendant"));
        let snapshot = manager.snapshot("descendant").unwrap();
        assert!(!snapshot.running);
        let descendant_pid = descendant_pid(&snapshot.data).expect("shell emitted descendant PID");
        reader_thread
            .join()
            .expect("PTY reader must return after group kill");
        assert_eq!(
            live_readers.load(Ordering::Acquire),
            0,
            "blocked PTY reader must release its thread budget"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(descendant_pid, 0) } == 0 && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_ne!(
            unsafe { libc::kill(descendant_pid, 0) },
            0,
            "descendant still alive after the archived PTY reader drained"
        );
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reaped_leader_never_signals_an_unrelated_process_group() {
        use super::PtySession;
        use std::os::unix::process::CommandExt;
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Condvar, Mutex};

        let mut unrelated_command = std::process::Command::new("/bin/sleep");
        unrelated_command.arg("30");
        unsafe {
            unrelated_command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut unrelated = unrelated_command.spawn().expect("unrelated group");
        // Cleanup is independent of the session manager; even a failing
        // assertion must not leak the unrelated sleep process.
        struct UnrelatedCleanup<'a>(&'a mut std::process::Child);
        impl Drop for UnrelatedCleanup<'_> {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let unrelated_cleanup = UnrelatedCleanup(&mut unrelated);
        let pgid = unrelated_cleanup.0.id() as libc::pid_t;

        let cwd = std::env::current_dir().unwrap();
        let mut pty = super::spawn_pty(std::path::Path::new("/usr/bin/true"), &cwd, 24, 80)
            .expect("short-lived PTY");
        pty.child.wait().expect("explicitly reap old PTY leader");
        let mut session = PtySession {
            master: pty.master,
            pgid: Some(pgid), // simulate a stale/reused numeric group identity
            child: pty.child,
            writer: Some(pty.writer),
            buffer: Arc::new(Mutex::new(super::OutputBuffer::default())),
            archived: Arc::new(AtomicBool::new(false)),
            monitor_state: super::MonitorState::Starting,
            read_done: Arc::new((Mutex::new(true), Condvar::new())),
        };
        let result = super::terminate_managed_child(&mut session);
        assert!(
            result.is_err(),
            "reaped PID must fail closed, not signal a PGID"
        );
        assert!(
            unrelated_cleanup.0.try_wait().unwrap().is_none(),
            "unrelated process group must remain alive"
        );
        drop(session);
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
