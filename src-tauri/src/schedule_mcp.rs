use crate::scheduled_tasks::{
    MAX_TIMEOUT_MINUTES, MIN_TIMEOUT_MINUTES, ScheduleSpec, ScheduledTaskDraft, Scheduler,
    TEXT_LIMIT,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const REQUEST_LIMIT: usize = 64 * 1024;
const AI_CONFIG_LIMIT: u64 = 1024 * 1024;
const TOOL_NAME: &str = "agentterm_scheduler";
const CHANGED_EVENT: &str = "scheduled-tasks-changed";
const SCHEDULE_TOKEN_ENV: &str = "AGENTTERM_SCHEDULE_TOKEN";
const MAX_CONNECTIONS: usize = 32;
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);
const REPLAY_WINDOW: Duration = Duration::from_secs(600);

#[derive(Clone, Debug)]
struct AgentContext {
    session_id: String,
    project_path: String,
    tool_id: String,
    model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionBridge {
    endpoint: String,
    token: String,
}

impl SessionBridge {
    pub fn command_args(&self, tool_id: &str) -> Result<Vec<String>, String> {
        let config = json!({
            "mcpServers": {
                TOOL_NAME: {
                    "type": "http",
                    "url": self.endpoint,
                    "headers": {
                        "Authorization": format!("Bearer ${{{SCHEDULE_TOKEN_ENV}}}")
                    }
                }
            }
        });
        match tool_id {
            "claude" => Ok(vec!["--mcp-config".into(), config.to_string()]),
            "codex" => {
                let url = serde_json::to_string(&self.endpoint)
                    .map_err(|_| "无法生成 Codex 调度工具配置".to_string())?;
                Ok(vec![
                    "-c".into(),
                    format!("mcp_servers.{TOOL_NAME}.url={url}"),
                    "-c".into(),
                    format!(
                        "mcp_servers.{TOOL_NAME}.bearer_token_env_var=\"{SCHEDULE_TOKEN_ENV}\""
                    ),
                ])
            }
            "agy" => Ok(Vec::new()),
            _ => Err("不支持的工具".into()),
        }
    }

    #[cfg(test)]
    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn environment(&self) -> Vec<(String, String)> {
        vec![(SCHEDULE_TOKEN_ENV.into(), self.token.clone())]
    }
}

#[derive(Clone, Default)]
pub struct ScheduleMcp {
    state: Arc<Mutex<McpState>>,
}

#[derive(Default)]
struct McpState {
    scheduler: Option<Scheduler>,
    app: Option<AppHandle>,
    port: u16,
    sequence: u64,
    sessions: HashMap<String, AgentContext>,
    tokens: HashMap<String, String>,
    active_connections: usize,
    recent_calls: HashMap<String, Instant>,
    /// Keyed by `ip|token-fingerprint`: a local process guessing tokens can no
    /// longer lock every other Agent session out of the loopback endpoint.
    auth_failures: HashMap<String, (u8, Instant)>,
}

impl ScheduleMcp {
    pub fn handle(&self) -> Self {
        self.clone()
    }

