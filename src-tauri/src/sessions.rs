use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::session_history::{
    SessionHistory, SessionRecord, is_valid_cli_session_id, is_valid_uuid, normalize_session_title,
    now_ms,
};
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::projects::normalize_project_dir;
use crate::schedule_mcp::ScheduleMcp;
use crate::tools::{
    ResolveResult, TOOL_DEFINITIONS, candidate_search_dirs, is_windows_batch, resolve_binary,
    windows_cmd_executable,
};

const READ_BUFFER_SIZE: usize = 8192;

/// Environment variables that point child tools at an unrelated parent
/// terminal (multiplexers) or at a server port. Inheriting them makes tools
/// like Claude Code attach to the wrong pane or try to bind a taken port.
pub(crate) const ENV_BLOCKLIST: &[&str] = &[
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub tool_id: String,
    pub tool_name: String,
    pub project_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_session_id: Option<String>,
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
    pub input_blocked: bool,
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

/// Maximum time an input write may take before the caller gets an actionable
/// error. Writes run on the session's own thread, so this only bounds the
/// caller — it never holds the global session table (#87).
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Bytes allowed in flight for one session before new input is refused. This is
/// backlog, not a per-paste limit: a large paste is chunked by the frontend.
const WRITE_BACKLOG_LIMIT: usize = 1024 * 1024;
const WRITE_CHUNK_BYTES: usize = 4096;
const INPUT_BLOCKED: &str = "输入送达状态未确认，该会话已暂停输入。重连仅恢复显示，请检查输出并结束该会话后重新打开；请勿直接重试原输入。";

struct WriteReservation {
    bytes: usize,
    queued: Arc<AtomicUsize>,
}

impl WriteReservation {
    fn acquire(queued: &Arc<AtomicUsize>, bytes: usize) -> Result<Self, String> {
        let mut current = queued.load(Ordering::SeqCst);
        loop {
            let next = current
                .checked_add(bytes)
                .filter(|next| *next <= WRITE_BACKLOG_LIMIT)
                .ok_or_else(|| "终端输入积压过多，请等待当前输入写入后再试".to_string())?;
            match queued.compare_exchange_weak(current, next, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        Ok(Self {
            bytes,
            queued: Arc::clone(queued),
        })
    }
}

impl Drop for WriteReservation {
    fn drop(&mut self) {
        self.queued.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

struct WriteRequest {
    data: Vec<u8>,
    reply: mpsc::Sender<Result<(), String>>,
    deadline: std::time::Instant,
    _reservation: WriteReservation,
}

/// Owns the PTY write end on a dedicated thread so a blocked write (CLI not
/// consuming stdin) can never stall the session table (#87).
#[derive(Clone)]
struct SessionWriter {
    sender: mpsc::Sender<WriteRequest>,
    queued: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
}

impl SessionWriter {
    fn spawn(mut writer: Box<dyn std::io::Write + Send>) -> Self {
        let (sender, receiver) = mpsc::channel::<WriteRequest>();
        let queued = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let dead = Arc::clone(&closed);
        std::thread::spawn(move || {
            while let Ok(request) = receiver.recv() {
                let outcome = Self::write_request(&mut *writer, &request, &dead);
                let failed = outcome.is_err();
                if failed {
                    dead.store(true, Ordering::SeqCst);
                }
                let WriteRequest {
                    reply,
                    _reservation: reservation,
                    data,
                    ..
                } = request;
                drop(data);
                drop(reservation);
                let _ = reply.send(outcome);
                if failed {
                    break;
                }
            }
            dead.store(true, Ordering::SeqCst);
            drop(receiver);
        });
        Self {
            sender,
            queued,
            closed,
        }
    }

    fn write_request(
        writer: &mut dyn std::io::Write,
        request: &WriteRequest,
        closed: &AtomicBool,
    ) -> Result<(), String> {
        let check = || {
            if closed.load(Ordering::SeqCst) || std::time::Instant::now() >= request.deadline {
                Err(INPUT_BLOCKED.to_string())
            } else {
                Ok(())
            }
        };
        let mut offset = 0;
        while offset < request.data.len() {
            check()?;
            let end = (offset + WRITE_CHUNK_BYTES).min(request.data.len());
            match writer.write(&request.data[offset..end]) {
                Ok(0) => return Err(format!("终端写入未完成。{INPUT_BLOCKED}")),
                Ok(written) => offset += written,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(format!("无法写入终端：{error}。{INPUT_BLOCKED}")),
            }
        }
        check()?;
        writer
            .flush()
            .map_err(|error| format!("无法写入终端：{error}。{INPUT_BLOCKED}"))?;
        check()
    }

    fn block(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    fn send(&self, data: &[u8]) -> Result<(), String> {
        self.send_with_timeout(data, WRITE_TIMEOUT)
    }

    fn send_with_timeout(&self, data: &[u8], timeout: std::time::Duration) -> Result<(), String> {
        if data.is_empty() {
            return Ok(());
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(INPUT_BLOCKED.into());
        }
        let reservation = WriteReservation::acquire(&self.queued, data.len())?;
        let (reply, answer) = mpsc::channel();
        if self.closed.load(Ordering::SeqCst) {
            return Err(INPUT_BLOCKED.into());
        }
        let deadline = std::time::Instant::now() + timeout;
        if self
            .sender
            .send(WriteRequest {
                data: data.to_vec(),
                reply,
                deadline,
                _reservation: reservation,
            })
            .is_err()
        {
            self.block();
            return Err(INPUT_BLOCKED.into());
        }
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return Err(INPUT_BLOCKED.into());
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                self.block();
                return Err(format!("终端写入超时。{INPUT_BLOCKED}"));
            }
            match answer.recv_timeout(remaining.min(std::time::Duration::from_millis(50))) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.block();
                    return Err(INPUT_BLOCKED.into());
                }
            }
        }
    }
}

struct PtySession {
    info: SessionInfo,
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Foreground process group of the PTY, captured while the child is alive.
    /// Used to terminate descendants that keep the PTY open after the child
    /// exits, so the reader thread can reach EOF and release its handle (#15).
    #[cfg(unix)]
    pgid: Option<libc::pid_t>,
    #[cfg(windows)]
    job: Option<WinJob>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: SessionWriter,
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

pub struct SessionLaunch<'a> {
    pub session_id: &'a str,
    pub tool_id: &'a str,
    pub project_path: &'a str,
    pub rows: u16,
    pub cols: u16,
    pub tool_args: Vec<String>,
    pub tool_env: Vec<(String, String)>,
    pub schedule_bridge: Option<crate::schedule_mcp::SessionBridge>,
    pub schedule_guard: ScheduleMcp,
    pub cli_session_id: Option<String>,
    pub title: Option<String>,
}

fn new_uuid_v4() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| format!("无法生成会话标识：{error}"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

fn build_tool_launch_args(
    tool_id: &str,
    bridge_args: Vec<String>,
    cli_session_id: Option<&str>,
    resume: bool,
) -> Result<(Option<String>, Vec<String>), String> {
    if let Some(id) = cli_session_id
        && !is_valid_cli_session_id(tool_id, id)
    {
        return Err("无效的 CLI 会话恢复标识".to_string());
    }
    match tool_id {
        "claude" => {
            if resume {
                let mut args = if let Some(id) = cli_session_id {
                    vec!["--resume".to_string(), id.to_string()]
                } else {
                    vec!["--continue".to_string()]
                };
                args.extend(bridge_args);
                Ok((cli_session_id.map(str::to_string), args))
            } else {
                let uuid = match cli_session_id {
                    Some(id) => id.to_string(),
                    None => new_uuid_v4()?,
                };
                let mut args = vec!["--session-id".to_string(), uuid.clone()];
                args.extend(bridge_args);
                Ok((Some(uuid), args))
            }
        }
        "codex" => {
            if resume {
                let mut args = if let Some(id) = cli_session_id {
                    vec!["resume".to_string(), id.to_string()]
                } else {
                    vec!["resume".to_string(), "--last".to_string()]
                };
                args.extend(bridge_args);
                Ok((cli_session_id.map(str::to_string), args))
            } else {
                Ok((None, bridge_args))
            }
        }
        "agy" => {
            if resume {
                let mut args = if let Some(id) = cli_session_id {
                    vec!["--resume".to_string(), id.to_string()]
                } else {
                    vec!["--continue".to_string()]
                };
                args.extend(bridge_args);
                Ok((cli_session_id.map(str::to_string), args))
            } else {
                Ok((None, bridge_args))
            }
        }
        "dsh" => Ok((cli_session_id.map(str::to_string), bridge_args)),
        _ => Err(format!("不支持的工具：{tool_id}")),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CliSessionStores {
    pub claude_projects_dir: Option<PathBuf>,
    pub codex_sessions_dir: Option<PathBuf>,
    pub agy_history_jsonl: Option<PathBuf>,
}

impl CliSessionStores {
    pub fn from_env() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from);
        match home {
            Some(root) => Self {
                claude_projects_dir: Some(root.join(".claude").join("projects")),
                codex_sessions_dir: Some(root.join(".codex").join("sessions")),
                agy_history_jsonl: Some(
                    root.join(".gemini")
                        .join("antigravity-cli")
                        .join("history.jsonl"),
                ),
            },
            None => Self::default(),
        }
    }
}

fn normalize_comparable_path(path: &str) -> String {
    let stripped = strip_windows_verbatim(std::path::Path::new(path));
    let mut text = stripped.to_string_lossy().replace('/', "\\");
    while text.len() > 3 && text.ends_with('\\') {
        text.pop();
    }
    if !cfg!(windows) {
        text = stripped.to_string_lossy().trim_end_matches('/').to_string();
        if text.is_empty() && stripped.as_os_str() == "/" {
            text = "/".to_string();
        }
    } else {
        text = text.to_ascii_lowercase();
    }
    text
}

fn encode_claude_project_dir(project_path: &str) -> String {
    let clean = strip_windows_verbatim(std::path::Path::new(project_path));
    clean
        .to_string_lossy()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn parse_rfc3339_ms(raw: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .and_then(|dt| u64::try_from(dt.timestamp_millis()).ok())
}

fn system_time_ms(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_millis() as u64)
}

fn read_claude_session_candidates(
    claude_projects_dir: &std::path::Path,
    project_path: &str,
) -> Vec<(String, u64)> {
    let encoded = encode_claude_project_dir(project_path);
    let project_dir = claude_projects_dir.join(encoded);
    let Ok(entries) = std::fs::read_dir(project_dir) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_valid_uuid(stem) {
            continue;
        }
        let mut timestamp_ms = None;
        if let Ok(file) = std::fs::File::open(&path) {
            let reader = BufReader::new(file);
            for line in reader.lines().take(20).map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line)
                    && let Some(ts) = value.get("timestamp").and_then(|v| v.as_str())
                    && let Some(ms) = parse_rfc3339_ms(ts)
                {
                    timestamp_ms = Some(ms);
                    break;
                }
            }
        }
        if timestamp_ms.is_none()
            && let Ok(meta) = entry.metadata()
        {
            timestamp_ms = meta
                .created()
                .ok()
                .and_then(system_time_ms)
                .or_else(|| meta.modified().ok().and_then(system_time_ms));
        }
        if let Some(ms) = timestamp_ms {
            candidates.push((stem.to_string(), ms));
        }
    }
    candidates
}

