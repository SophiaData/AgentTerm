use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use tauri::{AppHandle, Emitter};

use crate::schedule_mcp::SessionBridge;
use crate::session_history::is_valid_cli_session_id;
#[cfg(windows)]
use crate::sessions::WinJob;
use crate::sessions::{ENV_BLOCKLIST, SessionInfo, SessionManager, strip_windows_verbatim};
use crate::tools::{is_windows_batch, resolve_direct_command, windows_cmd_executable};

const MAX_MESSAGES: usize = 400;
const MAX_MESSAGE_CHARS: usize = 64 * 1024;
const MAX_TOOL_CALLS: usize = 200;
const MAX_TOOL_DETAIL_CHARS: usize = 8 * 1024;
const MAX_STDERR_CHARS: usize = 500;
const WRITER_QUEUE_CAP: usize = 128;
const MAX_PENDING_PERMISSIONS: usize = 32;
const MAX_PROMPT_IMAGES: usize = 4;
const MAX_IMAGE_BASE64_BYTES: usize = 10 * 1024 * 1024;
const MAX_FILE_SEARCH_SCAN: usize = 2500;
const MAX_FILE_SEARCH_DEPTH: usize = 6;
const MAX_FILE_SEARCH_RESULTS: usize = 25;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AcpPhase {
    Connecting,
    Ready,
    Exited,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpMessage {
    pub id: String,
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpToolCall {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUsage {
    pub used: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpTimelineEntry {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionRequest {
    pub request_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub options: Vec<AcpPermissionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigChoice {
    pub value: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigOption {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub current_value: String,
    pub choices: Vec<AcpConfigChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpCommandSpec {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpImageInput {
    pub mime_type: String,
    pub data: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSnapshot {
    pub phase: AcpPhase,
    pub busy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_reason: Option<String>,
    pub messages: Vec<AcpMessage>,
    pub tool_calls: Vec<AcpToolCall>,
    #[serde(default)]
    pub timeline: Vec<AcpTimelineEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<AcpUsage>,
    #[serde(default)]
    pub available_commands: Vec<AcpCommandSpec>,
    #[serde(default)]
    pub config_options: Vec<AcpConfigOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_permission: Option<AcpPermissionRequest>,
    pub version: u64,
}

#[derive(Clone)]
pub(crate) struct AcpWriter {
    tx: mpsc::SyncSender<String>,
    closed: Arc<AtomicBool>,
}

impl AcpWriter {
    fn spawn(mut stdin: ChildStdin) -> Self {
        let (tx, rx) = mpsc::sync_channel::<String>(WRITER_QUEUE_CAP);
        let closed = Arc::new(AtomicBool::new(false));
        let closed_worker = Arc::clone(&closed);
        std::thread::spawn(move || {
            while let Ok(line) = rx.recv() {
                if closed_worker.load(Ordering::SeqCst) {
                    break;
                }
                if stdin.write_all(line.as_bytes()).is_err()
                    || stdin.write_all(b"\n").is_err()
                    || stdin.flush().is_err()
                {
                    closed_worker.store(true, Ordering::SeqCst);
                    break;
                }
            }
            closed_worker.store(true, Ordering::SeqCst);
        });
        Self { tx, closed }
    }

    fn send_json(&self, value: &Value) -> Result<(), String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("ACP 会话已关闭，无法发送请求".to_string());
        }
        let serialized =
            serde_json::to_string(value).map_err(|_| "无法序列化 ACP 请求".to_string())?;
        self.tx.try_send(serialized).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => "ACP 请求队列已满，请稍后再试".to_string(),
            mpsc::TrySendError::Disconnected(_) => "ACP 会话写入通道已关闭".to_string(),
        })
    }

    fn block(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

pub(crate) struct AcpState {
    tool_id: String,
    project_path: String,
    resume_id: Option<String>,
    mcp_servers: Vec<Value>,
    retried_without_mcp: bool,
    next_request_id: u64,
    active_prompt_id: Option<u64>,
    setup_request_id: Option<u64>,
    pending_config_requests: HashSet<u64>,
    active_stream: Option<(String, Option<String>)>,
    pending_permission_raw_id: Option<(String, Value)>,
    queued_permissions: VecDeque<(AcpPermissionRequest, Value)>,
    next_message_seq: u64,
    total_output_bytes: u64,
    snapshot: AcpSnapshot,
}

impl AcpState {
    fn new(
        tool_id: &str,
        project_path: &str,
        resume_id: Option<String>,
        bridge: Option<&SessionBridge>,
    ) -> Self {
        let clean_project_path = strip_windows_verbatim(Path::new(project_path))
            .display()
            .to_string();
        let mcp_servers = bridge
            .map(|entry| {
                vec![json!({
                    "type": "http",
                    "name": crate::schedule_mcp::TOOL_NAME,
                    "url": entry.endpoint(),
                    "headers": [{
                        "name": "Authorization",
                        "value": format!("Bearer {}", entry.token()),
                    }]
                })]
            })
            .unwrap_or_default();
        Self {
            tool_id: tool_id.to_string(),
            project_path: clean_project_path,
            resume_id: resume_id.clone(),
            mcp_servers,
            retried_without_mcp: false,
            next_request_id: 2,
            active_prompt_id: None,
            setup_request_id: None,
            pending_config_requests: HashSet::new(),
            active_stream: None,
            pending_permission_raw_id: None,
            queued_permissions: VecDeque::new(),
            next_message_seq: 0,
            total_output_bytes: 0,
            snapshot: AcpSnapshot {
                phase: AcpPhase::Connecting,
                busy: false,
                cli_session_id: resume_id,
                error: None,
                exit_reason: None,
                messages: Vec::new(),
                tool_calls: Vec::new(),
                timeline: Vec::new(),
                usage: None,
                available_commands: Vec::new(),
                config_options: Vec::new(),
                pending_permission: None,
                version: 1,
            },
        }
    }

    fn bump(&mut self) {
        self.snapshot.version = self.snapshot.version.saturating_add(1);
    }

    fn record_timeline(&mut self, kind: &str, id: &str) {
        self.snapshot.timeline.push(AcpTimelineEntry {
            kind: kind.to_string(),
            id: id.to_string(),
        });
    }

    fn remove_timeline(&mut self, kind: &str, id: &str) {
        self.snapshot
            .timeline
            .retain(|entry| !(entry.kind == kind && entry.id == id));
    }

    fn evict_oldest_message_if_full(&mut self) {
        if self.snapshot.messages.len() >= MAX_MESSAGES {
            let removed = self.snapshot.messages.remove(0);
            self.remove_timeline("message", &removed.id);
        }
    }

    fn push_message(&mut self, role: &str, text: String) {
        if text.is_empty() {
            return;
        }
        self.next_message_seq = self.next_message_seq.saturating_add(1);
        self.total_output_bytes = self.total_output_bytes.saturating_add(text.len() as u64);
        let id = format!("msg-{}", self.next_message_seq);
        let msg = AcpMessage {
            id: id.clone(),
            role: role.to_string(),
            text: trim_chars(&text, MAX_MESSAGE_CHARS),
        };
        self.evict_oldest_message_if_full();
        self.snapshot.messages.push(msg);
        self.record_timeline("message", &id);
        self.active_stream = None;
        self.bump();
    }

    fn append_streaming_message(&mut self, role: &str, message_key: Option<&str>, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        self.total_output_bytes = self.total_output_bytes.saturating_add(chunk.len() as u64);
        let key_owned = message_key.map(str::to_string);
        let same_stream = self
            .active_stream
            .as_ref()
            .is_some_and(|(active_role, active_key)| {
                active_role == role && (key_owned.is_none() || *active_key == key_owned)
            });
        let can_append = same_stream
            && matches!(self.snapshot.messages.last(), Some(last) if last.role == role)
            && matches!(self.snapshot.timeline.last(), Some(last) if last.kind == "message");
        if !can_append && chunk.trim().is_empty() {
            return;
        }
        if can_append && let Some(last) = self.snapshot.messages.last_mut() {
            last.text.push_str(chunk);
            if last.text.chars().count() > MAX_MESSAGE_CHARS {
                last.text = trim_chars_tail(&last.text, MAX_MESSAGE_CHARS);
            }
        } else {
            self.next_message_seq = self.next_message_seq.saturating_add(1);
            let id = format!("msg-{}", self.next_message_seq);
            self.evict_oldest_message_if_full();
            self.snapshot.messages.push(AcpMessage {
                id: id.clone(),
                role: role.to_string(),
                text: trim_chars_tail(chunk, MAX_MESSAGE_CHARS),
            });
            self.record_timeline("message", &id);
            self.active_stream = Some((role.to_string(), key_owned));
        }
        self.bump();
    }

    fn upsert_tool_call(&mut self, call: AcpToolCall) {
        if let Some(existing) = self
            .snapshot
            .tool_calls
            .iter_mut()
            .find(|item| item.id == call.id)
        {
            if !call.title.is_empty() {
                existing.title = call.title;
            }
            if call.name.is_some() {
                existing.name = call.name;
            }
            if call.kind.is_some() {
                existing.kind = call.kind;
            }
            if !call.status.is_empty() {
                existing.status = call.status;
            }
            if call.input.is_some() {
                existing.input = call.input;
            }
            if call.detail.is_some() {
                existing.detail = call.detail;
            }
        } else {
            if self.snapshot.tool_calls.len() >= MAX_TOOL_CALLS {
                let removed = self.snapshot.tool_calls.remove(0);
                self.remove_timeline("tool", &removed.id);
            }
            let id = call.id.clone();
            self.snapshot.tool_calls.push(call);
            self.record_timeline("tool", &id);
            self.active_stream = None;
        }
        self.bump();
    }

    fn update_tool_call(
        &mut self,
        id: &str,
        title: Option<String>,
        status: Option<String>,
        input: Option<String>,
        detail: Option<String>,
    ) {
        if let Some(existing) = self
            .snapshot
            .tool_calls
            .iter_mut()
            .find(|item| item.id == id)
        {
            if let Some(title) = title.filter(|value| !value.is_empty()) {
                existing.title = title;
            }
            if let Some(status) = status.filter(|value| !value.is_empty()) {
                existing.status = status;
            }
            if input.is_some() {
                existing.input = input;
            }
            if detail.is_some() {
                existing.detail = detail;
            }
            self.bump();
        } else {
            self.upsert_tool_call(AcpToolCall {
                id: id.to_string(),
                title: title.unwrap_or_else(|| "工具调用".to_string()),
                name: None,
                kind: None,
                status: status.unwrap_or_else(|| "in_progress".to_string()),
                input,
                detail,
            });
        }
    }

    fn respond_permission(
        &mut self,
        writer: &AcpWriter,
        request_id: &str,
        option_id: Option<&str>,
    ) -> Result<(), String> {
        let Some((expected_id, raw_id)) = self.pending_permission_raw_id.as_ref() else {
            return Err("当前没有待处理的权限确认".to_string());
        };
        if expected_id != request_id {
            return Err("权限请求已过期或编号不匹配".to_string());
        }
        if let Some(chosen) = option_id
            && !self
                .snapshot
                .pending_permission
                .as_ref()
                .is_some_and(|request| {
                    request
                        .options
                        .iter()
                        .any(|option| option.option_id == chosen)
                })
        {
            return Err("权限选项无效，请重新核对请求".to_string());
        }
        let outcome = match option_id {
            Some(chosen) => json!({ "outcome": "selected", "optionId": chosen }),
            None => json!({ "outcome": "cancelled" }),
        };
        // Keep the request available on queue rejection; the user can retry.
        writer.send_json(&json!({
            "jsonrpc": "2.0", "id": raw_id,
            "result": { "outcome": outcome }
        }))?;
        if let Some((next, raw_id)) = self.queued_permissions.pop_front() {
            self.pending_permission_raw_id = Some((next.request_id.clone(), raw_id));
            self.snapshot.pending_permission = Some(next);
        } else {
            self.pending_permission_raw_id = None;
            self.snapshot.pending_permission = None;
        }
        self.bump();
        Ok(())
    }

    fn clear_permissions(&mut self) -> Vec<Value> {
        let mut cancelled = Vec::new();
        if let Some((_, raw_id)) = self.pending_permission_raw_id.take() {
            cancelled.push(permission_cancelled(raw_id));
        }
        cancelled.extend(
            self.queued_permissions
                .drain(..)
                .map(|(_, id)| permission_cancelled(id)),
        );
        self.snapshot.pending_permission = None;
        cancelled
    }
}

fn permission_cancelled(raw_id: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": raw_id, "result": {"outcome": {"outcome": "cancelled"}}})
}

pub struct AcpSessionHandle {
    pub info: SessionInfo,
    #[cfg(unix)]
    pgid: Option<libc::pid_t>,
    #[cfg(windows)]
    job: Option<WinJob>,
    child: Arc<Mutex<Child>>,
    writer: AcpWriter,
    state: Arc<Mutex<AcpState>>,
    closed: Arc<AtomicBool>,
}

impl AcpSessionHandle {
    pub fn snapshot(&self) -> Result<AcpSnapshot, String> {
        self.state
            .lock()
            .map(|guard| guard.snapshot.clone())
            .map_err(|_| "无法读取 ACP 会话状态".to_string())
    }

    pub fn project_path(&self) -> PathBuf {
        self.state
            .lock()
            .map(|guard| PathBuf::from(&guard.project_path))
            .unwrap_or_else(|_| strip_windows_verbatim(Path::new(&self.info.project_path)))
    }

    pub fn send_prompt(&self, raw_text: &str, images: &[AcpImageInput]) -> Result<(), String> {
        let text = raw_text.trim();
        if text.is_empty() && images.is_empty() {
            return Err("发送内容不能为空".to_string());
        }
        let mut prompt_blocks = validate_acp_images(images)?;
        if !text.is_empty() {
            prompt_blocks.push(json!({
                "type": "text",
                "text": text,
            }));
        }
        let display_text = format_user_prompt_display(text, images);
        let (request_id, cli_session_id) = {
            let mut guard = self
                .state
                .lock()
                .map_err(|_| "ACP 会话状态不可用".to_string())?;
            if guard.snapshot.phase != AcpPhase::Ready {
                return Err("ACP 会话尚未就绪".to_string());
            }
            if guard.snapshot.busy {
                return Err("当前轮次仍在执行，请等待完成或先停止".to_string());
            }
            let cli_session_id = guard
                .snapshot
                .cli_session_id
                .clone()
                .ok_or_else(|| "ACP 会话标识尚未建立".to_string())?;
            let id = guard.next_request_id;
            guard.next_request_id = guard.next_request_id.saturating_add(1);
            guard.active_prompt_id = Some(id);
            guard.snapshot.busy = true;
            guard.snapshot.error = None;
            guard.push_message("user", display_text);
            (id, cli_session_id)
        };
        let payload = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "session/prompt",
            "params": {
                "sessionId": cli_session_id,
                "prompt": prompt_blocks,
            }
        });
        if let Err(error) = self.writer.send_json(&payload) {
            if let Ok(mut guard) = self.state.lock() {
                guard.snapshot.busy = false;
                guard.active_prompt_id = None;
                guard.snapshot.error = Some(error.clone());
                guard.bump();
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn cancel_turn(&self) -> Result<(), String> {
        let cli_session_id = {
            let guard = self
                .state
                .lock()
                .map_err(|_| "ACP 会话状态不可用".to_string())?;
            guard
                .snapshot
                .cli_session_id
                .clone()
                .ok_or_else(|| "ACP 会话尚未就绪".to_string())?
        };
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": {
                "sessionId": cli_session_id,
            }
        });
        self.writer.send_json(&payload)
    }

    pub fn respond_permission(
        &self,
        request_id: &str,
        option_id: Option<&str>,
    ) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|_| "ACP 会话状态不可用".to_string())?
            .respond_permission(&self.writer, request_id, option_id)
    }

    pub fn set_config_option(&self, config_id: &str, value: &str) -> Result<(), String> {
        let config_id = config_id.trim();
        if config_id.is_empty() || value.is_empty() {
            return Err("配置参数不能为空".to_string());
        }
        let (request_id, cli_session_id) = {
            let mut guard = self
                .state
                .lock()
                .map_err(|_| "ACP 会话状态不可用".to_string())?;
            if guard.snapshot.phase != AcpPhase::Ready {
                return Err("ACP 会话尚未就绪".to_string());
            }
            if guard.snapshot.busy {
                return Err("当前轮次仍在执行，请稍后再切换配置".to_string());
            }
            let cli_session_id = guard
                .snapshot
                .cli_session_id
                .clone()
                .ok_or_else(|| "ACP 会话标识尚未建立".to_string())?;
            let id = guard.next_request_id;
            guard.next_request_id = guard.next_request_id.saturating_add(1);
            guard.pending_config_requests.insert(id);
            guard.snapshot.error = None;
            guard.bump();
            (id, cli_session_id)
        };
        let payload = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "session/set_config_option",
            "params": {
                "sessionId": cli_session_id,
                "configId": config_id,
                "value": value,
            }
        });
        if let Err(error) = self.writer.send_json(&payload) {
            if let Ok(mut guard) = self.state.lock() {
                guard.pending_config_requests.remove(&request_id);
                guard.snapshot.error = Some(error.clone());
                guard.bump();
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn terminate(&self) -> Result<(), String> {
        self.closed.store(true, Ordering::SeqCst);
        let cli_id = self
            .state
            .lock()
            .ok()
            .and_then(|guard| guard.snapshot.cli_session_id.clone());
        if let Some(cli_id) = cli_id {
            let _ = self.writer.send_json(&json!({
                "jsonrpc": "2.0",
                "id": 999_999,
                "method": "session/close",
                "params": { "sessionId": cli_id }
            }));
        }
        self.writer.block();
        let mut child = self
            .child
            .lock()
            .map_err(|_| "无法访问 ACP 进程句柄".to_string())?;
        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => child
                .kill()
                .map_err(|error| format!("无法结束 ACP 会话：{error}"))?,
            Err(error) => return Err(format!("无法确认 ACP 会话状态：{error}")),
        }
        #[cfg(unix)]
        crate::sessions::kill_process_group(self.pgid);
        #[cfg(windows)]
        if let Some(job) = self.job.as_ref() {
            job.terminate();
        }
        Ok(())
    }

    pub fn output_bytes(&self) -> u64 {
        self.state
            .lock()
            .map(|guard| guard.total_output_bytes)
            .unwrap_or(0)
    }

    pub fn emit_snapshot(&self, app: &AppHandle) {
        if let Ok(snapshot) = self.snapshot() {
            let _ = app.emit(&format!("acp-state-{}", self.info.id), snapshot);
        }
    }
}

#[tauri::command(async)]
pub fn acp_snapshot(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
) -> Result<AcpSnapshot, String> {
    state.acp_snapshot(&session_id)
}

#[tauri::command(async)]
pub fn acp_send_prompt(
    state: tauri::State<'_, SessionManager>,
    app: AppHandle,
    session_id: String,
    text: String,
    images: Option<Vec<AcpImageInput>>,
) -> Result<(), String> {
    state.acp_send_prompt(&app, &session_id, &text, &images.unwrap_or_default())
}

#[tauri::command(async)]
pub fn acp_search_project_files(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
    query: String,
) -> Result<Vec<String>, String> {
    state.acp_search_project_files(&session_id, &query)
}

#[tauri::command(async)]
pub fn acp_cancel_turn(
    state: tauri::State<'_, SessionManager>,
    session_id: String,
) -> Result<(), String> {
    state.acp_cancel_turn(&session_id)
}

#[tauri::command(async)]
pub fn acp_respond_permission(
    state: tauri::State<'_, SessionManager>,
    app: AppHandle,
    session_id: String,
    request_id: String,
    option_id: Option<String>,
) -> Result<(), String> {
    state.acp_respond_permission(&app, &session_id, &request_id, option_id.as_deref())
}

#[tauri::command(async)]
pub fn acp_set_config_option(
    state: tauri::State<'_, SessionManager>,
    app: AppHandle,
    session_id: String,
    config_id: String,
    value: String,
) -> Result<(), String> {
    state.acp_set_config_option(&app, &session_id, &config_id, &value)
}

pub struct AcpHooks {
    pub on_emit: Box<dyn Fn(&AcpSnapshot) + Send + Sync>,
    pub on_cli_session: Box<dyn Fn(String) + Send + Sync>,
    pub on_exit: Box<dyn Fn(String, Option<AcpSnapshot>, u64) + Send + Sync>,
}

pub fn build_acp_command(
    tool_id: &str,
    binary: &Path,
    cwd: &Path,
    additional_env: &[(String, String)],
) -> Command {
    let clean_binary = strip_windows_verbatim(binary);
    let clean_cwd = strip_windows_verbatim(cwd);
    let mut command = if let Some(spec) = resolve_direct_command(tool_id, &clean_binary) {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.prefix_args).args(["--profile", "acp"]);
        for (key, value) in spec.env {
            cmd.env(key, value);
        }
        cmd
    } else if is_windows_batch(&clean_binary) {
        let mut wrapped = Command::new(windows_cmd_executable());
        wrapped
            .arg("/D")
            .arg("/C")
            .arg(&clean_binary)
            .args(["--profile", "acp"]);
        wrapped
    } else {
        let mut cmd = Command::new(&clean_binary);
        cmd.args(["--profile", "acp"]);
        cmd
    };
    command.current_dir(clean_cwd);
    for (key, _) in std::env::vars() {
        if ENV_BLOCKLIST.contains(&key.as_str())
            && !(tool_id == "dsh" && key == "ELECTRON_RUN_AS_NODE")
        {
            command.env_remove(&key);
        }
    }
    for (key, value) in additional_env {
        command.env(key, value);
    }
    command
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

pub fn spawn_acp_session(
    info: SessionInfo,
    binary: &Path,
    cwd: &Path,
    tool_env: &[(String, String)],
    bridge: Option<&SessionBridge>,
    resume_cli_id: Option<String>,
    hooks: AcpHooks,
) -> Result<AcpSessionHandle, String> {
    let command = build_acp_command(&info.tool_id, binary, cwd, tool_env);
    spawn_acp_with_command(info, command, bridge, resume_cli_id, hooks)
}

pub(crate) fn spawn_acp_with_command(
    info: SessionInfo,
    mut command: Command,
    bridge: Option<&SessionBridge>,
    resume_cli_id: Option<String>,
    hooks: AcpHooks,
) -> Result<AcpSessionHandle, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 ACP 进程：{error}"))?;
    #[cfg(windows)]
    let job = {
        use std::os::windows::io::AsRawHandle;
        WinJob::attach(child.as_raw_handle()).ok()
    };
    #[cfg(unix)]
    let pgid = Some(child.id() as libc::pid_t);

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "无法打开 ACP 标准输入".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法打开 ACP 标准输出".to_string())?;
    let stderr = child.stderr.take();

    let writer = AcpWriter::spawn(stdin);
    let state = Arc::new(Mutex::new(AcpState::new(
        &info.tool_id,
        &info.project_path,
        resume_cli_id,
        bridge,
    )));
    let closed = Arc::new(AtomicBool::new(false));
    let child_arc = Arc::new(Mutex::new(child));

    let stderr_tail = Arc::new(Mutex::new(String::new()));
    if let Some(mut stderr_pipe) = stderr {
        let stderr_tail_writer = Arc::clone(&stderr_tail);
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut acc = String::new();
            while let Ok(read) = stderr_pipe.read(&mut buf) {
                if read == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&buf[..read]);
                acc.push_str(&text);
                if acc.chars().count() > MAX_STDERR_CHARS {
                    acc = trim_chars_tail(&acc, MAX_STDERR_CHARS);
                }
                if let Ok(mut guard) = stderr_tail_writer.lock() {
                    *guard = acc.trim().to_string();
                }
            }
        });
    }

    let init_payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {
                    "readTextFile": false,
                    "writeTextFile": false
                },
                "terminal": false
            },
            "clientInfo": {
                "name": "agentterm",
                "version": env!("CARGO_PKG_VERSION"),
            }
        }
    });
    writer.send_json(&init_payload)?;

    let reader_state = Arc::clone(&state);
    let reader_writer = writer.clone();
    let reader_child = Arc::clone(&child_arc);
    let reader_closed = Arc::clone(&closed);
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(Ok(line)) = lines.next() {
            if reader_closed.load(Ordering::SeqCst) {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            let outcome = {
                let Ok(mut guard) = reader_state.lock() else {
                    break;
                };
                apply_server_message(&mut guard, &value)
            };
            for outgoing in outcome.outgoing {
                let _ = reader_writer.send_json(&outgoing);
            }
            if let Some(cli_id) = outcome.bound_cli_session {
                (hooks.on_cli_session)(cli_id);
            }
            if let Some(snapshot) = outcome.emitted_snapshot {
                (hooks.on_emit)(&snapshot);
            }
        }

        let status_reason = wait_for_child_exit(&reader_child)
            .ok()
            .map(|status| format!("{status}"))
            .unwrap_or_else(|| "已退出".to_string());
        let stderr_note = stderr_tail
            .lock()
            .ok()
            .filter(|text| !text.is_empty())
            .map(|text| text.clone());
        let (final_snapshot, bytes) = if let Ok(mut guard) = reader_state.lock() {
            let was_connecting = guard.snapshot.phase == AcpPhase::Connecting;
            guard.snapshot.busy = false;
            guard.active_prompt_id = None;
            let _ = guard.clear_permissions();
            if was_connecting && !reader_closed.load(Ordering::SeqCst) {
                guard.snapshot.phase = AcpPhase::Error;
                let detail = stderr_note
                    .as_deref()
                    .map(|msg| format!("：{msg}"))
                    .unwrap_or_default();
                guard.snapshot.error = Some(format!("ACP 会话在握手完成前退出{detail}"));
            } else {
                guard.snapshot.phase = AcpPhase::Exited;
            }
            guard.snapshot.exit_reason = Some(status_reason.clone());
            guard.bump();
            (Some(guard.snapshot.clone()), guard.total_output_bytes)
        } else {
            (None, 0)
        };
        if let Some(snapshot) = final_snapshot.as_ref() {
            (hooks.on_emit)(snapshot);
        }
        (hooks.on_exit)(status_reason, final_snapshot, bytes);
    });

    Ok(AcpSessionHandle {
        info,
        #[cfg(unix)]
        pgid,
        #[cfg(windows)]
        job,
        child: child_arc,
        writer,
        state,
        closed,
    })
}