    pub fn configure(&self, scheduler: Scheduler, app: AppHandle) -> Result<(), String> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("无法启动 Agent 定时任务工具：{error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("无法读取 Agent 定时任务工具地址：{error}"))?
            .port();
        {
            let mut state = self.lock()?;
            if state.scheduler.is_some() {
                return Err("Agent 定时任务工具已经启动".into());
            }
            state.scheduler = Some(scheduler);
            state.app = Some(app);
            state.port = port;
        }
        // A previous run may have exited or crashed before removing the Agy
        // entry; clear it now so Agy never points at a dead endpoint. The
        // single-instance guard means no live session owns it at this point.
        if let Err(error) = remove_agy_mcp_config() {
            eprintln!("AgentTerm could not clear a stale Agy scheduling MCP entry: {error}");
        }
        let shared = Arc::clone(&self.state);
        std::thread::Builder::new()
            .name("agentterm-schedule-mcp".into())
            .spawn(move || {
                for connection in listener.incoming() {
                    match connection {
                        Ok(stream) => {
                            let admitted = shared
                                .lock()
                                .map(|mut state| {
                                    if state.active_connections >= MAX_CONNECTIONS {
                                        false
                                    } else {
                                        state.active_connections += 1;
                                        true
                                    }
                                })
                                .unwrap_or(false);
                            if !admitted {
                                let mut stream = stream;
                                let _ = write_response(&mut stream, 429, "Too many connections");
                                continue;
                            }
                            let worker_state = Arc::clone(&shared);
                            if std::thread::Builder::new()
                                .name("agentterm-schedule-mcp-connection".into())
                                .spawn(move || {
                                    handle_connection(stream, Arc::clone(&worker_state));
                                    if let Ok(mut state) = worker_state.lock() {
                                        state.active_connections =
                                            state.active_connections.saturating_sub(1);
                                    }
                                })
                                .is_err()
                                && let Ok(mut state) = shared.lock()
                            {
                                state.active_connections =
                                    state.active_connections.saturating_sub(1);
                            }
                        }
                        Err(error) => {
                            eprintln!("AgentTerm schedule MCP accept failed: {error}");
                        }
                    }
                }
            })
            .map_err(|error| format!("无法启动 Agent 定时任务工具线程：{error}"))?;
        Ok(())
    }

    pub fn issue_session(
        &self,
        session_id: &str,
        project_path: &str,
        tool_id: &str,
        model: Option<String>,
    ) -> Result<SessionBridge, String> {
        let mut state = self.lock()?;
        if state.scheduler.is_none() {
            return Err("Agent 定时任务工具尚未启动".into());
        }
        if tool_id == "agy"
            && state
                .sessions
                .values()
                .any(|context| context.tool_id == "agy")
        {
            return Err("Agy 定时任务工具一次只能绑定一个运行中的会话".into());
        }
        state.sequence = state.sequence.wrapping_add(1);
        let token = random_token();
        let endpoint = format!("http://127.0.0.1:{}/mcp", state.port);
        if tool_id == "agy" {
            write_agy_mcp_config(&endpoint)?;
        }
        state.tokens.insert(token.clone(), session_id.to_string());
        state.sessions.insert(
            session_id.to_string(),
            AgentContext {
                session_id: session_id.to_string(),
                project_path: project_path.to_string(),
                tool_id: tool_id.to_string(),
                model,
            },
        );
        Ok(SessionBridge { endpoint, token })
    }

    pub fn revoke_session(&self, session_id: &str) {
        let remove_agy = if let Ok(mut state) = self.state.lock() {
            let removed = state.sessions.remove(session_id);
            state.tokens.retain(|_, owner| owner != session_id);
            removed.is_some_and(|context| context.tool_id == "agy")
        } else {
            false
        };
        if remove_agy && let Err(error) = remove_agy_mcp_config() {
            eprintln!("AgentTerm could not remove Agy scheduling MCP entry: {error}");
        }
    }