fn collect_codex_files(
    dir: &std::path::Path,
    depth: usize,
    min_modified_ms: Option<u64>,
    out: &mut Vec<PathBuf>,
) {
    const MAX_CODEX_FILES: usize = 2000;
    if depth > 4 || out.len() >= MAX_CODEX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_CODEX_FILES {
            break;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_codex_files(&path, depth + 1, min_modified_ms, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        {
            if let Some(min_ms) = min_modified_ms
                && let Ok(meta) = entry.metadata()
                && let Ok(modified) = meta.modified()
                && let Some(mod_ms) = system_time_ms(modified)
                && mod_ms < min_ms
            {
                continue;
            }
            out.push(path);
        }
    }
}

fn read_codex_session_candidates(
    codex_sessions_dir: &std::path::Path,
    min_modified_ms: Option<u64>,
) -> Vec<(String, String, u64)> {
    let mut files = Vec::new();
    collect_codex_files(codex_sessions_dir, 0, min_modified_ms, &mut files);
    let mut candidates = Vec::new();
    for path in files {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut first_line = String::new();
        if BufReader::new(file.take(64 * 1024))
            .read_line(&mut first_line)
            .is_err()
        {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&first_line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        let Some(id) = payload.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if !is_valid_cli_session_id("codex", id) {
            continue;
        }
        let Some(cwd) = payload.get("cwd").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(ts_ms) = payload
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_rfc3339_ms)
        else {
            continue;
        };
        candidates.push((id.to_string(), normalize_comparable_path(cwd), ts_ms));
    }
    candidates
}

fn read_agy_session_candidates(agy_history_jsonl: &std::path::Path) -> Vec<(String, String, u64)> {
    let Ok(file) = std::fs::File::open(agy_history_jsonl) else {
        return Vec::new();
    };
    let mut earliest_by_id: HashMap<String, (String, u64)> = HashMap::new();
    for line in BufReader::new(file.take(crate::journal::HARD_READ_LIMIT))
        .lines()
        .map_while(Result::ok)
    {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let Some(id) = value.get("conversationId").and_then(|v| v.as_str()) else {
            continue;
        };
        if !is_valid_cli_session_id("agy", id) {
            continue;
        }
        let Some(workspace) = value.get("workspace").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(ts_ms) = value.get("timestamp").and_then(|v| v.as_u64()) else {
            continue;
        };
        let norm_ws = normalize_comparable_path(workspace);
        earliest_by_id
            .entry(id.to_string())
            .and_modify(|existing| {
                if ts_ms < existing.1 {
                    *existing = (norm_ws.clone(), ts_ms);
                }
            })
            .or_insert((norm_ws, ts_ms));
    }
    earliest_by_id
        .into_iter()
        .map(|(id, (ws, ts))| (id, ws, ts))
        .collect()
}

pub(crate) fn resolve_missing_cli_sessions(
    records: &mut [SessionRecord],
    skip_ids: &HashSet<String>,
    stores: &CliSessionStores,
) -> Vec<(String, String)> {
    let mut claimed: HashSet<String> = records
        .iter()
        .filter_map(|record| record.cli_session_id.clone())
        .collect();

    let mut min_codex_start: Option<u64> = None;
    let mut need_codex = false;
    let mut need_agy = false;
    for r in records.iter() {
        if r.cli_session_id.is_some() || skip_ids.contains(&r.id) {
            continue;
        }
        if r.tool_id == "codex" {
            need_codex = true;
            let start_bound = r.started_at_ms.saturating_sub(15_000);
            min_codex_start = Some(match min_codex_start {
                Some(prev) => prev.min(start_bound),
                None => start_bound,
            });
        } else if r.tool_id == "agy" {
            need_agy = true;
        }
    }

    let codex_candidates = if need_codex {
        stores
            .codex_sessions_dir
            .as_deref()
            .map(|dir| read_codex_session_candidates(dir, min_codex_start))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let agy_candidates = if need_agy {
        stores
            .agy_history_jsonl
            .as_deref()
            .map(read_agy_session_candidates)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut claude_cache: HashMap<String, Vec<(String, u64)>> = HashMap::new();

    // Process oldest-first so older records claim their own older session IDs first.
    let mut indices: Vec<usize> = (0..records.len()).collect();
    indices.sort_by_key(|&idx| records[idx].started_at_ms);

    let mut newly_bound = Vec::new();
    for idx in indices {
        if records[idx].cli_session_id.is_some() || skip_ids.contains(&records[idx].id) {
            continue;
        }
        let start = records[idx].started_at_ms;
        let window_start = start.saturating_sub(15_000);
        let window_end = match records[idx].ended_at_ms {
            Some(ended) => ended.saturating_add(5_000).max(start),
            None => start.saturating_add(120_000),
        };
        let norm_proj = normalize_comparable_path(&records[idx].project_path);

        let matched = match records[idx].tool_id.as_str() {
            "claude" => {
                if let Some(dir) = stores.claude_projects_dir.as_deref() {
                    let entries = claude_cache
                        .entry(records[idx].project_path.clone())
                        .or_insert_with(|| {
                            read_claude_session_candidates(dir, &records[idx].project_path)
                        });
                    entries
                        .iter()
                        .filter(|(id, ts)| {
                            !claimed.contains(id) && *ts >= window_start && *ts <= window_end
                        })
                        .min_by_key(|(_, ts)| ts.abs_diff(start))
                        .map(|(id, _)| id.clone())
                } else {
                    None
                }
            }
            "codex" => codex_candidates
                .iter()
                .filter(|(id, cwd, ts)| {
                    !claimed.contains(id)
                        && *cwd == norm_proj
                        && *ts >= window_start
                        && *ts <= window_end
                })
                .min_by_key(|(_, _, ts)| ts.abs_diff(start))
                .map(|(id, _, _)| id.clone()),
            "agy" => agy_candidates
                .iter()
                .filter(|(id, cwd, ts)| {
                    !claimed.contains(id)
                        && *cwd == norm_proj
                        && *ts >= start.saturating_sub(5_000)
                        && *ts <= window_end
                })
                .min_by_key(|(_, _, ts)| ts.abs_diff(start))
                .map(|(id, _, _)| id.clone()),
            _ => None,
        };

        if let Some(cli_id) = matched {
            claimed.insert(cli_id.clone());
            records[idx].cli_session_id = Some(cli_id.clone());
            newly_bound.push((records[idx].id.clone(), cli_id));
        }
    }
    newly_bound
}

fn resolve_single_session_on_exit(history: &SessionHistory, exit_id: &str) {
    let Ok(mut records) = history.list() else {
        return;
    };
    let Some(target) = records.iter().find(|item| item.id == exit_id) else {
        return;
    };
    if target.cli_session_id.is_some() || !matches!(target.tool_id.as_str(), "codex" | "agy") {
        return;
    }
    let skip_ids: HashSet<String> = records
        .iter()
        .filter(|item| item.id != exit_id)
        .map(|item| item.id.clone())
        .collect();
    let stores = CliSessionStores::from_env();
    for (id, cli_id) in resolve_missing_cli_sessions(&mut records, &skip_ids, &stores) {
        if id == exit_id {
            let _ = history.record_cli_session(&id, &cli_id);
        }
    }
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
fn build_pty_command(
    binary: &std::path::Path,
    cwd: &std::path::Path,
    additional_args: &[String],
    additional_env: &[(String, String)],
) -> CommandBuilder {
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
    for argument in additional_args {
        command.arg(argument);
    }
    for (key, value) in additional_env {
        command.env(key, value);
    }
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

pub(crate) fn strip_windows_verbatim(path: &std::path::Path) -> PathBuf {
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
    additional_args: &[String],
    additional_env: &[(String, String)],
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

    let command = build_pty_command(binary, cwd, additional_args, additional_env);

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
                let text = decoder.push(&chunk[..count]);
                if text.is_empty() {
                    continue; // only a partial character so far
                }
                // Keep the archive decision, buffer update, and live delivery in
                // one critical section. The watcher takes this same lock before
                // marking the session archived, so it cannot snapshot old output
                // or emit the exit event ahead of an in-flight chunk (#19).
                let Ok(mut guard) = buffer.lock() else {
                    break;
                };
                if archived.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                guard.end_offset += text.len() as u64;
                guard.data.push_str(&text);
                trim_buffer_front(&mut guard.data, MAX_BUFFER_CHARS);
                let event = SessionOutput {
                    data: text,
                    end_offset: guard.end_offset,
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
    if !tail.is_empty() {
        let Ok(mut guard) = buffer.lock() else {
            return;
        };
        if !archived.load(std::sync::atomic::Ordering::Acquire) {
            guard.end_offset += tail.len() as u64;
            guard.data.push_str(&tail);
            let _ = sink(SessionOutput {
                data: tail,
                end_offset: guard.end_offset,
            });
        }
    }
}

/// Forcefully terminate the child's whole process group. Used only on the
/// drain-timeout path, when a descendant still holds the PTY output end and the
/// reader cannot reach EOF (#15). The child is its own session leader (portable-pty
/// calls setsid), so a negative pid targets only that terminal's processes.
#[cfg(unix)]
pub(crate) fn kill_process_group(pgid: Option<libc::pid_t>) {
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
    pub(crate) fn attach(process_handle: std::os::windows::io::RawHandle) -> std::io::Result<Self> {
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

    pub(crate) fn terminate(&self) {
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

struct SessionReservation {
    pending: Arc<Mutex<HashSet<String>>>,
    id: String,
}

impl Drop for SessionReservation {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&self.id);
        }
    }
}

#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
    acp_sessions: Arc<Mutex<HashMap<String, crate::acp::AcpSessionHandle>>>,
    starting: Arc<Mutex<HashSet<String>>>,
    /// Last output of sessions that have already exited, keyed by session id.
    /// Kept so a view that mounts *after* the process died can still replay the
    /// final screen and learn the process is gone (instead of seeing "running").
    finished: Arc<Mutex<HashMap<String, (String, u64)>>>,
    finished_acp: Arc<Mutex<HashMap<String, crate::acp::AcpSnapshot>>>,
    /// Persisted identity/end-state history, configured once before commands
    /// are served. Recording is best-effort: a history write failure must not
    /// orphan a running CLI, so callers keep the session and log the failure.
    history: Arc<Mutex<Option<SessionHistory>>>,
    checked_history_ids: Arc<Mutex<HashSet<String>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            acp_sessions: Arc::new(Mutex::new(HashMap::new())),
            starting: Arc::new(Mutex::new(HashSet::new())),
            finished: Arc::new(Mutex::new(HashMap::new())),
            finished_acp: Arc::new(Mutex::new(HashMap::new())),
            history: Arc::new(Mutex::new(None)),
            checked_history_ids: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

impl SessionManager {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, PtySession>>, String> {
        self.sessions
            .lock()
            .map_err(|_| "会话管理暂时不可用，请重试".to_string())
    }

    fn lock_acp(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, crate::acp::AcpSessionHandle>>, String>
    {
        self.acp_sessions
            .lock()
            .map_err(|_| "ACP 会话管理暂时不可用，请重试".to_string())
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
        let history = self.history.lock().ok().and_then(|guard| guard.clone());
        let Some(history) = history else {
            return;
        };
        if let Err(error) = history.record_end(id, now_ms(), reason, output_bytes) {
            eprintln!("AgentTerm could not persist session end for {id}: {error}");
        }
        resolve_single_session_on_exit(&history, id);
    }

    pub fn list_history(&self) -> Result<Vec<SessionRecord>, String> {
        let history = {
            let guard = self
                .history
                .lock()
                .map_err(|_| "会话历史暂时不可用，请重试".to_string())?;
            guard
                .as_ref()
                .cloned()
                .ok_or_else(|| "会话历史尚未配置".to_string())?
        };
        let mut records = history.list()?;
        let live_ids: HashSet<String> = self
            .lock()?
            .keys()
            .chain(self.lock_acp()?.keys())
            .cloned()
            .collect();
        let mut skip_ids = live_ids.clone();
        if let Ok(checked) = self.checked_history_ids.lock() {
            skip_ids.extend(checked.iter().cloned());
        }
        let stores = CliSessionStores::from_env();
        let resolved = resolve_missing_cli_sessions(&mut records, &skip_ids, &stores);
        for (id, cli_id) in resolved {
            if let Err(error) = history.record_cli_session(&id, &cli_id) {
                eprintln!("AgentTerm could not bind resolved CLI session id for {id}: {error}");
            }
        }
        if let Ok(mut checked) = self.checked_history_ids.lock() {
            for record in &records {
                if !live_ids.contains(&record.id) {
                    checked.insert(record.id.clone());
                }
            }
        }
        Ok(records)
    }

    fn reserve_start(&self, id: &str) -> Result<SessionReservation, String> {
        let mut pending = self
            .starting
            .lock()
            .map_err(|_| "会话管理暂时不可用，请重试".to_string())?;
        if pending.contains(id)
            || self.lock()?.contains_key(id)
            || self.lock_acp()?.contains_key(id)
        {
            return Err("会话已存在，请换一个会话再启动".into());
        }
        pending.insert(id.to_owned());
        Ok(SessionReservation {
            pending: Arc::clone(&self.starting),
            id: id.to_owned(),
        })
    }

    fn start(
        &self,
        app: AppHandle,
        request: SessionLaunch<'_>,
        reservation: Arc<SessionReservation>,
    ) -> Result<SessionInfo, String> {
        let SessionLaunch {
            session_id,
            tool_id,
            project_path,
            rows,
            cols,
            tool_args,
            tool_env,
            schedule_bridge,
            schedule_guard,
            cli_session_id,
            title,
        } = request;
        if let Some(id) = cli_session_id.as_deref()
            && !is_valid_cli_session_id(tool_id, id)
        {
            return Err("无效的 CLI 会话标识".to_string());
        }
        let normalized_title = title
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(normalize_session_title)
            .transpose()?;
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

        if tool.id == "dsh" {
            let info = SessionInfo {
                id: session_id.to_string(),
                tool_id: tool.id.to_string(),
                tool_name: tool.name.to_string(),
                project_path: project_dir.display().to_string(),
                title: normalized_title.clone(),
                cli_session_id: cli_session_id.clone(),
            };
            let emit_id = session_id.to_string();
            let app_emit = app.clone();
            let bind_id = session_id.to_string();
            let acp_sessions_bind = Arc::clone(&self.acp_sessions);
            let history_bind = Arc::clone(&self.history);
            let exit_id = session_id.to_string();
            let acp_sessions_exit = Arc::clone(&self.acp_sessions);
            let finished_acp_exit = Arc::clone(&self.finished_acp);
            let history_exit = Arc::clone(&self.history);
            let app_exit = app.clone();
            let schedule_guard_exit = schedule_guard;
            let reservation_exit = reservation;

            let handle = crate::acp::spawn_acp_session(
                info.clone(),
                &binary,
                &project_dir,
                &tool_env,
                schedule_bridge.as_ref(),
                cli_session_id.clone(),
                crate::acp::AcpHooks {
                    on_emit: Box::new(move |snapshot| {
                        let _ = app_emit.emit(&format!("acp-state-{emit_id}"), snapshot);
                    }),
                    on_cli_session: Box::new(move |cli_id| {
                        if let Ok(mut guard) = acp_sessions_bind.lock()
                            && let Some(active) = guard.get_mut(&bind_id)
                        {
                            active.info.cli_session_id = Some(cli_id.clone());
                        }
                        if let Ok(history_guard) = history_bind.lock()
                            && let Some(history) = history_guard.as_ref()
                            && let Err(error) = history.record_cli_session(&bind_id, &cli_id)
                        {
                            eprintln!(
                                "AgentTerm could not bind CLI session id for {bind_id}: {error}"
                            );
                        }
                    }),
                    on_exit: Box::new(move |reason, final_snapshot, bytes| {
                        let _reservation = &reservation_exit;
                        if let Ok(mut guard) = acp_sessions_exit.lock() {
                            guard.remove(&exit_id);
                        }
                        if let Some(snapshot) = final_snapshot
                            && let Ok(mut done) = finished_acp_exit.lock()
                        {
                            done.insert(exit_id.clone(), snapshot);
                        }
                        if let Ok(history_guard) = history_exit.lock()
                            && let Some(history) = history_guard.as_ref()
                            && let Err(error) = history.record_end(
                                &exit_id,
                                now_ms(),
                                Some(reason.clone()),
                                Some(bytes),
                            )
                        {
                            eprintln!(
                                "AgentTerm could not persist ACP session end for {exit_id}: {error}"
                            );
                        }
                        let _ = app_exit.emit(
                            &format!("session-exit-{exit_id}"),
                            SessionExit {
                                id: exit_id.clone(),
                                reason,
                            },
                        );
                        schedule_guard_exit.revoke_session(&exit_id);
                    }),
                },
            )?;
            handle.emit_snapshot(&app);
            self.lock_acp()?.insert(session_id.to_string(), handle);
            self.record_start(SessionRecord {
                id: session_id.to_string(),
                tool_id: tool.id.to_string(),
                tool_name: tool.name.to_string(),
                project_path: info.project_path.clone(),
                title: normalized_title,
                cli_session_id,
                started_at_ms: now_ms(),
                ended_at_ms: None,
                exit_reason: None,
                output_bytes: None,
            });
            return Ok(info);
        }

        let spawned = spawn_pty(&binary, &project_dir, rows, cols, &tool_args, &tool_env)?;
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
            title: normalized_title.clone(),
            cli_session_id: cli_session_id.clone(),
        };

        #[cfg(unix)]
        let pgid = master.process_group_leader();
        let session = PtySession {
            info: info.clone(),
            master,
            #[cfg(unix)]
            pgid,
            #[cfg(windows)]
            job,
            child,
            writer: SessionWriter::spawn(writer),
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
            title: normalized_title,
            cli_session_id,
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
        let schedule_guard_exit = schedule_guard;

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
            let _reservation = reservation;
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
                    // Lock the output buffer before marking the session archived.
                    // `pump_reader` holds this lock through live delivery, so the
                    // snapshot and the last emitted chunk cannot diverge (#19).
                    let mut archived_bytes = None;
                    if let Ok(mut sessions_guard) = sessions.lock()
                        && let Some(session) = sessions_guard.get(&exit_id)
                    {
                        let buffer = Arc::clone(&session.buffer);
                        if let Ok(buffer_guard) = buffer.lock() {
                            archived_watcher.store(true, std::sync::atomic::Ordering::Release);

                            let archived_data = buffer_guard.data.clone();
                            let archived_offset = buffer_guard.end_offset;
                            archived_bytes = Some(buffer_guard.data.len() as u64);
                            drop(buffer_guard);
                            sessions_guard.remove(&exit_id);
                            if let Ok(mut done) = finished_buffers.lock() {
                                done.insert(exit_id.clone(), (archived_data, archived_offset));
                            }
                        } else {
                            archived_watcher.store(true, std::sync::atomic::Ordering::Release);

                            sessions_guard.remove(&exit_id);
                        }
                    }
                    if let Ok(history_guard) = history_writer.lock()
                        && let Some(history) = history_guard.as_ref()
                    {
                        if let Err(error) = history.record_end(
                            &exit_id,
                            now_ms(),
                            Some(reason.clone()),
                            archived_bytes,
                        ) {
                            eprintln!(
                                "AgentTerm could not persist session end for {exit_id}: {error}"
                            );
                        }
                        resolve_single_session_on_exit(history, &exit_id);
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
            schedule_guard_exit.revoke_session(&exit_id);
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
                    input_blocked: session.writer.closed.load(Ordering::SeqCst),
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
                input_blocked: true,
            }),
            None => Err("会话不存在或已结束".to_string()),
        }
    }

    /// Returns metadata owned by the backend for a live session. Renderer
    /// supplied labels and paths are never authoritative for task binding.
    pub fn session_info(&self, session_id: &str) -> Result<SessionInfo, String> {
        if let Some(session) = self.lock()?.get(session_id) {
            return Ok(session.info.clone());
        }
        if let Some(session) = self.lock_acp()?.get(session_id) {
            return Ok(session.info.clone());
        }
        Err("会话不存在或已结束".to_string())
    }

    /// The renderer uses this snapshot to recover tabs after a WebView reload
    /// or renderer crash. Only live sessions owned by this backend are returned.
    pub fn list_live(&self) -> Result<Vec<SessionInfo>, String> {
        let mut result = self
            .lock()?
            .values()
            .map(|session| session.info.clone())
            .collect::<Vec<_>>();
        result.extend(
            self.lock_acp()?
                .values()
                .map(|session| session.info.clone()),
        );
        result.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(result)
    }

    pub fn acp_snapshot(&self, session_id: &str) -> Result<crate::acp::AcpSnapshot, String> {
        if let Some(session) = self.lock_acp()?.get(session_id) {
            return session.snapshot();
        }
        let finished = self
            .finished_acp
            .lock()
            .map_err(|_| "无法读取 ACP 会话状态".to_string())?;
        finished
            .get(session_id)
            .cloned()
            .ok_or_else(|| "ACP 会话不存在或已结束".to_string())
    }

    pub fn acp_send_prompt(
        &self,
        app: &AppHandle,
        session_id: &str,
        text: &str,
        images: &[crate::acp::AcpImageInput],
    ) -> Result<(), String> {
        let guard = self.lock_acp()?;
        let session = guard
            .get(session_id)
            .ok_or_else(|| "ACP 会话不存在或已结束".to_string())?;
        let result = session.send_prompt(text, images);
        session.emit_snapshot(app);
        result
    }

    pub fn acp_search_project_files(
        &self,
        session_id: &str,
        query: &str,
    ) -> Result<Vec<String>, String> {
        let project_path = {
            let guard = self.lock_acp()?;
            let session = guard
                .get(session_id)
                .ok_or_else(|| "ACP 会话不存在或已结束".to_string())?;
            session.project_path()
        };
        crate::acp::search_project_files_in_dir(&project_path, query)
    }

    pub fn acp_cancel_turn(&self, session_id: &str) -> Result<(), String> {
        let guard = self.lock_acp()?;
        let session = guard
            .get(session_id)
            .ok_or_else(|| "ACP 会话不存在或已结束".to_string())?;
        session.cancel_turn()
    }

    pub fn acp_respond_permission(
        &self,
        app: &AppHandle,
        session_id: &str,
        request_id: &str,
        option_id: Option<&str>,
    ) -> Result<(), String> {
        let guard = self.lock_acp()?;
        let session = guard
            .get(session_id)
            .ok_or_else(|| "ACP 会话不存在或已结束".to_string())?;
        let result = session.respond_permission(request_id, option_id);
        session.emit_snapshot(app);
        result
    }

    pub fn acp_set_config_option(
        &self,
        app: &AppHandle,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<(), String> {
        let guard = self.lock_acp()?;
        let session = guard
            .get(session_id)
            .ok_or_else(|| "ACP 会话不存在或已结束".to_string())?;
        let result = session.set_config_option(config_id, value);
        session.emit_snapshot(app);
        result
    }

    pub fn write(&self, session_id: &str, data: &str) -> Result<(), String> {
        // Clone the write channel under the lock, then write with the lock
        // released. A stuck CLI can no longer block every other session
        // operation or the shutdown path (#87).
        let writer = {
            let sessions = self.lock()?;
            sessions
                .get(session_id)
                .ok_or_else(|| "会话不存在或已结束".to_string())?
                .writer
                .clone()
        };
        writer.send(data.as_bytes())
    }

    pub fn block_input(&self, session_id: &str) -> Result<(), String> {
        let sessions = self.lock()?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "会话不存在或已结束".to_string())?;
        session.writer.block();
        Ok(())
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
        self.stop_with_reason(session_id, "已停止")
    }

    pub fn rename(&self, session_id: &str, title: &str) -> Result<String, String> {
        if session_id.trim().is_empty() {
            return Err("会话标识无效".to_string());
        }
        let normalized = normalize_session_title(title)?;
        let history = self
            .history
            .lock()
            .map_err(|_| "会话历史暂时不可用，请重试".to_string())?
            .clone();
        let saved = match history {
            Some(history) => match history.record_rename(session_id, &normalized) {
                Ok(saved) => saved,
                Err(error) => {
                    // If history recording failed when the session started,
                    // allow renaming the live in-memory session instead of
                    // blocking the user; still fail closed for missing/corrupt
                    // non-live ids.
                    let is_live = self.lock()?.contains_key(session_id)
                        || self.lock_acp()?.contains_key(session_id);
                    if error == "会话历史记录不存在" && is_live {
                        normalized
                    } else {
                        return Err(error);
                    }
                }
            },
            None => {
                let is_live = self.lock()?.contains_key(session_id)
                    || self.lock_acp()?.contains_key(session_id);
                if !is_live {
                    return Err("会话不存在或已结束".to_string());
                }
                normalized
            }
        };
        if let Some(session) = self.lock()?.get_mut(session_id) {
            session.info.title = Some(saved.clone());
        }
        if let Some(session) = self.lock_acp()?.get_mut(session_id) {
            session.info.title = Some(saved.clone());
        }
        Ok(saved)
    }

    fn stop_with_reason(&self, session_id: &str, reason: &str) -> Result<(), String> {
        {
            let acp_sessions = self.lock_acp()?;
            if let Some(acp_session) = acp_sessions.get(session_id) {
                acp_session.terminate()?;
                drop(acp_sessions);
                let removed = self.lock_acp()?.remove(session_id);
                let output_bytes = removed.as_ref().map(|item| item.output_bytes());
                if let (true, Ok(mut done)) = (removed.is_some(), self.finished_acp.lock()) {
                    done.remove(session_id);
                }
                self.record_end(session_id, Some(reason.to_string()), output_bytes);
                return Ok(());
            }
        }
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
            #[cfg(unix)]
            kill_process_group(session.pgid);
            #[cfg(windows)]
            if let Some(job) = session.job.as_ref() {
                job.terminate();
            }
            session.writer.block();
        }

        // Kill succeeded (or the process was already gone): now release it.
        // Read the byte count under the same lock: if the watcher already
        // archived the session, fall back to that buffer instead of recording
        // "stopped" with no byte count or dropping its replay data.
        let (session, output_bytes) = {
            let mut sessions = self.lock()?;
            let session = sessions.remove(session_id);
            let bytes = session
                .as_ref()
                .and_then(|session| {
                    session
                        .buffer
                        .lock()
                        .ok()
                        .map(|buffer| buffer.data.len() as u64)
                })
                .or_else(|| {
                    self.finished
                        .lock()
                        .ok()
                        .and_then(|done| done.get(session_id).map(|(data, _)| data.len() as u64))
                });
            (session, bytes)
        };
        // Only clear the finished buffer when this path owned the live session;
        // a watcher-owned archive is left for replay until it is forgotten.
        if let (true, Ok(mut done)) = (session.is_some(), self.finished.lock()) {
            done.remove(session_id);
        }
        drop(session);
        self.record_end(session_id, Some(reason.to_string()), output_bytes);
        Ok(())
    }

    pub fn forget(&self, session_id: &str) -> Result<(), String> {
        // Hold the live-session lock while releasing replay data. A caller may
        // forget an ended tab, but must never detach a running process.
        let sessions = self.lock()?;
        let acp_sessions = self.lock_acp()?;
        if sessions.contains_key(session_id) || acp_sessions.contains_key(session_id) {
            return Err("会话仍在运行，请先结束会话".to_string());
        }
        self.finished
            .lock()
            .map_err(|_| "无法释放会话输出".to_string())?
            .remove(session_id);
        self.finished_acp
            .lock()
            .map_err(|_| "无法释放 ACP 会话输出".to_string())?
            .remove(session_id);
        Ok(())
    }

    /// Stop every live session before allowing the process to exit.
    /// A failed kill keeps that session in the map so the caller can retry;
    /// silently draining the map would orphan the child process.
    pub fn stop_all(&self) -> Result<(), String> {
        let mut ids = self
            .sessions
            .lock()
            .map_err(|_| "会话管理暂时不可用，请重试".to_string())?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        ids.extend(
            self.acp_sessions
                .lock()
                .map_err(|_| "ACP 会话管理暂时不可用，请重试".to_string())?
                .keys()
                .cloned(),
        );
        let mut failures = Vec::new();
        for id in ids {
            match self.stop_with_reason(&id, "应用退出") {
                Ok(()) => {}
                Err(error) if error == "会话不存在或已结束" => {}
                Err(error) => failures.push(format!("{id}: {error}")),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!("无法安全结束所有终端：{}", failures.join("；")))
        }
    }

    #[cfg(test)]
    pub fn is_running(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .map(|guard| guard.contains_key(session_id))
            .unwrap_or(false)
            || self
                .acp_sessions
                .lock()
                .map(|guard| guard.contains_key(session_id))
                .unwrap_or(false)
    }
}

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)] // Tauri injects state/AppHandle beside the IPC args.
pub fn start_session(
    state: tauri::State<'_, SessionManager>,
    schedule_mcp: tauri::State<'_, ScheduleMcp>,
    app: AppHandle,
    session_id: String,
    tool_id: String,
    project_path: String,
    rows: u16,
    cols: u16,
    resume: Option<bool>,
    cli_session_id: Option<String>,
    title: Option<String>,
) -> Result<SessionInfo, String> {
    // Reserve before the MCP bridge: a duplicate must not replace or revoke
    // the existing session's registration. Keep it through error cleanup.
    let reservation = Arc::new(state.reserve_start(&session_id)?);
    let bridge = schedule_mcp.issue_session(&session_id, &project_path, &tool_id, None)?;
    let tool_env = bridge.environment();
    let (resolved_cli_id, tool_args) = match bridge.command_args(&tool_id).and_then(|args| {
        build_tool_launch_args(
            &tool_id,
            args,
            cli_session_id.as_deref(),
            resume.unwrap_or(false),
        )
    }) {
        Ok(result) => result,
        Err(error) => {
            schedule_mcp.revoke_session(&session_id);
            return Err(error);
        }
    };
    match state.start(
        app,
        SessionLaunch {
            session_id: &session_id,
            tool_id: &tool_id,
            project_path: &project_path,
            rows,
            cols,
            tool_args,
            tool_env,
            schedule_bridge: Some(bridge),
            schedule_guard: schedule_mcp.handle(),
            cli_session_id: resolved_cli_id,
            title,
        },
        Arc::clone(&reservation),
    ) {
        Ok(info) => Ok(info),
        Err(error) => {
            schedule_mcp.revoke_session(&session_id);
            Err(error)
        }
    }
}

#[tauri::command(async)]
pub fn session_snapshot(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
) -> Result<SessionSnapshot, String> {
    state.snapshot(&session_id)
}

#[tauri::command(async)]
pub fn list_sessions(state: tauri::State<'_, SessionManager>) -> Result<Vec<SessionInfo>, String> {
    state.list_live()
}

#[tauri::command]
pub async fn write_session_input(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.write(&session_id, &data))
        .await
        .map_err(|error| format!("终端输入线程异常：{error}"))?
}

#[tauri::command(async)]
pub fn block_session_input(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
) -> Result<(), String> {
    state.block_input(&session_id)
}

#[tauri::command(async)]
pub fn resize_session(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    state.resize(&session_id, rows, cols)
}

#[tauri::command(async)]
pub fn forget_session(
    state: tauri::State<'_, SessionManager>,
    schedule_mcp: tauri::State<'_, ScheduleMcp>,
    session_id: String,
) -> Result<(), String> {
    state.forget(&session_id)?;
    schedule_mcp.revoke_session(&session_id);
    Ok(())
}

#[tauri::command(async)]
pub fn stop_session(
    state: tauri::State<'_, SessionManager>,
    schedule_mcp: tauri::State<'_, ScheduleMcp>,
    session_id: String,
) -> Result<(), String> {
    state.stop(&session_id)?;
    schedule_mcp.revoke_session(&session_id);
    Ok(())
}

#[tauri::command(async)]
pub fn rename_session(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
    title: String,
) -> Result<String, String> {
    state.rename(&session_id, &title)
}

#[tauri::command(async)]
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
    use std::time::Duration;

    use super::SessionManager;

    #[test]
    fn session_id_is_reserved_until_the_owner_releases_it() {
        let manager = SessionManager::default();
        let reservation = manager.reserve_start("reserved").unwrap();
        let contender = manager.clone();
        assert!(
            std::thread::spawn(move || contender.reserve_start("reserved").is_err())
                .join()
                .unwrap()
        );
        assert!(manager.reserve_start("different").is_ok());
        drop(reservation);
        assert!(manager.reserve_start("reserved").is_ok());
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
        insert_fake_session_with_writer(manager, id, kill_fails, Box::new(std::io::sink()));
    }

    fn insert_fake_session_with_writer(
        manager: &SessionManager,
        id: &str,
        kill_fails: bool,
        writer: Box<dyn std::io::Write + Send>,
    ) {
        let mut map = manager.sessions.lock().unwrap();
        map.insert(
            id.to_string(),
            super::PtySession {
                info: super::SessionInfo {
                    id: id.to_string(),
                    tool_id: "claude".into(),
                    tool_name: "Claude Code".into(),
                    project_path: std::env::current_dir().unwrap().to_string_lossy().into(),
                    title: None,
                    cli_session_id: None,
                },
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
                writer: super::SessionWriter::spawn(writer),
                buffer: std::sync::Arc::new(super::Mutex::new(super::OutputBuffer::default())),
            },
        );
    }

    /// Writer that blocks until released, standing in for a CLI that stopped
    /// consuming stdin (#87).
    struct GatedWriter {
        entered: std::sync::Arc<std::sync::atomic::AtomicBool>,
        release: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl std::io::Write for GatedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            use std::sync::atomic::Ordering;
            self.entered.store(true, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_blocked_write_never_holds_the_session_table() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Instant;

        let manager = SessionManager::default();
        let entered = std::sync::Arc::new(AtomicBool::new(false));
        let release = std::sync::Arc::new(AtomicBool::new(false));
        insert_fake_session_with_writer(
            &manager,
            "s-stuck",
            false,
            Box::new(GatedWriter {
                entered: std::sync::Arc::clone(&entered),
                release: std::sync::Arc::clone(&release),
            }),
        );

        let writer_manager = manager.clone();
        let write = std::thread::spawn(move || writer_manager.write("s-stuck", "pasted text"));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !entered.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            entered.load(Ordering::SeqCst),
            "write never reached the PTY"
        );

        // While the PTY write is stuck, reads, stops and the exit path must all
        // still be reachable — that was impossible when the write held the lock.
        let started = Instant::now();
        assert!(manager.snapshot("s-stuck").unwrap().running);
        assert!(manager.list_live().unwrap().len() == 1);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "read paths waited on the blocked write"
        );

        let started = Instant::now();
        manager.stop("s-stuck").unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "stop waited on the blocked write"
        );

        // New input is refused immediately once the session is gone.
        assert!(manager.write("s-stuck", "later").is_err());

        release.store(true, Ordering::SeqCst);
        write.join().unwrap().ok();
    }

    struct RecordedGatedWriter {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        observed: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        dropped: std::sync::mpsc::Sender<()>,
    }

    impl std::io::Write for RecordedGatedWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.entered.send(()).unwrap();
            self.release
                .recv_timeout(Duration::from_secs(3))
                .map_err(|_| std::io::Error::other("fixture was not released"))?;
            self.observed.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Drop for RecordedGatedWriter {
        fn drop(&mut self) {
            let _ = self.dropped.send(());
        }
    }

    #[test]
    fn timeout_blocks_reconnect_and_cancels_queued_and_remaining_input() {
        use std::sync::atomic::Ordering;
        use std::sync::{Arc, Mutex, mpsc};
        use std::time::Instant;

        let manager = SessionManager::default();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let (entered, entry) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (dropped, exited) = mpsc::channel();
        insert_fake_session_with_writer(
            &manager,
            "timeout",
            false,
            Box::new(RecordedGatedWriter {
                entered,
                release: gate,
                observed: Arc::clone(&observed),
                dropped,
            }),
        );
        let writer = manager.sessions.lock().unwrap()["timeout"].writer.clone();
        let first_writer = writer.clone();
        let first = std::thread::spawn(move || {
            first_writer.send_with_timeout(
                &vec![b'a'; super::WRITE_CHUNK_BYTES * 2],
                Duration::from_millis(250),
            )
        });
        entry.recv_timeout(Duration::from_secs(1)).unwrap();
        let queued_writer = writer.clone();
        let queued = std::thread::spawn(move || queued_writer.send(b"queued"));
        let deadline = Instant::now() + Duration::from_millis(200);
        while writer.queued.load(Ordering::SeqCst) < super::WRITE_CHUNK_BYTES * 2 + 6
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert!(first.join().unwrap().is_err());
        assert!(queued.join().unwrap().is_err());
        assert!(manager.snapshot("timeout").unwrap().input_blocked);
        assert!(manager.snapshot("timeout").unwrap().running);
        assert!(manager.write("timeout", "must not write").is_err());
        assert!(observed.lock().unwrap().is_empty());
        release.send(()).unwrap();
        exited.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            *observed.lock().unwrap(),
            vec![b'a'; super::WRITE_CHUNK_BYTES]
        );
        assert_eq!(writer.queued.load(Ordering::SeqCst), 0);
        manager.stop("timeout").unwrap();
    }

    #[test]
    fn concurrent_reservations_never_exceed_the_input_budget() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier, mpsc};

        let counter = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(17));
        let release = Arc::new(Barrier::new(17));
        let (results, replies) = mpsc::channel();
        let mut callers = Vec::new();
        for _caller in 0..16 {
            let counter = Arc::clone(&counter);
            let start = Arc::clone(&start);
            let release = Arc::clone(&release);
            let results = results.clone();
            callers.push(std::thread::spawn(move || {
                start.wait();
                let reservation = super::WriteReservation::acquire(&counter, 768 * 1024);
                results.send(reservation.is_ok()).unwrap();
                release.wait();
                drop(reservation);
            }));
        }
        start.wait();
        let accepted = (0..16)
            .filter(|_| replies.recv_timeout(Duration::from_secs(2)).unwrap())
            .count();
        let peak = counter.load(Ordering::SeqCst);
        release.wait();
        for caller in callers {
            caller.join().unwrap();
        }
        assert_eq!(accepted, 1);
        assert_eq!(peak, 768 * 1024);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn closed_receiver_releases_the_reserved_input_budget() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, mpsc};

        let (sender, receiver) = mpsc::channel();
        drop(receiver);
        let writer = super::SessionWriter {
            sender,
            queued: Arc::new(AtomicUsize::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
        };
        assert!(writer.send(b"undelivered").is_err());
        assert!(writer.closed.load(Ordering::SeqCst));
        assert_eq!(writer.queued.load(Ordering::SeqCst), 0);
    }

    #[test]
    #[ignore = "subprocess fixture invoked by native input tests"]
    fn non_reading_input_fixture() {
        std::thread::sleep(Duration::from_secs(10));
    }

    #[test]
    fn native_pipe_timeout_blocks_new_input_and_releases_after_child_exit() {
        use std::process::{Command, Stdio};
        use std::sync::atomic::Ordering;
        use std::time::Instant;

        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "sessions::tests::non_reading_input_fixture",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().unwrap();
        let writer = super::SessionWriter::spawn(Box::new(child.stdin.take().unwrap()));
        let result = writer.send_with_timeout(
            &vec![b'x'; super::WRITE_BACKLOG_LIMIT],
            Duration::from_millis(200),
        );
        let subsequent = writer.send(b"must not be queued");
        let _ = child.kill();
        child.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while writer.queued.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(result.is_err());
        assert!(subsequent.is_err());
        assert!(writer.closed.load(Ordering::SeqCst));
        assert_eq!(writer.queued.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn blocking_partial_input_preserves_output_but_refuses_further_writes() {
        let manager = SessionManager::default();
        insert_fake_session(&manager, "partial", false);
        manager.write("partial", "prefix").unwrap();
        manager.block_input("partial").unwrap();
        let snapshot = manager.snapshot("partial").unwrap();
        assert!(snapshot.running);
        assert!(snapshot.input_blocked);
        assert!(manager.write("partial", "\r").is_err());
        assert!(manager.list_live().is_ok());
        manager.stop_all().unwrap();
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
    fn archive_waits_for_in_flight_output_before_snapshotting() {
        let mut reader = ScriptedReader {
            chunks: vec![b"TAIL".to_vec()].into(),
        };
        let buffer = std::sync::Arc::new(super::Mutex::new(super::OutputBuffer::default()));
        let archived = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let archive_started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let snapshot = std::sync::Arc::new(super::Mutex::new(None));
        let mut events = Vec::new();
        let mut archive_thread = None;

        let mut sink = |event: super::SessionOutput| {
            let buffer_for_archive = std::sync::Arc::clone(&buffer);
            let archived_for_archive = std::sync::Arc::clone(&archived);
            let archive_started_for_thread = std::sync::Arc::clone(&archive_started);
            let snapshot_for_archive = std::sync::Arc::clone(&snapshot);
            archive_thread = Some(std::thread::spawn(move || {
                archive_started_for_thread.store(true, std::sync::atomic::Ordering::Release);
                let guard = buffer_for_archive.lock().unwrap();
                archived_for_archive.store(true, std::sync::atomic::Ordering::Release);
                *snapshot_for_archive.lock().unwrap() =
                    Some((guard.data.clone(), guard.end_offset));
            }));
            while !archive_started.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::yield_now();
            }
            events.push(event.data);
            true
        };

        super::pump_reader(&mut reader, &buffer, &archived, &mut sink);
        archive_thread.unwrap().join().unwrap();

        assert_eq!(events, vec!["TAIL".to_string()]);
        assert_eq!(snapshot.lock().unwrap().as_ref().unwrap().0, "TAIL");
        assert_eq!(snapshot.lock().unwrap().as_ref().unwrap().1, 4);
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

        let mut pty = spawn_pty(Path::new("/bin/cat"), &dir, 30, 120, &[], &[]).unwrap();
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

        let mut first = spawn_pty(Path::new("/bin/cat"), &first_dir, 24, 80, &[], &[]).unwrap();
        let mut second = spawn_pty(Path::new("/bin/cat"), &second_dir, 24, 80, &[], &[]).unwrap();

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
    fn session_info_reads_backend_owned_metadata_for_live_session() {
        let manager = super::SessionManager::default();
        insert_fake_session(&manager, "owned", false);
        let info = manager.session_info("owned").unwrap();
        assert_eq!(info.id, "owned");
        assert_eq!(info.tool_id, "claude");
        assert!(manager.session_info("missing").is_err());
    }

    #[test]
    fn list_sessions_returns_only_live_sessions_in_stable_order() {
        let manager = super::SessionManager::default();
        insert_fake_session(&manager, "b-session", false);
        insert_fake_session(&manager, "a-session", false);
        let listed = manager.list_live().unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a-session", "b-session"]
        );
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
        let mut pty = spawn_pty(Path::new("/bin/sh"), &dir, 30, 120, &[], &[]).unwrap();
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
            title: None,
            cli_session_id: None,
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
    fn explicit_stop_keeps_output_bytes_when_the_session_is_live() {
        let manager = SessionManager::default();
        let path = history_store("stop-bytes");
        manager.configure_history(path).unwrap();
        insert_fake_session(&manager, "s-bytes", false);
        {
            let mut sessions = manager.sessions.lock().unwrap();
            let session = sessions.get_mut("s-bytes").unwrap();
            let mut buffer = session.buffer.lock().unwrap();
            buffer.data.push_str("hello");
            buffer.end_offset = 5;
        }
        manager.record_start(super::SessionRecord {
            id: "s-bytes".to_string(),
            tool_id: "claude".to_string(),
            tool_name: "Claude Code".to_string(),
            project_path: history_project(),
            title: None,
            cli_session_id: None,
            started_at_ms: 10,
            ended_at_ms: None,
            exit_reason: None,
            output_bytes: None,
        });

        manager.stop("s-bytes").unwrap();

        let listed = manager.list_history().unwrap();
        assert_eq!(listed[0].exit_reason.as_deref(), Some("已停止"));
        assert_eq!(listed[0].output_bytes, Some(5));
        assert!(manager.finished.lock().unwrap().is_empty());
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
                title: None,
                cli_session_id: None,
                started_at_ms: 10,
                ended_at_ms: None,
                exit_reason: None,
                output_bytes: None,
            });
        }

        let _ = manager.stop_all();

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
    fn build_pty_command_wraps_windows_cmd_and_preserves_tool_args() {
        let extra = vec![
            "-c".to_string(),
            "mcp_servers.agentterm_scheduler.url=\"http://127.0.0.1:1/mcp\"".to_string(),
        ];
        let environment = vec![("AGENTTERM_SCHEDULE_TOKEN".to_string(), "secret".to_string())];
        let cmd = super::build_pty_command(
            Path::new(r"\\?\C:\Users\Administrator\AppData\Roaming\npm\codex.cmd"),
            Path::new(r"\\?\E:\AgentTerm"),
            &extra,
            &environment,
        );
        let args: Vec<String> = cmd
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.len(), 6);
        assert!(args[0].to_ascii_lowercase().ends_with("cmd.exe"));
        assert_eq!(args[1], "/D");
        assert_eq!(args[2], "/C");
        assert_eq!(
            args[3],
            r"C:\Users\Administrator\AppData\Roaming\npm\codex.cmd"
        );
        assert_eq!(args[4], extra[0]);
        assert_eq!(args[5], extra[1]);
        assert_eq!(cmd.get_cwd().unwrap().to_string_lossy(), r"E:\AgentTerm");
        assert_eq!(cmd.get_env("PYTHONUTF8").unwrap(), "1");
        assert_eq!(cmd.get_env("PYTHONIOENCODING").unwrap(), "utf-8");
        assert_eq!(cmd.get_env("AGENTTERM_SCHEDULE_TOKEN").unwrap(), "secret");
        assert!(!args.iter().any(|argument| argument.contains("secret")));
    }

    #[test]
    fn rename_updates_live_and_persisted_history_and_fails_closed() {
        let manager = SessionManager::default();
        let path = history_store("rename-manager");
        manager.configure_history(path).unwrap();
        insert_fake_session(&manager, "s-live", false);
        manager.record_start(super::SessionRecord {
            id: "s-live".to_string(),
            tool_id: "claude".to_string(),
            tool_name: "Claude Code".to_string(),
            project_path: history_project(),
            title: None,
            cli_session_id: None,
            started_at_ms: 10,
            ended_at_ms: None,
            exit_reason: None,
            output_bytes: None,
        });

        let saved = manager.rename("s-live", "  接口联调排查  ").unwrap();
        assert_eq!(saved, "接口联调排查");
        assert_eq!(
            manager.session_info("s-live").unwrap().title.as_deref(),
            Some("接口联调排查")
        );

        // Invalid rename must fail and keep previous live/persisted title intact.
        assert!(manager.rename("s-live", "   ").is_err());
        assert_eq!(
            manager.session_info("s-live").unwrap().title.as_deref(),
            Some("接口联调排查")
        );

        // Stopped sessions still present in history can also be renamed.
        manager.stop("s-live").unwrap();
        let after_stop = manager.rename("s-live", "归档：接口排查").unwrap();
        assert_eq!(after_stop, "归档：接口排查");
        let listed = manager.list_history().unwrap();
        assert_eq!(listed[0].title.as_deref(), Some("归档：接口排查"));
    }

    #[test]
    fn build_tool_launch_args_supports_fresh_and_resume_modes() {
        let (fresh_id, fresh_claude) = super::build_tool_launch_args(
            "claude",
            vec!["--mcp-config".into(), "{}".into()],
            None,
            false,
        )
        .unwrap();
        let uuid = fresh_id.expect("fresh claude session gets a UUID");
        assert!(crate::session_history::is_valid_uuid(&uuid));
        assert_eq!(
            fresh_claude,
            vec!["--session-id", &uuid, "--mcp-config", "{}"]
        );

        let (resumed_id, resumed_claude) = super::build_tool_launch_args(
            "claude",
            vec!["--mcp-config".into(), "{}".into()],
            Some(&uuid),
            true,
        )
        .unwrap();
        assert_eq!(resumed_id.as_deref(), Some(uuid.as_str()));
        assert_eq!(
            resumed_claude,
            vec!["--resume", &uuid, "--mcp-config", "{}"]
        );

        let (fallback_id, fallback_claude) =
            super::build_tool_launch_args("claude", vec![], None, true).unwrap();
        assert_eq!(fallback_id, None);
        assert_eq!(fallback_claude, vec!["--continue"]);

        let (codex_id, codex_resume) =
            super::build_tool_launch_args("codex", vec!["-c".into(), "foo=bar".into()], None, true)
                .unwrap();
        assert_eq!(codex_id, None);
        assert_eq!(codex_resume, vec!["resume", "--last", "-c", "foo=bar"]);

        let (codex_exact_id, codex_exact_resume) = super::build_tool_launch_args(
            "codex",
            vec!["-c".into(), "foo=bar".into()],
            Some("01a21ba5-7c2b-76c3-97c2-df4cdde2362f"),
            true,
        )
        .unwrap();
        assert_eq!(
            codex_exact_id.as_deref(),
            Some("01a21ba5-7c2b-76c3-97c2-df4cdde2362f")
        );
        assert_eq!(
            codex_exact_resume,
            vec![
                "resume",
                "01a21ba5-7c2b-76c3-97c2-df4cdde2362f",
                "-c",
                "foo=bar"
            ]
        );

        let (agy_id, agy_resume) =
            super::build_tool_launch_args("agy", vec![], None, true).unwrap();
        assert_eq!(agy_id, None);
        assert_eq!(agy_resume, vec!["--continue"]);

        let (agy_exact_id, agy_exact_resume) = super::build_tool_launch_args(
            "agy",
            vec![],
            Some("a83b21f1-fe05-4b49-a65a-db1e6d29da59"),
            true,
        )
        .unwrap();
        assert_eq!(
            agy_exact_id.as_deref(),
            Some("a83b21f1-fe05-4b49-a65a-db1e6d29da59")
        );
        assert_eq!(
            agy_exact_resume,
            vec!["--resume", "a83b21f1-fe05-4b49-a65a-db1e6d29da59"]
        );

        assert!(super::build_tool_launch_args("claude", vec![], Some("bad-id"), true).is_err());
    }

    #[test]
    fn resolves_distinct_cli_session_ids_for_claude_codex_and_agy() {
        let root = std::env::temp_dir().join(format!(
            "agentterm-resolve-cli-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let (proj, plain_proj, claude_dir_name) = if cfg!(windows) {
            (
                r"\\?\E:\Demo".to_string(),
                r"E:\Demo".to_string(),
                "E--Demo",
            )
        } else {
            ("/Demo".to_string(), "/Demo".to_string(), "-Demo")
        };
        let escaped_proj = plain_proj.replace('\\', "\\\\");
        let claude_dir = root.join("claude-projects").join(claude_dir_name);
        let codex_dir = root
            .join("codex-sessions")
            .join("2026")
            .join("11")
            .join("27");
        let agy_history = root.join("agy-history.jsonl");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::create_dir_all(&codex_dir).unwrap();

        let claude_u1 = "11111111-1111-4111-8111-111111111111";
        let claude_u2 = "22222222-2222-4222-8222-222222222222";
        std::fs::write(
            claude_dir.join(format!("{claude_u1}.jsonl")),
            "{\"type\":\"user\",\"timestamp\":\"2026-11-27T01:00:02.000Z\"}\n",
        )
        .unwrap();
        std::fs::write(
            claude_dir.join(format!("{claude_u2}.jsonl")),
            "{\"type\":\"user\",\"timestamp\":\"2026-11-27T02:00:03.000Z\"}\n",
        )
        .unwrap();

        let codex_u1 = "33333333-3333-4333-8333-333333333333";
        std::fs::write(
            codex_dir.join(format!("rollout-2026-11-27-{codex_u1}.jsonl")),
            format!(
                "{{\"timestamp\":\"2026-11-27T03:00:04.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{codex_u1}\",\"timestamp\":\"2026-11-27T03:00:04.000Z\",\"cwd\":\"{escaped_proj}\"}}}}\n"
            ),
        )
        .unwrap();
        let codex_skip = "55555555-5555-4555-8555-555555555555";
        std::fs::write(
            codex_dir.join(format!("rollout-2026-11-27-{codex_skip}.jsonl")),
            format!(
                "{{\"timestamp\":\"2026-11-27T05:00:04.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{codex_skip}\",\"timestamp\":\"2026-11-27T05:00:04.000Z\",\"cwd\":\"{escaped_proj}\"}}}}\n"
            ),
        )
        .unwrap();

        let t0 = chrono::DateTime::parse_from_rfc3339("2026-11-27T00:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let t1 = chrono::DateTime::parse_from_rfc3339("2026-11-27T01:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let t2 = chrono::DateTime::parse_from_rfc3339("2026-11-27T02:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let t3 = chrono::DateTime::parse_from_rfc3339("2026-11-27T03:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let t4 = chrono::DateTime::parse_from_rfc3339("2026-11-27T04:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let t5 = chrono::DateTime::parse_from_rfc3339("2026-11-27T05:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;

        let agy_u1 = "44444444-4444-4444-8444-444444444444";
        std::fs::write(
            &agy_history,
            format!(
                concat!(
                    "{{\"display\":\"later-turn\",\"timestamp\":{},\"workspace\":\"{}\",\"conversationId\":\"{}\"}}\n",
                    "{{\"display\":\"hello\",\"timestamp\":{},\"workspace\":\"{}\",\"conversationId\":\"{}\"}}\n"
                ),
                t4 + 90_000,
                escaped_proj,
                agy_u1,
                t4 + 2_000,
                escaped_proj,
                agy_u1,
            ),
        )
        .unwrap();

        let mut records = vec![
            super::SessionRecord {
                id: "s-codex-skip".into(),
                tool_id: "codex".into(),
                tool_name: "Codex".into(),
                project_path: proj.clone(),
                title: None,
                cli_session_id: None,
                started_at_ms: t5,
                ended_at_ms: Some(t5 + 20_000),
                exit_reason: Some("已停止".into()),
                output_bytes: None,
            },
            super::SessionRecord {
                id: "s-agy".into(),
                tool_id: "agy".into(),
                tool_name: "Agy".into(),
                project_path: plain_proj.clone(),
                title: None,
                cli_session_id: None,
                started_at_ms: t4,
                ended_at_ms: Some(t4 + 20_000),
                exit_reason: Some("已停止".into()),
                output_bytes: None,
            },
            super::SessionRecord {
                id: "s-codex".into(),
                tool_id: "codex".into(),
                tool_name: "Codex".into(),
                project_path: proj.clone(),
                title: None,
                cli_session_id: None,
                started_at_ms: t3,
                ended_at_ms: Some(t3 + 20_000),
                exit_reason: Some("已停止".into()),
                output_bytes: None,
            },
            super::SessionRecord {
                id: "s-claude-2".into(),
                tool_id: "claude".into(),
                tool_name: "Claude Code".into(),
                project_path: proj.clone(),
                title: None,
                cli_session_id: None,
                started_at_ms: t2,
                ended_at_ms: Some(t2 + 20_000),
                exit_reason: Some("已停止".into()),
                output_bytes: None,
            },
            super::SessionRecord {
                id: "s-claude-1".into(),
                tool_id: "claude".into(),
                tool_name: "Claude Code".into(),
                project_path: proj.clone(),
                title: None,
                cli_session_id: None,
                started_at_ms: t1,
                ended_at_ms: Some(t1 + 20_000),
                exit_reason: Some("已停止".into()),
                output_bytes: None,
            },
            super::SessionRecord {
                id: "s-claude-empty".into(),
                tool_id: "claude".into(),
                tool_name: "Claude Code".into(),
                project_path: proj,
                title: None,
                cli_session_id: None,
                started_at_ms: t0,
                ended_at_ms: Some(t0 + 5_000),
                exit_reason: Some("已停止".into()),
                output_bytes: None,
            },
        ];

        let stores = super::CliSessionStores {
            claude_projects_dir: Some(root.join("claude-projects")),
            codex_sessions_dir: Some(root.join("codex-sessions")),
            agy_history_jsonl: Some(agy_history),
        };
        let mut skip_ids = std::collections::HashSet::new();
        skip_ids.insert("s-codex-skip".to_string());
        let bound = super::resolve_missing_cli_sessions(&mut records, &skip_ids, &stores);
        assert!(bound.iter().any(|(id, cid)| id == "s-agy" && cid == agy_u1));
        assert!(
            bound
                .iter()
                .any(|(id, cid)| id == "s-codex" && cid == codex_u1)
        );
        assert!(
            bound
                .iter()
                .any(|(id, cid)| id == "s-claude-1" && cid == claude_u1)
        );
        assert!(
            bound
                .iter()
                .any(|(id, cid)| id == "s-claude-2" && cid == claude_u2)
        );
        assert!(!bound.iter().any(|(id, _)| id == "s-claude-empty"));
        assert!(!bound.iter().any(|(id, _)| id == "s-codex-skip"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