// stdout EOF does not prove process exit. Release the handle lock between
// checks so terminate() can still kill a process which closed stdout first.
fn wait_for_child_exit(child: &Mutex<Child>) -> Result<std::process::ExitStatus, String> {
    loop {
        let status = child
            .lock()
            .map_err(|_| "无法访问 ACP 进程句柄".to_string())?
            .try_wait()
            .map_err(|error| format!("无法确认 ACP 会话状态：{error}"))?;
        if let Some(status) = status {
            return Ok(status);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

pub(crate) struct ApplyOutcome {
    pub emitted_snapshot: Option<AcpSnapshot>,
    pub outgoing: Vec<Value>,
    pub bound_cli_session: Option<String>,
}

pub(crate) fn apply_server_message(state: &mut AcpState, value: &Value) -> ApplyOutcome {
    let mut outgoing = Vec::new();
    let mut bound_cli_session = None;
    let initial_version = state.snapshot.version;

    let method = value.get("method").and_then(Value::as_str);
    let has_id = value.get("id").is_some() && !value["id"].is_null();

    match (method, has_id) {
        (None, true) => {
            let id_num = value["id"].as_u64();
            let err_msg = extract_rpc_error(value);
            if id_num == Some(1) {
                if let Some(err) = err_msg {
                    state.snapshot.phase = AcpPhase::Error;
                    state.snapshot.error = Some(format!("ACP 握手失败：{err}"));
                    state.bump();
                } else {
                    let req_id = state.next_request_id;
                    state.next_request_id = state.next_request_id.saturating_add(1);
                    state.setup_request_id = Some(req_id);
                    outgoing.push(build_session_open_request(
                        req_id,
                        &state.project_path,
                        state.resume_id.as_deref(),
                        &state.mcp_servers,
                    ));
                }
            } else if id_num.is_some() && id_num == state.setup_request_id {
                if let Some(err) = err_msg {
                    if !state.retried_without_mcp && !state.mcp_servers.is_empty() {
                        state.retried_without_mcp = true;
                        let req_id = state.next_request_id;
                        state.next_request_id = state.next_request_id.saturating_add(1);
                        state.setup_request_id = Some(req_id);
                        state.push_message(
                            "system",
                            format!("未能挂载定时任务 MCP，已降级为普通 ACP 会话（{err}）"),
                        );
                        outgoing.push(build_session_open_request(
                            req_id,
                            &state.project_path,
                            state.resume_id.as_deref(),
                            &[],
                        ));
                    } else {
                        state.snapshot.phase = AcpPhase::Error;
                        state.snapshot.error = Some(format!("无法创建或恢复 ACP 会话：{err}"));
                        state.bump();
                    }
                } else {
                    let returned_id = value
                        .get("result")
                        .and_then(|res| res.get("sessionId"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| state.resume_id.clone());
                    if let Some(cli_id) = returned_id
                        && is_valid_cli_session_id(&state.tool_id, &cli_id)
                    {
                        state.snapshot.cli_session_id = Some(cli_id.clone());
                        bound_cli_session = Some(cli_id);
                    }
                    if let Some(options) = parse_config_options(
                        value.get("result").and_then(|res| res.get("configOptions")),
                    ) {
                        state.snapshot.config_options = options;
                    }
                    state.snapshot.phase = AcpPhase::Ready;
                    state.snapshot.error = None;
                    state.bump();
                }
            } else if id_num.is_some() && id_num == state.active_prompt_id {
                outgoing.extend(state.clear_permissions());
                state.snapshot.busy = false;
                state.active_prompt_id = None;
                state.active_stream = None;
                if let Some(err) = err_msg {
                    state.snapshot.error = Some(err.clone());
                    state.push_message("system", format!("请求失败：{err}"));
                }
                state.bump();
            } else if let Some(req_id) = id_num
                && state.pending_config_requests.remove(&req_id)
            {
                if let Some(err) = err_msg {
                    state.snapshot.error = Some(format!("切换配置失败：{err}"));
                } else {
                    if let Some(options) = parse_config_options(
                        value.get("result").and_then(|res| res.get("configOptions")),
                    ) {
                        state.snapshot.config_options = options;
                    }
                    state.snapshot.error = None;
                }
                state.bump();
            }
        }
        (Some("session/update"), false) => {
            if let Some(update) = value.get("params").and_then(|p| p.get("update")) {
                apply_session_update(state, update);
            }
        }
        (Some("session/request_permission"), true) => {
            let raw_id = value["id"].clone();
            let params = value.get("params").unwrap_or(&Value::Null);
            let request_id = match &raw_id {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            let title = params
                .get("title")
                .and_then(Value::as_str)
                .or_else(|| {
                    params
                        .get("toolCall")
                        .and_then(|tc| tc.get("title"))
                        .and_then(Value::as_str)
                })
                .unwrap_or("需要确认工具操作权限")
                .to_string();
            let description = params
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string);
            let tool_call_id = params
                .get("toolCall")
                .and_then(|tc| tc.get("toolCallId"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let options = params
                .get("options")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            let option_id = item.get("optionId").and_then(Value::as_str)?;
                            let name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(option_id);
                            let kind = item
                                .get("kind")
                                .and_then(Value::as_str)
                                .unwrap_or("allow_once");
                            Some(AcpPermissionOption {
                                option_id: option_id.to_string(),
                                name: name.to_string(),
                                kind: kind.to_string(),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let request = AcpPermissionRequest {
                request_id,
                title,
                description,
                tool_call_id,
                options,
            };
            if state.pending_permission_raw_id.is_none() {
                state.pending_permission_raw_id = Some((request.request_id.clone(), raw_id));
                state.snapshot.pending_permission = Some(request);
            } else if state.queued_permissions.len() + 1 < MAX_PENDING_PERMISSIONS {
                state.queued_permissions.push_back((request, raw_id));
            } else {
                outgoing.push(permission_cancelled(raw_id));
            }
            state.bump();
        }
        (Some(_), true) => {
            outgoing.push(json!({
                "jsonrpc": "2.0",
                "id": value["id"].clone(),
                "error": {
                    "code": -32601,
                    "message": "Method not supported by AgentTerm ACP client"
                }
            }));
        }
        _ => {}
    }

    let emitted_snapshot =
        (state.snapshot.version != initial_version).then(|| state.snapshot.clone());
    ApplyOutcome {
        emitted_snapshot,
        outgoing,
        bound_cli_session,
    }
}

fn build_session_open_request(
    req_id: u64,
    cwd: &str,
    resume_id: Option<&str>,
    mcp_servers: &[Value],
) -> Value {
    if let Some(session_id) = resume_id {
        json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "session/resume",
            "params": {
                "sessionId": session_id,
                "cwd": cwd,
                "mcpServers": mcp_servers,
            }
        })
    } else {
        json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "session/new",
            "params": {
                "cwd": cwd,
                "mcpServers": mcp_servers,
            }
        })
    }
}

fn apply_session_update(state: &mut AcpState, update: &Value) {
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("agent_message_chunk") => {
            let message_id = update.get("messageId").and_then(Value::as_str);
            if let Some(text) = extract_content_text(update.get("content")) {
                state.append_streaming_message("assistant", message_id, &text);
            }
        }
        Some("agent_thought_chunk") => {
            let message_id = update.get("messageId").and_then(Value::as_str);
            if let Some(text) = extract_content_text(update.get("content")) {
                state.append_streaming_message("thought", message_id, &text);
            }
        }
        Some("user_message_chunk") => {
            let message_id = update.get("messageId").and_then(Value::as_str);
            if let Some(text) = extract_content_text(update.get("content")) {
                state.append_streaming_message("user", message_id, &text);
            }
        }
        Some("tool_call") => {
            let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
                return;
            };
            let title = update
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("工具调用")
                .to_string();
            let name = update
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string);
            let kind = update
                .get("kind")
                .and_then(Value::as_str)
                .map(str::to_string);
            let status = update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_string();
            let input = format_json_field(update.get("rawInput"));
            let detail = extract_tool_output(update);
            state.upsert_tool_call(AcpToolCall {
                id: id.to_string(),
                title,
                name,
                kind,
                status,
                input,
                detail,
            });
        }
        Some("tool_call_update") => {
            let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
                return;
            };
            let title = update
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string);
            let status = update
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string);
            let input = format_json_field(update.get("rawInput"));
            let detail = extract_tool_output(update);
            state.update_tool_call(id, title, status, input, detail);
        }
        Some("usage_update") => {
            if let (Some(used), Some(size)) = (
                update.get("used").and_then(Value::as_u64),
                update.get("size").and_then(Value::as_u64),
            ) {
                state.snapshot.usage = Some(AcpUsage { used, size });
                state.bump();
            }
        }
        Some("available_commands_update") => {
            if let Some(items) = update.get("availableCommands").and_then(Value::as_array) {
                state.snapshot.available_commands = items
                    .iter()
                    .filter_map(|item| {
                        let name = item.get("name").and_then(Value::as_str)?.trim();
                        if name.is_empty() {
                            return None;
                        }
                        let description = item
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let input_hint = item
                            .get("input")
                            .and_then(|input| input.get("hint"))
                            .or_else(|| item.get("hint"))
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        Some(AcpCommandSpec {
                            name: name.to_string(),
                            description,
                            input_hint,
                        })
                    })
                    .collect();
                state.bump();
            }
        }
        Some("config_option_update") => {
            if let Some(options) = parse_config_options(update.get("configOptions")) {
                state.snapshot.config_options = options;
                state.bump();
            }
        }
        _ => {}
    }
}