    pub fn cleanup(&self) {
        let remove_agy = if let Ok(mut state) = self.state.lock() {
            let remove_agy = state
                .sessions
                .values()
                .any(|context| context.tool_id == "agy");
            state.sessions.clear();
            state.tokens.clear();
            remove_agy
        } else {
            false
        };
        if remove_agy && let Err(error) = remove_agy_mcp_config() {
            eprintln!("AgentTerm could not clean Agy scheduling MCP entry: {error}");
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, McpState>, String> {
        self.state
            .lock()
            .map_err(|_| "Agent 定时任务工具暂时不可用".to_string())
    }
}

fn handle_connection(mut stream: TcpStream, state: Arc<Mutex<McpState>>) {
    let remote = stream.peer_addr().ok().map(|address| address.ip());
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            let _ = write_response(&mut stream, 400, error);
            return;
        }
    };
    if request.method != "POST" {
        let _ = write_response(
            &mut stream,
            405,
            "AgentTerm scheduling MCP only accepts POST requests",
        );
        return;
    }
    if request.path != "/mcp" {
        let _ = write_response(&mut stream, 404, "Unknown AgentTerm scheduling endpoint");
        return;
    }
    if !valid_host(request.host.as_deref(), &state)
        || !valid_origin(request.origin.as_deref(), &state)
    {
        let _ = write_response(&mut stream, 403, "Invalid scheduling MCP origin");
        return;
    }
    let fingerprint = token_fingerprint(request.authorization.as_deref());
    if remote.is_some_and(|address| auth_limited(&state, address, &fingerprint)) {
        let _ = write_response(&mut stream, 429, "Too many authentication failures");
        return;
    }
    let Some(token) = authenticated_token(request.authorization.as_deref(), &state) else {
        if let Some(address) = remote {
            record_auth_failure(&state, address, &fingerprint);
        }
        let _ = write_response(&mut stream, 404, "Unknown AgentTerm scheduling endpoint");
        return;
    };
    if let Some(address) = remote {
        clear_auth_failure(&state, address, &fingerprint);
    }
    let payload: Value = match serde_json::from_slice(&request.body) {
        Ok(payload) => payload,
        Err(_) => {
            let _ = write_json_response(
                &mut stream,
                400,
                &jsonrpc_error(Value::Null, -32700, "Parse error"),
            );
            return;
        }
    };
    let Some(id) = payload.get("id").cloned() else {
        let _ = write_response(&mut stream, 202, "");
        return;
    };
    if is_schedule_write(&payload) && !register_unique_call(&state, &token, &request.body) {
        let replay = jsonrpc_error(id, -32001, "Duplicate scheduling request");
        let _ = write_json_response(&mut stream, 409, &replay);
        return;
    }
    let response = match process_request(&payload, &token, &state) {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => jsonrpc_error(id, code, &message),
    };
    let _ = write_json_response(&mut stream, 200, &response);
}

fn authenticated_token(
    authorization: Option<&str>,
    state: &Arc<Mutex<McpState>>,
) -> Option<String> {
    let token = authorization?.strip_prefix("Bearer ")?;
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let state = state.lock().ok()?;
    state
        .tokens
        .keys()
        .find(|candidate| constant_time_eq(candidate.as_bytes(), token.as_bytes()))
        .cloned()
}

fn valid_host(host: Option<&str>, state: &Arc<Mutex<McpState>>) -> bool {
    let Ok(state) = state.lock() else {
        return false;
    };
    let expected = format!("127.0.0.1:{}", state.port);
    host.is_some_and(|host| host.eq_ignore_ascii_case(&expected))
}

fn valid_origin(origin: Option<&str>, state: &Arc<Mutex<McpState>>) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let Ok(state) = state.lock() else {
        return false;
    };
    let expected = format!("http://127.0.0.1:{}", state.port);
    origin.eq_ignore_ascii_case(&expected)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn auth_key(address: IpAddr, fingerprint: &str) -> String {
    format!("{address}|{fingerprint}")
}

/// A short, non-reversible label for the presented bearer value. Only the
/// fingerprint of a *failed* attempt is retained, so a hostile local process
/// cannot lock out sessions that use a different (valid) token.
fn token_fingerprint(authorization: Option<&str>) -> String {
    let Some(token) = authorization.and_then(|value| value.strip_prefix("Bearer ")) else {
        return "anonymous".into();
    };
    let digest = Sha256::digest(token.as_bytes());
    format!("{:x}", digest)[..16].to_string()
}

fn auth_limited(state: &Arc<Mutex<McpState>>, address: IpAddr, fingerprint: &str) -> bool {
    let Ok(state) = state.lock() else {
        return true;
    };
    state
        .auth_failures
        .get(&auth_key(address, fingerprint))
        .is_some_and(|(attempts, since)| {
            *attempts >= 20 && since.elapsed() < Duration::from_secs(60)
        })
}

fn record_auth_failure(state: &Arc<Mutex<McpState>>, address: IpAddr, fingerprint: &str) {
    let Ok(mut state) = state.lock() else {
        return;
    };
    let now = Instant::now();
    state
        .auth_failures
        .retain(|_, (_, since)| now.duration_since(*since) < Duration::from_secs(60));
    let entry = state
        .auth_failures
        .entry(auth_key(address, fingerprint))
        .or_insert((0, now));
    if now.duration_since(entry.1) >= Duration::from_secs(60) {
        *entry = (1, now);
    } else {
        entry.0 = entry.0.saturating_add(1);
    }
}

fn clear_auth_failure(state: &Arc<Mutex<McpState>>, address: IpAddr, fingerprint: &str) {
    if let Ok(mut state) = state.lock() {
        state.auth_failures.remove(&auth_key(address, fingerprint));
    }
}

fn is_schedule_write(payload: &Value) -> bool {
    payload.get("method").and_then(Value::as_str) == Some("tools/call")
        && payload
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
            == Some("schedule_task")
}

fn register_unique_call(state: &Arc<Mutex<McpState>>, token: &str, body: &[u8]) -> bool {
    let Ok(mut state) = state.lock() else {
        return false;
    };
    let now = Instant::now();
    state
        .recent_calls
        .retain(|_, created| now.duration_since(*created) <= REPLAY_WINDOW);
    let mut digest = Sha256::new();
    digest.update(token.as_bytes());
    digest.update([0]);
    digest.update(body);
    let key = format!("{:x}", digest.finalize());
    if state.recent_calls.contains_key(&key) {
        return false;
    }
    state.recent_calls.insert(key, now);
    true
}

fn process_request(
    payload: &Value,
    token: &str,
    shared: &Arc<Mutex<McpState>>,
) -> Result<Value, (i64, String)> {
    if payload.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err((-32600, "Invalid Request".into()));
    }
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32600, "Missing method".into()))?;
    let (scheduler, app, context) = {
        let state = shared
            .lock()
            .map_err(|_| (-32603, "Agent 定时任务工具暂时不可用".into()))?;
        let session_id = state
            .tokens
            .get(token)
            .ok_or_else(|| (-32600, "会话授权已失效".into()))?;
        let context = state
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| (-32600, "会话上下文已失效".into()))?;
        (
            state
                .scheduler
                .as_ref()
                .map(Scheduler::handle)
                .ok_or_else(|| (-32603, "调度器尚未初始化".into()))?,
            state.app.clone(),
            context,
        )
    };
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {
                "name": "agentterm-scheduler",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": "AgentTerm 已提供持久定时任务工具。用户要求定时执行时，调用 schedule_task。频率或时间不明确时先向用户确认，不要把“每天”自行改成每小时。任务创建后保持待确认，必须由用户在 AgentTerm 面板启用。prompt 只写到点后要执行的工作，不要重复“每天几点”等排期描述。项目、CLI 和来源会话由当前 AgentTerm 会话自动绑定。"
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions()})),
        "tools/call" => {
            let params = payload
                .get("params")
                .and_then(Value::as_object)
                .ok_or_else(|| (-32602, "tools/call 缺少 params".into()))?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| (-32602, "tools/call 缺少工具名称".into()))?;
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let (text, structured) = match name {
                "schedule_task" => create_schedule(&scheduler, &context, &arguments)
                    .map_err(|message| (-32000, message))?,
                "list_scheduled_tasks" => {
                    list_schedules(&scheduler, &context).map_err(|message| (-32000, message))?
                }
                _ => return Err((-32601, format!("未知工具：{name}"))),
            };
            if name == "schedule_task"
                && let Some(app) = app
            {
                let _ = app.emit(CHANGED_EVENT, ());
            }
            Ok(json!({
                "content": [{"type": "text", "text": text}],
                "structuredContent": structured,
                "isError": false
            }))
        }
        _ => Err((-32601, format!("不支持的方法：{method}"))),
    }
}

fn create_schedule(
    scheduler: &Scheduler,
    context: &AgentContext,
    arguments: &Value,
) -> Result<(String, Value), String> {
    let object = arguments
        .as_object()
        .ok_or_else(|| "schedule_task 参数必须是对象".to_string())?;
    let prompt = required_string(object, "prompt")?;
    let schedule: ScheduleSpec = serde_json::from_value(
        object
            .get("schedule")
            .cloned()
            .ok_or_else(|| "schedule_task 缺少 schedule".to_string())?,
    )
    .map_err(|error| format!("schedule 格式无效：{error}"))?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_name(&prompt));
    let timeout_minutes = object
        .get("timeoutMinutes")
        .and_then(Value::as_u64)
        .unwrap_or(20);
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| context.model.clone());
    let task = scheduler.save(ScheduledTaskDraft {
        name,
        project_path: context.project_path.clone(),
        tool_id: context.tool_id.clone(),
        model,
        prompt,
        schedule,
        timeout_minutes,
        enabled: false,
        approval_pending: true,
        source_session_id: Some(context.session_id.clone()),
    })?;
    let text = format!(
        "定时任务“{}”已创建，正在等待用户在 AgentTerm 面板确认启用。计划时间：{}。",
        task.name,
        format_time(task.next_run_at_ms)
    );
    Ok((
        text,
        serde_json::to_value(task).map_err(|_| "无法编码定时任务结果".to_string())?,
    ))
}