fn parse_config_options(raw: Option<&Value>) -> Option<Vec<AcpConfigOption>> {
    let items = raw?.as_array()?;
    let mut parsed = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(current_value) = item.get("currentValue").and_then(Value::as_str) else {
            continue;
        };
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string();
        let category = item
            .get("category")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut choices = Vec::new();
        if let Some(options) = item.get("options").and_then(Value::as_array) {
            for entry in options {
                if let Some(group_items) = entry.get("options").and_then(Value::as_array) {
                    let group_label = entry
                        .get("name")
                        .or_else(|| entry.get("group"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    for sub in group_items {
                        if let Some(choice) = parse_config_choice(sub, group_label.as_deref()) {
                            choices.push(choice);
                        }
                    }
                } else if let Some(choice) = parse_config_choice(entry, None) {
                    choices.push(choice);
                }
            }
        }
        parsed.push(AcpConfigOption {
            id: id.to_string(),
            name,
            category,
            current_value: current_value.to_string(),
            choices,
        });
    }
    Some(parsed)
}

fn parse_config_choice(value: &Value, group: Option<&str>) -> Option<AcpConfigChoice> {
    let val = value.get("value").and_then(Value::as_str)?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(val)
        .to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(AcpConfigChoice {
        value: val.to_string(),
        name,
        group: group.map(str::to_string),
        description,
    })
}

fn extract_content_text(content: Option<&Value>) -> Option<String> {
    let content = content?;
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Object(map) => map.get("text").and_then(Value::as_str).map(str::to_string),
        Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                } else if let Some(inner) = item
                    .get("content")
                    .and_then(|c| c.get("text"))
                    .and_then(Value::as_str)
                {
                    out.push_str(inner);
                }
            }
            (!out.is_empty()).then_some(out)
        }
        _ => None,
    }
}

fn format_json_field(raw: Option<&Value>) -> Option<String> {
    let value = raw?;
    if value.is_null() {
        return None;
    }
    let formatted = match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    if formatted.trim().is_empty() || formatted.trim() == "{}" {
        None
    } else {
        Some(trim_chars(&formatted, MAX_TOOL_DETAIL_CHARS))
    }
}

fn extract_tool_output(update: &Value) -> Option<String> {
    if let Some(text) = extract_content_text(update.get("content")) {
        return Some(trim_chars(&text, MAX_TOOL_DETAIL_CHARS));
    }
    format_json_field(update.get("rawOutput"))
}

fn extract_rpc_error(value: &Value) -> Option<String> {
    let err = value.get("error")?;
    if err.is_null() {
        return None;
    }
    let details = err
        .get("data")
        .and_then(|data| {
            data.get("details")
                .or_else(|| data.get("message"))
                .and_then(Value::as_str)
        })
        .filter(|text| !text.trim().is_empty());
    let message = err
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("未知 ACP 错误");
    Some(match details {
        Some(detail) if detail != message => format!("{message}: {detail}"),
        _ => message.to_string(),
    })
}

fn trim_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn trim_chars_tail(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let skip = count.saturating_sub(max_chars);
    text.chars().skip(skip).collect()
}