fn list_schedules(
    scheduler: &Scheduler,
    context: &AgentContext,
) -> Result<(String, Value), String> {
    let tasks = scheduler.list()?;
    let current: Vec<_> = tasks
        .into_iter()
        .filter(|task| task.source_session_id.as_deref() == Some(&context.session_id))
        .collect();
    let text = if current.is_empty() {
        "当前会话还没有创建定时任务。".to_string()
    } else {
        format!("当前会话共有 {} 个定时任务。", current.len())
    };
    Ok((
        text,
        serde_json::to_value(current).map_err(|_| "无法编码定时任务列表".to_string())?,
    ))
}

fn required_string(object: &serde_json::Map<String, Value>, name: &str) -> Result<String, String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("schedule_task 缺少 {name}"))
}

fn default_name(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or(prompt).trim();
    let value: String = line.chars().take(32).collect();
    if value.is_empty() {
        "定时任务".into()
    } else {
        value
    }
}

fn format_time(value: u64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_millis_opt(value as i64)
        .single()
        .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "时间未知".into())
}

fn agy_mcp_config_path() -> Result<PathBuf, String> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| format!("无法定位 Agy 配置目录：{variable} 未设置"))?;
    Ok(PathBuf::from(home)
        .join(".gemini")
        .join("config")
        .join("mcp_config.json"))
}

fn write_agy_mcp_config(endpoint: &str) -> Result<(), String> {
    write_agy_mcp_config_at(&agy_mcp_config_path()?, endpoint)
}

fn write_agy_mcp_config_at(path: &Path, endpoint: &str) -> Result<(), String> {
    let mut root = read_agy_mcp_config(path)?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Default::default()));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| "Agy mcpServers 配置不是对象".to_string())?;
    servers.insert(
        TOOL_NAME.into(),
        json!({
            "type": "http",
            "url": endpoint,
            "headers": {
                "Authorization": format!("Bearer ${{{SCHEDULE_TOKEN_ENV}}}")
            }
        }),
    );
    write_json_atomic(path, &Value::Object(root))
}

fn remove_agy_mcp_config() -> Result<(), String> {
    remove_agy_mcp_config_at(&agy_mcp_config_path()?)
}

fn remove_agy_mcp_config_at(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_agy_mcp_config(path)?;
    let Some(servers) = root.get_mut("mcpServers") else {
        return Ok(());
    };
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| "Agy mcpServers 配置不是对象".to_string())?;
    if servers.remove(TOOL_NAME).is_none() {
        return Ok(());
    }
    write_json_atomic(path, &Value::Object(root))
}

fn read_agy_mcp_config(path: &Path) -> Result<serde_json::Map<String, Value>, String> {
    if !path.exists() {
        return Ok(serde_json::Map::new());
    }
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("无法读取 Agy MCP 配置：{error}"))?;
    if !metadata.is_file() || metadata.len() > AI_CONFIG_LIMIT {
        return Err("Agy MCP 配置不是普通文件或超过 1 MiB".into());
    }
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("无法读取 Agy MCP 配置：{error}"))?;
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    serde_json::from_str::<Value>(&text)
        .map_err(|error| format!("Agy MCP 配置不是有效 JSON：{error}"))?
        .as_object()
        .cloned()
        .ok_or_else(|| "Agy MCP 配置顶层必须是对象".to_string())
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Agy MCP 配置路径无效".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("无法创建 Agy MCP 配置目录：{error}"))?;
    let mut temporary: OsString = path.as_os_str().to_owned();
    temporary.push(".agentterm.tmp");
    let temporary = PathBuf::from(temporary);
    let mut body =
        serde_json::to_vec_pretty(value).map_err(|_| "无法编码 Agy MCP 配置".to_string())?;
    body.push(b'\n');
    std::fs::write(&temporary, body).map_err(|error| format!("无法写入 Agy MCP 配置：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("无法保护 Agy MCP 配置：{error}"))?;
    }
    std::fs::rename(&temporary, path).map_err(|error| format!("无法替换 Agy MCP 配置：{error}"))
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "schedule_task",
            "description": "When the user asks to run something later or repeatedly in AgentTerm, create a persistent scheduled task in pending-confirmation state. Ask the user for missing frequency or exact daily time before calling. The current project, CLI, and source session are supplied automatically; the user must confirm it in AgentTerm before it can run.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": {"type": "string", "description": "Short human-readable task name."},
                    "prompt": {"type": "string", "description": "Complete work instruction for the selected CLI at execution time. Omit scheduling phrases such as '每天九点'.", "maxLength": TEXT_LIMIT},
                    "schedule": {
                        "oneOf": [
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "kind": {"const": "interval"},
                                    "minutes": {"type": "integer", "minimum": 1, "maximum": 10080}
                                },
                                "required": ["kind", "minutes"]
                            },
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "kind": {"const": "daily"},
                                    "hour": {"type": "integer", "minimum": 0, "maximum": 23},
                                    "minute": {"type": "integer", "minimum": 0, "maximum": 59}
                                },
                                "required": ["kind", "hour", "minute"]
                            }
                        ]
                    },
                    "model": {"type": "string", "description": "Optional CLI model override. Omit to use the CLI default."},
                    "timeoutMinutes": {"type": "integer", "minimum": MIN_TIMEOUT_MINUTES, "maximum": MAX_TIMEOUT_MINUTES, "default": 20}
                },
                "required": ["prompt", "schedule"]
            }
        }),
        json!({
            "name": "list_scheduled_tasks",
            "description": "List persistent scheduled tasks created from the current AgentTerm conversation.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }
        }),
    ]
}