fn is_valid_canonical_base64(data: &str) -> bool {
    if data.is_empty() || data.len() > MAX_IMAGE_BASE64_BYTES || !data.len().is_multiple_of(4) {
        return false;
    }
    let bytes = data.as_bytes();
    let mut padding_seen = false;
    let mut padding_count = 0usize;
    for &byte in bytes {
        if byte == b'=' {
            padding_seen = true;
            padding_count += 1;
            if padding_count > 2 {
                return false;
            }
        } else if padding_seen || !(byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/') {
            return false;
        }
    }
    true
}

fn validate_acp_images(images: &[AcpImageInput]) -> Result<Vec<Value>, String> {
    if images.len() > MAX_PROMPT_IMAGES {
        return Err(format!("单次最多附带 {MAX_PROMPT_IMAGES} 张图片"));
    }
    let mut blocks = Vec::with_capacity(images.len());
    for img in images {
        let mime = img.mime_type.trim();
        if !matches!(
            mime,
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) {
            return Err("仅支持 PNG、JPEG、WebP 或 GIF 图片".to_string());
        }
        if !is_valid_canonical_base64(&img.data) {
            return Err("图片编码格式无效或体积超出限制".to_string());
        }
        blocks.push(json!({
            "type": "image",
            "mimeType": mime,
            "data": img.data,
        }));
    }
    Ok(blocks)
}