struct HttpRequest {
    method: String,
    path: String,
    host: Option<String>,
    origin: Option<String>,
    authorization: Option<String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, &'static str> {
    let deadline = Instant::now() + REQUEST_DEADLINE;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        if let Some(index) = find_header_end(&bytes) {
            break index;
        }
        if bytes.len() >= REQUEST_LIMIT {
            return Err("Request headers too large");
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or("Request deadline exceeded")?;
        let _ = stream.set_read_timeout(Some(remaining.min(Duration::from_secs(3))));
        let read = stream
            .read(&mut chunk)
            .map_err(|_| "Unable to read request")?;
        if read == 0 {
            return Err("Incomplete request");
        }
        bytes.extend_from_slice(&chunk[..read]);
    };
    let header =
        std::str::from_utf8(&bytes[..header_end]).map_err(|_| "Request headers are not UTF-8")?;
    let mut lines = header.split("\r\n");
    let request_line = lines.next().ok_or("Missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("Missing HTTP method")?.to_string();
    let target = parts.next().ok_or("Missing request target")?;
    let path = target.split('?').next().unwrap_or(target).to_string();
    let mut content_length = 0usize;
    let mut host = None;
    let mut origin = None;
    let mut authorization = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().map_err(|_| "Invalid Content-Length")?;
            } else if name.eq_ignore_ascii_case("host") {
                host = Some(value.to_string());
            } else if name.eq_ignore_ascii_case("origin") {
                origin = Some(value.to_string());
            } else if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value.to_string());
            }
        }
    }
    let body_start = header_end + 4;
    if content_length > REQUEST_LIMIT || body_start + content_length > REQUEST_LIMIT {
        return Err("Request body too large");
    }
    while bytes.len() < body_start + content_length {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or("Request deadline exceeded")?;
        let _ = stream.set_read_timeout(Some(remaining.min(Duration::from_secs(3))));
        let read = stream
            .read(&mut chunk)
            .map_err(|_| "Unable to read request body")?;
        if read == 0 {
            return Err("Incomplete request body");
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > REQUEST_LIMIT {
            return Err("Request body too large");
        }
    }
    Ok(HttpRequest {
        method,
        path,
        host,
        origin,
        authorization,
        body: bytes[body_start..body_start + content_length].to_vec(),
    })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_json_response(stream: &mut TcpStream, status: u16, body: &Value) -> std::io::Result<()> {
    let body = body.to_string();
    write_response_with_type(
        stream,
        status,
        "application/json; charset=utf-8",
        body.as_bytes(),
    )
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    write_response_with_type(stream, status, "text/plain; charset=utf-8", body.as_bytes())
}

fn write_response_with_type(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        403 => "Forbidden",
        429 => "Too Many Requests",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn jsonrpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message}
    })
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    if getrandom::fill(&mut bytes).is_err() {
        // Entropy failure is unrecoverable for an authenticated endpoint.
        panic!("operating-system randomness is unavailable");
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        AgentContext, McpState, ScheduleMcp, SessionBridge, create_schedule, list_schedules,
        process_request,
    };
    use crate::scheduled_tasks::Scheduler;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn session_bridge_uses_http_mcp_for_every_supported_cli() {
        let bridge = SessionBridge {
            endpoint: "http://127.0.0.1:43210/mcp".into(),
            token: "ab".repeat(32),
        };
        let claude = bridge.command_args("claude").unwrap();
        assert_eq!(claude[0], "--mcp-config");
        assert!(claude[1].contains("\"type\":\"http\""));
        assert!(claude[1].contains(bridge.endpoint()));
        assert!(claude[1].contains("${AGENTTERM_SCHEDULE_TOKEN}"));
        assert!(!claude[1].contains(&bridge.token));

        let codex = bridge.command_args("codex").unwrap();
        assert_eq!(codex[0], "-c");
        assert!(codex[1].contains("mcp_servers.agentterm_scheduler.url="));
        assert!(codex[1].contains(bridge.endpoint()));
        assert!(codex[3].contains("bearer_token_env_var"));
        assert!(!codex.iter().any(|arg| arg.contains(&bridge.token)));
        assert_eq!(
            bridge.environment(),
            vec![("AGENTTERM_SCHEDULE_TOKEN".into(), bridge.token.clone())]
        );

        let agy = bridge.command_args("agy").unwrap();
        assert!(agy.is_empty());
    }

    #[test]
    fn schedule_mcp_requires_configuration_before_issuing_sessions() {
        let mcp = ScheduleMcp::default();
        assert!(
            mcp.issue_session("session-1", "E:\\AgentTerm", "codex", None)
                .is_err()
        );
    }

    #[test]
    fn mcp_schedule_tool_binds_the_current_agent_context() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-agent-source-{}-{}.json",
            std::process::id(),
            super::random_token()
        ));
        let scheduler = Scheduler::default();
        scheduler.configure(path).unwrap();
        let context = AgentContext {
            session_id: "session-42".into(),
            project_path: std::env::current_dir().unwrap().display().to_string(),
            tool_id: "codex".into(),
            model: Some("gpt-5".into()),
        };
        let (text, task) = create_schedule(
            &scheduler,
            &context,
            &json!({
                "name": "每日汇报",
                "prompt": "汇总今天的进展",
                "schedule": {"kind": "daily", "hour": 9, "minute": 0}
            }),
        )
        .unwrap();
        let normalized_project =
            crate::projects::normalize_project_dir(&context.project_path).unwrap();
        assert!(text.contains("已创建"));
        assert_eq!(task["sourceSessionId"], "session-42");
        assert_eq!(
            task["projectPath"],
            normalized_project.display().to_string()
        );
        assert_eq!(task["toolId"], "codex");
        assert_eq!(task["model"], "gpt-5");

        let (_, listed) = list_schedules(&scheduler, &context).unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
    }

    #[test]
    fn mcp_json_rpc_dispatches_schedule_task() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-agent-rpc-{}-{}.json",
            std::process::id(),
            super::random_token()
        ));
        let scheduler = Scheduler::default();
        scheduler.configure(path).unwrap();
        let context = AgentContext {
            session_id: "session-rpc".into(),
            project_path: std::env::current_dir().unwrap().display().to_string(),
            tool_id: "agy".into(),
            model: None,
        };
        let shared = Arc::new(Mutex::new(McpState {
            scheduler: Some(scheduler.handle()),
            app: None,
            port: 0,
            sequence: 1,
            sessions: HashMap::from([(context.session_id.clone(), context)]),
            tokens: HashMap::from([("ab".repeat(32), "session-rpc".into())]),
            active_connections: 0,
            recent_calls: HashMap::new(),
            auth_failures: HashMap::new(),
        }));
        let response = process_request(
            &json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "schedule_task",
                    "arguments": {
                        "name": "晚间检查",
                        "prompt": "检查今天失败的任务",
                        "schedule": {"kind": "daily", "hour": 20, "minute": 30}
                    }
                }
            }),
            "ab".repeat(32).as_str(),
            &shared,
        )
        .unwrap();
        assert_eq!(response["isError"], false);
        assert_eq!(
            response["structuredContent"]["toolId"],
            Value::String("agy".into())
        );
    }

    #[test]
    fn agy_config_merge_preserves_other_servers_and_removes_only_agentterm() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-agy-mcp-{}-{}.json",
            std::process::id(),
            super::random_token()
        ));
        std::fs::write(
            &path,
            r#"{"mcpServers":{"existing":{"type":"http","url":"http://127.0.0.1:1/mcp"}}}"#,
        )
        .unwrap();
        super::write_agy_mcp_config_at(&path, "http://127.0.0.1:2/mcp/server/session").unwrap();
        let configured: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            configured["mcpServers"]["agentterm_scheduler"]["url"],
            "http://127.0.0.1:2/mcp/server/session"
        );
        assert_eq!(
            configured["mcpServers"]["agentterm_scheduler"]["headers"]["Authorization"],
            "Bearer ${AGENTTERM_SCHEDULE_TOKEN}"
        );
        assert_eq!(
            configured["mcpServers"]["existing"]["url"],
            "http://127.0.0.1:1/mcp"
        );

        super::remove_agy_mcp_config_at(&path).unwrap();
        let restored: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(restored["mcpServers"].get("agentterm_scheduler").is_none());
        assert_eq!(
            restored["mcpServers"]["existing"]["url"],
            "http://127.0.0.1:1/mcp"
        );
    }

    #[test]
    fn random_tokens_are_256_bit_and_do_not_repeat() {
        let first = super::random_token();
        let second = super::random_token();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn bearer_auth_requires_an_exact_session_token_and_limits_failures() {
        let token = super::random_token();
        let shared = Arc::new(Mutex::new(McpState {
            tokens: HashMap::from([(token.clone(), "session-auth".into())]),
            ..McpState::default()
        }));
        assert_eq!(
            super::authenticated_token(Some(&format!("Bearer {token}")), &shared),
            Some(token.clone())
        );
        assert!(super::authenticated_token(Some("Bearer wrong"), &shared).is_none());

        let address = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
        let failed = super::token_fingerprint(Some("Bearer wrong"));
        for _ in 0..20 {
            super::record_auth_failure(&shared, address, &failed);
        }
        assert!(super::auth_limited(&shared, address, &failed));
    }

    #[test]
    fn a_token_guess_lockout_does_not_block_another_session() {
        let shared = Arc::new(Mutex::new(McpState::default()));
        let address = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
        let attacker = super::token_fingerprint(Some(&format!("Bearer {}", "a".repeat(64))));
        let legitimate = super::token_fingerprint(Some(&format!("Bearer {}", "b".repeat(64))));
        for _ in 0..25 {
            super::record_auth_failure(&shared, address, &attacker);
        }
        assert!(super::auth_limited(&shared, address, &attacker));
        assert!(!super::auth_limited(&shared, address, &legitimate));
    }
}