fn format_user_prompt_display(text: &str, images: &[AcpImageInput]) -> String {
    if images.is_empty() {
        return text.to_string();
    }
    let tags: Vec<String> = images
        .iter()
        .enumerate()
        .map(|(index, img)| {
            let label = img
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("图片 {}", index + 1));
            format!("[图片: {label}]")
        })
        .collect();
    if text.is_empty() {
        tags.join(" ")
    } else {
        format!("{}\n{}", tags.join(" "), text)
    }
}

fn should_skip_search_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | "dist" | ".next" | ".venv" | ".trash" | "build"
    )
}

fn is_symlink_or_reparse(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return true;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

pub(crate) fn search_project_files_in_dir(
    root: &Path,
    raw_query: &str,
) -> Result<Vec<String>, String> {
    let clean_root = strip_windows_verbatim(root);
    if !clean_root.is_dir() {
        return Err("项目目录不存在或不可访问".to_string());
    }
    let query = raw_query.trim().to_ascii_lowercase();
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((clean_root.clone(), 0));
    let mut scanned = 0usize;
    let mut candidates: Vec<(u8, String)> = Vec::new();

    while let Some((dir, depth)) = queue.pop_front() {
        if scanned >= MAX_FILE_SEARCH_SCAN {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        children.sort();

        for path in children {
            if scanned >= MAX_FILE_SEARCH_SCAN {
                break;
            }
            scanned += 1;
            if is_symlink_or_reparse(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if path.is_dir() {
                if depth < MAX_FILE_SEARCH_DEPTH && !should_skip_search_dir(name) {
                    queue.push_back((path, depth + 1));
                }
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(&clean_root) else {
                continue;
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if query.is_empty() {
                candidates.push((1, rel_str));
                continue;
            }
            let rel_lower = rel_str.to_ascii_lowercase();
            let base_lower = name.to_ascii_lowercase();
            if base_lower.starts_with(&query) {
                candidates.push((0, rel_str));
            } else if rel_lower.contains(&query) {
                candidates.push((1, rel_str));
            }
        }
    }

    candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(candidates
        .into_iter()
        .take(MAX_FILE_SEARCH_RESULTS)
        .map(|(_, path)| path)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{AcpPhase, AcpState, MAX_MESSAGES, MAX_TOOL_CALLS, apply_server_message};
    use serde_json::json;

    #[test]
    #[ignore = "local subprocess fixture, invoked by lifecycle regression tests"]
    fn acp_process_fixture() {
        use std::io::{BufRead, Write};
        match std::env::var("AGENTTERM_ACP_TEST_MODE").as_deref() {
            Ok("wait") => std::thread::sleep(std::time::Duration::from_secs(2)),
            Ok("handshake") => {
                for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                    let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                    let result = match request["method"].as_str() {
                        Some("initialize") => json!({"protocolVersion": 1}),
                        Some("session/new") => json!({"sessionId": "sess_fixture"}),
                        _ => continue,
                    };
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0", "id":request["id"], "result":result})
                    );
                    std::io::stdout().flush().unwrap();
                    if request["method"] == "session/new" {
                        break;
                    }
                }
            }
            _ => panic!("fixture mode missing"),
        }
    }

    #[test]
    fn waiting_for_exit_leaves_child_available_for_stop() {
        use std::sync::{Arc, Mutex, mpsc};
        use std::time::{Duration, Instant};
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "acp::tests::acp_process_fixture",
                "--ignored",
                "--nocapture",
                "--quiet",
            ])
            .env("AGENTTERM_ACP_TEST_MODE", "wait")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let child = Arc::new(Mutex::new(child));
        let watched = Arc::clone(&child);
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let result = super::wait_for_child_exit(&watched);
            done_tx.send(result).unwrap();
        });
        std::thread::sleep(Duration::from_millis(100));
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut stopped = false;
        while Instant::now() < deadline {
            if let Ok(mut process) = child.try_lock() {
                process.kill().unwrap();
                stopped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        done_rx
            .recv_timeout(Duration::from_secs(4))
            .unwrap()
            .unwrap();
        waiter.join().unwrap();
        assert!(stopped, "exit waiter held the child lock and blocked stop");
    }

    fn permission_request(id: serde_json::Value) -> serde_json::Value {
        json!({"jsonrpc":"2.0", "id":id, "method":"session/request_permission",
            "params":{"options":[{"optionId":"allow", "name":"允许一次", "kind":"allow_once"}]}})
    }

    #[test]
    fn permissions_are_answered_in_order_and_failed_send_is_retryable() {
        let mut state = AcpState::new("dsh", "/project", None, None);
        apply_server_message(&mut state, &permission_request(json!(42)));
        apply_server_message(&mut state, &permission_request(json!("second")));
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let writer = super::AcpWriter {
            tx,
            closed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        // A full writer queue must retain the first request and its options.
        writer.send_json(&json!({"occupied":true})).unwrap();
        assert!(
            state
                .respond_permission(&writer, "42", Some("allow"))
                .is_err()
        );
        assert_eq!(
            state
                .snapshot
                .pending_permission
                .as_ref()
                .unwrap()
                .request_id,
            "42"
        );
        rx.recv().unwrap();
        assert!(
            state
                .respond_permission(&writer, "second", Some("allow"))
                .is_err()
        );
        assert!(
            state
                .respond_permission(&writer, "42", Some("invented"))
                .is_err()
        );
        state
            .respond_permission(&writer, "42", Some("allow"))
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(response["id"], 42);
        assert_eq!(response["result"]["outcome"]["optionId"], "allow");
        assert_eq!(
            state
                .snapshot
                .pending_permission
                .as_ref()
                .unwrap()
                .request_id,
            "second"
        );
        state.respond_permission(&writer, "second", None).unwrap();
        let response: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(response["id"], "second");
        assert_eq!(response["result"]["outcome"]["outcome"], "cancelled");
        assert!(state.snapshot.pending_permission.is_none());
    }

    #[test]
    fn permission_overflow_and_finished_turn_get_explicit_cancellations() {
        let mut state = AcpState::new("dsh", "/project", None, None);
        for id in 0..super::MAX_PENDING_PERMISSIONS {
            assert!(
                apply_server_message(&mut state, &permission_request(json!(id)))
                    .outgoing
                    .is_empty()
            );
        }
        let overflow = apply_server_message(&mut state, &permission_request(json!(1000)));
        assert_eq!(
            overflow.outgoing,
            vec![super::permission_cancelled(json!(1000))]
        );
        assert_eq!(
            state
                .snapshot
                .pending_permission
                .as_ref()
                .unwrap()
                .request_id,
            "0"
        );
        state.active_prompt_id = Some(1001);
        let finished = apply_server_message(
            &mut state,
            &json!({"id":1001, "result":{"stopReason":"cancelled"}}),
        );
        assert_eq!(finished.outgoing.len(), super::MAX_PENDING_PERMISSIONS);
        assert!(state.snapshot.pending_permission.is_none());
        assert!(state.queued_permissions.is_empty());
    }

    #[test]
    fn handshake_session_open_stream_and_permission_roundtrip() {
        let mut state = AcpState::new("dsh", "E:\\AgentTerm", None, None);

        // 1. initialize response triggers session/new
        let init_ok = apply_server_message(
            &mut state,
            &json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": 1}}),
        );
        assert_eq!(init_ok.outgoing.len(), 1);
        assert_eq!(init_ok.outgoing[0]["method"], "session/new");
        assert_eq!(init_ok.outgoing[0]["params"]["cwd"], "E:\\AgentTerm");
        let setup_id = init_ok.outgoing[0]["id"].as_u64().unwrap();

        // 2. session/new response binds cliSessionId and enters Ready
        let open_ok = apply_server_message(
            &mut state,
            &json!({"jsonrpc": "2.0", "id": setup_id, "result": {"sessionId": "sess_abc123"}}),
        );
        assert_eq!(open_ok.bound_cli_session.as_deref(), Some("sess_abc123"));
        let snap = open_ok.emitted_snapshot.unwrap();
        assert_eq!(snap.phase, AcpPhase::Ready);
        assert_eq!(snap.cli_session_id.as_deref(), Some("sess_abc123"));

        // 3. streaming assistant chunks merge cleanly per messageId
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "sess_abc123",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "messageId": "m-1",
                        "content": { "type": "text", "text": "你好" }
                    }
                }
            }),
        );
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "sess_abc123",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "messageId": "m-1",
                        "content": { "type": "text", "text": "，正在检查。" }
                    }
                }
            }),
        );
        assert_eq!(state.snapshot.messages.len(), 1);
        assert_eq!(state.snapshot.messages[0].text, "你好，正在检查。");

        // 4. tool_call + tool_call_update
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "sess_abc123",
                    "update": {
                        "sessionUpdate": "tool_call",
                        "toolCallId": "call-1",
                        "title": "读取 package.json",
                        "name": "read_file",
                        "kind": "read",
                        "status": "pending"
                    }
                }
            }),
        );
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "sess_abc123",
                    "update": {
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": "call-1",
                        "status": "completed",
                        "rawOutput": "ok"
                    }
                }
            }),
        );
        assert_eq!(state.snapshot.tool_calls.len(), 1);
        assert_eq!(state.snapshot.tool_calls[0].status, "completed");
        assert_eq!(state.snapshot.tool_calls[0].detail.as_deref(), Some("ok"));

        // 5. permission request captures raw id and options
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 42,
                "method": "session/request_permission",
                "params": {
                    "sessionId": "sess_abc123",
                    "title": "允许运行命令？",
                    "toolCall": { "toolCallId": "call-2" },
                    "options": [
                        { "optionId": "allow-once", "name": "允许一次", "kind": "allow_once" },
                        { "optionId": "reject-once", "name": "拒绝", "kind": "reject_once" }
                    ]
                }
            }),
        );
        let perm = state.snapshot.pending_permission.as_ref().unwrap();
        assert_eq!(perm.request_id, "42");
        assert_eq!(perm.options.len(), 2);

        // 6. unknown server request gets JSON-RPC method not found reply
        let unknown = apply_server_message(
            &mut state,
            &json!({"jsonrpc": "2.0", "id": 99, "method": "fs/read_text_file", "params": {}}),
        );
        assert_eq!(unknown.outgoing.len(), 1);
        assert_eq!(unknown.outgoing[0]["error"]["code"], -32601);
    }

    #[test]
    fn parses_config_options_and_handles_updates() {
        let (project, expected) = if cfg!(windows) {
            (r"\\?\E:\AgentTerm", r"E:\AgentTerm")
        } else {
            ("/tmp/AgentTerm", "/tmp/AgentTerm")
        };
        let mut state = AcpState::new("dsh", project, None, None);
        assert_eq!(state.project_path, expected);

        let init_ok = apply_server_message(
            &mut state,
            &json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": 1}}),
        );
        let setup_id = init_ok.outgoing[0]["id"].as_u64().unwrap();
        assert_eq!(init_ok.outgoing[0]["params"]["cwd"], expected);

        let open_ok = apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": setup_id,
                "result": {
                    "sessionId": "sess_cfg_1",
                    "configOptions": [{
                        "id": "model",
                        "name": "Model",
                        "category": "model",
                        "currentValue": "[\"deepseek-official\",\"deepseek-v4-flash\"]",
                        "options": [{
                            "group": "magpie",
                            "name": "Magpie",
                            "options": [{
                                "value": "[\"magpie\",\"dsh-fe/deepseek-v4-flash\"]",
                                "name": "DeepSeek V4 Flash · dsh-fe"
                            }]
                        }]
                    }]
                }
            }),
        );
        let snap = open_ok.emitted_snapshot.unwrap();
        assert_eq!(snap.config_options.len(), 1);
        assert_eq!(snap.config_options[0].choices.len(), 1);
        assert_eq!(
            snap.config_options[0].choices[0].group.as_deref(),
            Some("Magpie")
        );

        state.pending_config_requests.insert(10);
        let set_ok = apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 10,
                "result": {
                    "configOptions": [{
                        "id": "model",
                        "name": "Model",
                        "category": "model",
                        "currentValue": "[\"magpie\",\"dsh-fe/deepseek-v4-flash\"]",
                        "options": []
                    }]
                }
            }),
        );
        assert_eq!(
            set_ok.emitted_snapshot.unwrap().config_options[0].current_value,
            "[\"magpie\",\"dsh-fe/deepseek-v4-flash\"]"
        );

        // config_option_update notification updates configOptions in snapshot
        let updated = apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "config_option_update",
                        "configOptions": [{
                            "id": "reasoning_effort",
                            "name": "Reasoning effort",
                            "category": "thought_level",
                            "currentValue": "max",
                            "options": [{"value": "max", "name": "Max"}]
                        }]
                    }
                }
            }),
        );
        assert_eq!(
            updated.emitted_snapshot.unwrap().config_options[0].current_value,
            "max"
        );

        // config option error updates snapshot.error and clears pending request id
        state.pending_config_requests.insert(11);
        let set_err = apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 11,
                "error": { "code": -32602, "message": "invalid model" }
            }),
        );
        assert!(
            set_err
                .emitted_snapshot
                .unwrap()
                .error
                .unwrap()
                .contains("切换配置失败")
        );
        assert!(!state.pending_config_requests.contains(&11));

        let handle_state = std::sync::Arc::new(std::sync::Mutex::new(state));
        let (tx, _rx) = std::sync::mpsc::sync_channel(4);
        let dummy_child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "exit 0"][..]
            } else {
                &[]
            })
            .spawn()
            .unwrap();
        let handle = super::AcpSessionHandle {
            info: crate::sessions::SessionInfo {
                id: "s-cfg".into(),
                tool_id: "dsh".into(),
                tool_name: "DeepSeek Harness".into(),
                project_path: r"E:\AgentTerm".into(),
                title: None,
                cli_session_id: Some("sess_cfg_1".into()),
            },
            #[cfg(unix)]
            pgid: None,
            #[cfg(windows)]
            job: None,
            child: std::sync::Arc::new(std::sync::Mutex::new(dummy_child)),
            writer: super::AcpWriter {
                tx,
                closed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            state: std::sync::Arc::clone(&handle_state),
            closed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        assert!(handle.set_config_option("", "x").is_err());
        handle_state.lock().unwrap().snapshot.busy = true;
        assert!(handle.set_config_option("model", "x").is_err());
        handle_state.lock().unwrap().snapshot.busy = false;
        handle_state.lock().unwrap().snapshot.phase = AcpPhase::Connecting;
        assert!(handle.set_config_option("model", "x").is_err());
        let _ = handle.terminate();
    }

    #[test]
    fn resume_sends_session_resume_with_existing_cli_id() {
        let mut state = AcpState::new(
            "dsh",
            "E:\\AgentTerm",
            Some("sess_existing_1".to_string()),
            None,
        );
        let outcome = apply_server_message(
            &mut state,
            &json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": 1}}),
        );
        assert_eq!(outcome.outgoing[0]["method"], "session/resume");
        assert_eq!(
            outcome.outgoing[0]["params"]["sessionId"],
            "sess_existing_1"
        );
    }

    #[test]
    fn bounds_messages_and_tool_calls() {
        let mut state = AcpState::new("dsh", "E:\\AgentTerm", None, None);
        for index in 0..(MAX_MESSAGES + 25) {
            state.push_message("user", format!("msg-{index}"));
        }
        assert_eq!(state.snapshot.messages.len(), MAX_MESSAGES);

        for index in 0..(MAX_TOOL_CALLS + 25) {
            apply_server_message(
                &mut state,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "update": {
                            "sessionUpdate": "tool_call",
                            "toolCallId": format!("call-{index}"),
                            "title": "op",
                            "status": "pending"
                        }
                    }
                }),
            );
        }
        assert_eq!(state.snapshot.tool_calls.len(), MAX_TOOL_CALLS);
    }

    #[test]
    fn streams_thoughts_usage_and_interleaved_timeline() {
        let mut state = AcpState::new("dsh", "E:\\AgentTerm", None, None);
        state.push_message("user", "检查配置".into());

        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "agent_thought_chunk",
                        "messageId": "m-t1",
                        "content": { "type": "text", "text": "先看" }
                    }
                }
            }),
        );
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "agent_thought_chunk",
                        "messageId": "m-t1",
                        "content": { "type": "text", "text": "目录" }
                    }
                }
            }),
        );
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "tool_call",
                        "toolCallId": "call-bash",
                        "title": "bash",
                        "status": "in_progress",
                        "rawInput": { "command": "dir" }
                    }
                }
            }),
        );
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": "call-bash",
                        "status": "completed",
                        "content": [{ "type": "text", "text": "ok.txt" }]
                    }
                }
            }),
        );
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "messageId": "m-a1",
                        "content": { "type": "text", "text": "已查看完成" }
                    }
                }
            }),
        );
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "usage_update",
                        "used": 12400,
                        "size": 1000000
                    }
                }
            }),
        );

        assert_eq!(state.snapshot.messages.len(), 3);
        assert_eq!(state.snapshot.messages[1].role, "thought");
        assert_eq!(state.snapshot.messages[1].text, "先看目录");
        assert_eq!(state.snapshot.messages[2].role, "assistant");
        assert_eq!(
            state.snapshot.tool_calls[0].input.as_deref(),
            Some("{\n  \"command\": \"dir\"\n}")
        );
        assert_eq!(
            state.snapshot.tool_calls[0].detail.as_deref(),
            Some("ok.txt")
        );
        let kinds: Vec<&str> = state
            .snapshot
            .timeline
            .iter()
            .map(|e| e.kind.as_str())
            .collect();
        assert_eq!(kinds, vec!["message", "message", "tool", "message"]);
        assert_eq!(state.snapshot.usage.as_ref().map(|u| u.used), Some(12400));
        assert_eq!(state.snapshot.usage.as_ref().map(|u| u.size), Some(1000000));

        // Leading whitespace-only message chunks do not spawn empty bubbles.
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "messageId": "m-ws",
                        "content": { "type": "text", "text": "   \n\t" }
                    }
                }
            }),
        );
        assert_eq!(state.snapshot.messages.len(), 3);
    }

    #[test]
    fn parses_available_commands_and_searches_project_files_and_images() {
        let mut state = AcpState::new("dsh", "E:\\AgentTerm", None, None);
        apply_server_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "available_commands_update",
                        "availableCommands": [{
                            "name": "review",
                            "description": "审查改动",
                            "input": { "hint": "[path]" }
                        }]
                    }
                }
            }),
        );
        assert_eq!(state.snapshot.available_commands.len(), 1);
        assert_eq!(state.snapshot.available_commands[0].name, "review");
        assert_eq!(
            state.snapshot.available_commands[0].input_hint.as_deref(),
            Some("[path]")
        );

        let ok_img = vec![super::AcpImageInput {
            mime_type: "image/png".to_string(),
            data: "AAAA".to_string(),
            name: Some("shot.png".to_string()),
        }];
        let blocks = super::validate_acp_images(&ok_img).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(
            super::format_user_prompt_display("看图", &ok_img),
            "[图片: shot.png]\n看图"
        );

        assert!(
            super::validate_acp_images(&[super::AcpImageInput {
                mime_type: "image/svg+xml".to_string(),
                data: "AAAA".to_string(),
                name: None,
            }])
            .is_err()
        );
        assert!(
            super::validate_acp_images(&[super::AcpImageInput {
                mime_type: "image/png".to_string(),
                data: "not-valid!".to_string(),
                name: None,
            }])
            .is_err()
        );

        let root = std::env::temp_dir().join(format!("agentterm-acp-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("src/App.tsx"), "").unwrap();
        std::fs::write(root.join("node_modules/pkg/ignored.ts"), "").unwrap();
        let found = super::search_project_files_in_dir(&root, "app").unwrap();
        assert_eq!(found, vec!["src/App.tsx"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
