use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::projects::normalize_project_dir;
use crate::tools::{
    candidate_search_dirs, is_windows_batch, resolve_binary, windows_cmd_executable,
};

const STORE_BYTES: u64 = 1024 * 1024;
const TASK_LIMIT: usize = 50;
const RUN_LIMIT: usize = 10;
const OUTPUT_LIMIT: usize = 12_000;
const MIN_INTERVAL_MINUTES: u64 = 1;
const MAX_INTERVAL_MINUTES: u64 = 10_080;
const MIN_TIMEOUT_MINUTES: u64 = 1;
const MAX_TIMEOUT_MINUTES: u64 = 180;
const TEXT_LIMIT: usize = 2_000;
const MODEL_LIMIT: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    pub project_path: String,
    pub tool_id: String,
    pub model: Option<String>,
    pub prompt: String,
    pub interval_minutes: u64,
    pub timeout_minutes: u64,
    pub enabled: bool,
    pub next_run_at_ms: u64,
    pub runs: Vec<ScheduledRun>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledRun {
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub status: String,
    pub exit_code: Option<i32>,
    pub output: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledTaskDraft {
    pub name: String,
    pub project_path: String,
    pub tool_id: String,
    pub model: Option<String>,
    pub prompt: String,
    pub interval_minutes: u64,
    pub timeout_minutes: u64,
    pub enabled: bool,
}

#[derive(Default)]
pub struct Scheduler {
    state: Arc<Mutex<SchedulerInner>>,
}

#[derive(Default)]
struct SchedulerInner {
    path: Option<PathBuf>,
    tasks: Vec<ScheduledTask>,
    running: bool,
}

impl Scheduler {
    pub fn handle(&self) -> Scheduler {
        Scheduler {
            state: Arc::clone(&self.state),
        }
    }

    pub fn configure(&self, path: PathBuf) -> Result<(), String> {
        let mut inner = self.lock()?;
        if inner.path.is_some() {
            return Err("定时任务存储已经配置，不能切换".into());
        }
        let parent = path
            .parent()
            .filter(|parent| parent.is_dir())
            .ok_or_else(|| "定时任务存储父目录必须已经存在".to_string())?;
        let parent = parent.canonicalize().map_err(storage_error)?;
        let name = path
            .file_name()
            .ok_or_else(|| "定时任务存储路径无效".to_string())?;
        inner.path = Some(parent.join(name));
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<ScheduledTask>, String> {
        let mut inner = self.lock()?;
        load_if_configured(&mut inner)?;
        Ok(inner.tasks.clone())
    }

    pub fn save(&self, draft: ScheduledTaskDraft) -> Result<ScheduledTask, String> {
        let mut task = normalize_draft(draft, None)?;
        let mut inner = self.lock()?;
        load_if_configured(&mut inner)?;
        if inner.tasks.len() >= TASK_LIMIT {
            return Err(format!("定时任务最多 {TASK_LIMIT} 个"));
        }
        task.id = unique_id(&inner.tasks);
        inner.tasks.insert(0, task.clone());
        persist(&inner)?;
        Ok(task)
    }

    pub fn update(&self, id: &str, draft: ScheduledTaskDraft) -> Result<ScheduledTask, String> {
        let mut inner = self.lock()?;
        load_if_configured(&mut inner)?;
        let existing = inner
            .tasks
            .iter()
            .find(|task| task.id == id)
            .cloned()
            .ok_or_else(|| "定时任务不存在".to_string())?;
        let updated = normalize_draft(draft, Some(&existing))?;
        let task = inner
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| "定时任务不存在".to_string())?;
        *task = updated.clone();
        persist(&inner)?;
        Ok(updated)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<ScheduledTask, String> {
        let mut inner = self.lock()?;
        load_if_configured(&mut inner)?;
        let task = inner
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| "定时任务不存在".to_string())?;
        task.enabled = enabled;
        if enabled {
            task.next_run_at_ms = now_ms().saturating_add(task.interval_minutes * 60_000);
        }
        let saved = task.clone();
        persist(&inner)?;
        Ok(saved)
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut inner = self.lock()?;
        load_if_configured(&mut inner)?;
        let before = inner.tasks.len();
        inner.tasks.retain(|task| task.id != id);
        if inner.tasks.len() == before {
            return Err("定时任务不存在".into());
        }
        persist(&inner)
    }

    pub fn run_now(&self, id: &str) -> Result<ScheduledTask, String> {
        let task = {
            let mut inner = self.lock()?;
            load_if_configured(&mut inner)?;
            if inner.running {
                return Err("已有定时任务正在运行，请等待结束后再试".into());
            }
            let task = inner
                .tasks
                .iter()
                .find(|task| task.id == id)
                .cloned()
                .ok_or_else(|| "定时任务不存在".to_string())?;
            inner.running = true;
            task
        };
        let run = execute_task(&task);
        let mut inner = self.lock()?;
        inner.running = false;
        load_if_configured(&mut inner)?;
        let saved = if let Some(current) = inner.tasks.iter_mut().find(|item| item.id == id) {
            current.runs.insert(0, run);
            current.runs.truncate(RUN_LIMIT);
            current.next_run_at_ms = now_ms().saturating_add(current.interval_minutes * 60_000);
            current.clone()
        } else {
            return Err("定时任务已在运行期间删除".into());
        };
        persist(&inner)?;
        Ok(saved)
    }

    pub fn tick(&self) -> Result<(), String> {
        let due = {
            let mut inner = self.lock()?;
            load_if_configured(&mut inner)?;
            if inner.running {
                return Ok(());
            }
            let now = now_ms();
            let task = inner
                .tasks
                .iter()
                .filter(|task| task.enabled && task.next_run_at_ms <= now)
                .min_by_key(|task| task.next_run_at_ms)
                .cloned();
            if task.is_some() {
                inner.running = true;
            }
            task
        };
        let Some(task) = due else {
            return Ok(());
        };
        let run = execute_task(&task);
        let mut inner = self.lock()?;
        inner.running = false;
        load_if_configured(&mut inner)?;
        if let Some(current) = inner.tasks.iter_mut().find(|item| item.id == task.id) {
            current.runs.insert(0, run);
            current.runs.truncate(RUN_LIMIT);
            current.next_run_at_ms = now_ms().saturating_add(current.interval_minutes * 60_000);
            persist(&inner)?;
        }
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, SchedulerInner>, String> {
        self.state
            .lock()
            .map_err(|_| "定时任务暂时不可用，请重试".to_string())
    }
}

fn load_if_configured(inner: &mut SchedulerInner) -> Result<(), String> {
    let Some(path) = inner.path.clone() else {
        return Ok(());
    };
    inner.tasks = read_store(&path)?;
    Ok(())
}

fn persist(inner: &SchedulerInner) -> Result<(), String> {
    let Some(path) = &inner.path else {
        return Ok(());
    };
    write_store(path, &inner.tasks)
}

fn normalize_draft(
    draft: ScheduledTaskDraft,
    existing: Option<&ScheduledTask>,
) -> Result<ScheduledTask, String> {
    let name = clean_text(&draft.name, "任务名称", 80)?;
    let prompt = clean_text(&draft.prompt, "任务指令", TEXT_LIMIT)?;
    let model = match draft
        .model
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(value) => Some(clean_token(value, "模型名称", MODEL_LIMIT)?),
        None => None,
    };
    if !matches!(draft.tool_id.as_str(), "claude" | "codex" | "agy") {
        return Err("请选择 Claude Code、Codex 或 Agy".into());
    }
    if !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(&draft.interval_minutes) {
        return Err("运行间隔需在 1 分钟到 7 天之间".into());
    }
    if !(MIN_TIMEOUT_MINUTES..=MAX_TIMEOUT_MINUTES).contains(&draft.timeout_minutes) {
        return Err("单次超时需在 1 到 180 分钟之间".into());
    }
    let project = normalize_project_dir(&draft.project_path)?;
    let now = now_ms();
    let next = if draft.enabled {
        existing
            .filter(|task| task.enabled && task.interval_minutes == draft.interval_minutes)
            .map(|task| task.next_run_at_ms)
            .unwrap_or_else(|| now.saturating_add(draft.interval_minutes * 60_000))
    } else {
        existing.map(|task| task.next_run_at_ms).unwrap_or(now)
    };
    Ok(ScheduledTask {
        id: existing.map(|task| task.id.clone()).unwrap_or_default(),
        name,
        project_path: project.display().to_string(),
        tool_id: draft.tool_id,
        model,
        prompt,
        interval_minutes: draft.interval_minutes,
        timeout_minutes: draft.timeout_minutes,
        enabled: draft.enabled,
        next_run_at_ms: next,
        runs: existing.map(|task| task.runs.clone()).unwrap_or_default(),
    })
}

fn clean_text(value: &str, label: &str, limit: usize) -> Result<String, String> {
    let text = value.trim().replace("\r\n", "\n").replace('\r', "\n");
    if text.is_empty() {
        return Err(format!("{label}不能为空"));
    }
    if text.chars().count() > limit
        || text
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return Err(format!("{label}包含无效字符或超过 {limit} 个字符"));
    }
    Ok(text)
}

fn clean_token(value: &str, label: &str, limit: usize) -> Result<String, String> {
    if value.chars().count() > limit
        || value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(format!("{label}只能是一行且最多 {limit} 个字符"));
    }
    Ok(value.to_string())
}

fn unique_id(tasks: &[ScheduledTask]) -> String {
    let millis = now_ms();
    let mut sequence = 0u32;
    loop {
        let id = format!("task-{millis}-{sequence}");
        if tasks.iter().all(|task| task.id != id) {
            return id;
        }
        sequence += 1;
    }
}

fn execute_task(task: &ScheduledTask) -> ScheduledRun {
    let started = now_ms();
    let result = run_process(task);
    let finished = now_ms();
    match result {
        Ok((code, output)) => ScheduledRun {
            started_at_ms: started,
            finished_at_ms: finished,
            status: if code == 0 { "成功" } else { "失败" }.into(),
            exit_code: Some(code),
            output,
        },
        Err(error) => ScheduledRun {
            started_at_ms: started,
            finished_at_ms: finished,
            status: "失败".into(),
            exit_code: None,
            output: error,
        },
    }
}

fn run_process(task: &ScheduledTask) -> Result<(i32, String), String> {
    let binary = resolve_tool(&task.tool_id)?;
    let cwd = normalize_project_dir(&task.project_path)?;
    let (program, mut args) = command_for_tool(&binary, &task.tool_id, task.model.as_deref())?;
    args.push(task.prompt.clone());
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动定时任务：{error}"))?;
    let timeout = Duration::from_secs(task.timeout_minutes.saturating_mul(60));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "任务超时（{} 分钟），已停止本次运行",
                    task.timeout_minutes
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                let _ = child.kill();
                return Err(format!("无法确认任务状态：{error}"));
            }
        }
    };
    let mut output = String::new();
    if let Some(mut stdout) = child.stdout {
        let _ = stdout.read_to_string(&mut output);
    }
    if let Some(mut stderr) = child.stderr {
        let mut err = String::new();
        let _ = stderr.read_to_string(&mut err);
        if !err.trim().is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&err);
        }
    }
    Ok((status.code().unwrap_or(-1), trim_output(&output)))
}

fn resolve_tool(tool_id: &str) -> Result<PathBuf, String> {
    let binaries = match tool_id {
        "claude" => &["claude"][..],
        "codex" => &["codex"][..],
        "agy" => &["agy"][..],
        _ => return Err("不支持的工具".into()),
    };
    match resolve_binary(binaries, &candidate_search_dirs()) {
        crate::tools::ResolveResult::Found(path) => Ok(path),
        crate::tools::ResolveResult::NotExecutable(path) => {
            Err(format!("工具没有执行权限：{}", path.display()))
        }
        crate::tools::ResolveResult::Missing => Err("未安装所选工具".into()),
    }
}

fn command_for_tool(
    binary: &Path,
    tool_id: &str,
    model: Option<&str>,
) -> Result<(PathBuf, Vec<String>), String> {
    let binary_arg = binary.display().to_string();
    let mut args = Vec::new();
    let program = if cfg!(windows) && is_windows_batch(binary) {
        args.extend(["/D".into(), "/C".into(), binary_arg]);
        windows_cmd_executable()
    } else {
        binary.to_path_buf()
    };
    match tool_id {
        "claude" => {
            args.push("--print".into());
            if let Some(model) = model {
                args.extend(["--model".into(), model.into()]);
            }
        }
        "codex" => {
            args.push("exec".into());
            if let Some(model) = model {
                args.extend(["--model".into(), model.into()]);
            }
        }
        "agy" => {
            args.push("--print".into());
            if let Some(model) = model {
                args.extend(["--model".into(), model.into()]);
            }
        }
        _ => return Err("不支持的工具".into()),
    }
    Ok((program, args))
}

fn trim_output(value: &str) -> String {
    let text = value.trim();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= OUTPUT_LIMIT {
        return text.to_string();
    }
    chars[chars.len() - OUTPUT_LIMIT..].iter().collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreRecord {
    schema: u32,
    revision: u64,
    tasks: Vec<ScheduledTask>,
    sha256: String,
}

fn read_store(path: &Path) -> Result<Vec<ScheduledTask>, String> {
    Ok(read_locked(path)?.1)
}

fn write_store(path: &Path, tasks: &[ScheduledTask]) -> Result<(), String> {
    validate_tasks(tasks)?;
    let _lock = lock_store(path)?;
    let (revision, _) = read_existing(path)?;
    let record = StoreRecord {
        schema: 1,
        revision: revision + 1,
        sha256: hash(revision + 1, tasks)?,
        tasks: tasks.to_vec(),
    };
    let mut line = serde_json::to_vec(&record).map_err(|_| "定时任务无法保存".to_string())?;
    line.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(storage_error)?;
    file.write_all(&line).map_err(storage_error)?;
    file.sync_all().map_err(storage_error)
}

fn read_locked(path: &Path) -> Result<(u64, Vec<ScheduledTask>), String> {
    let _lock = lock_store(path)?;
    read_existing(path)
}

fn read_existing(path: &Path) -> Result<(u64, Vec<ScheduledTask>), String> {
    if !path.exists() {
        return Ok((0, Vec::new()));
    }
    let file = File::open(path).map_err(storage_error)?;
    let mut bytes = Vec::new();
    file.take(STORE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(storage_error)?;
    if bytes.len() as u64 > STORE_BYTES {
        return Err("定时任务存储超过 1 MiB".into());
    }
    if bytes.is_empty() {
        return Ok((0, Vec::new()));
    }
    if !bytes.ends_with(b"\n") {
        return Err("定时任务存储校验失败".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "定时任务存储不是有效文本".to_string())?;
    let mut revision = 0u64;
    let mut tasks = Vec::new();
    for line in text.split_terminator('\n') {
        let record: StoreRecord =
            serde_json::from_str(line).map_err(|_| "定时任务存储校验失败".to_string())?;
        if record.schema != 1
            || revision + 1 != record.revision
            || record.sha256 != hash(record.revision, &record.tasks)?
        {
            return Err("定时任务存储校验失败".into());
        }
        validate_tasks(&record.tasks)?;
        revision = record.revision;
        tasks = record.tasks;
    }
    Ok((revision, tasks))
}

struct StoreLock(File);
impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn lock_store(path: &Path) -> Result<StoreLock, String> {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(PathBuf::from(name))
        .map_err(storage_error)?;
    file.try_lock()
        .map_err(|_| "定时任务正被另一操作使用，请重试".to_string())?;
    Ok(StoreLock(file))
}

fn hash(revision: u64, tasks: &[ScheduledTask]) -> Result<String, String> {
    let bytes =
        serde_json::to_vec(&(revision, tasks)).map_err(|_| "定时任务无法校验".to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn validate_tasks(tasks: &[ScheduledTask]) -> Result<(), String> {
    if tasks.len() > TASK_LIMIT {
        return Err("定时任务数量超过上限".into());
    }
    let mut seen = std::collections::HashSet::new();
    for task in tasks {
        if !seen.insert(&task.id) || task.runs.len() > RUN_LIMIT {
            return Err("定时任务记录无效".into());
        }
    }
    Ok(())
}

fn storage_error(error: std::io::Error) -> String {
    format!("定时任务存储读写失败：{error}")
}

#[cfg(test)]
mod tests {
    use super::{ScheduledTaskDraft, Scheduler};
    use std::fs;
    use std::path::Path;

    #[test]
    fn scheduled_commands_leave_permissions_to_each_agent() {
        let claude = super::command_for_tool(Path::new("claude"), "claude", None)
            .unwrap()
            .1;
        assert!(claude.iter().any(|arg| arg == "--print"));
        assert!(!claude.iter().any(|arg| {
            arg.contains("permission") || arg.contains("sandbox") || arg.contains("dangerously")
        }));

        let codex = super::command_for_tool(Path::new("codex"), "codex", None)
            .unwrap()
            .1;
        assert!(codex.iter().any(|arg| arg == "exec"));
        assert!(!codex.iter().any(|arg| {
            arg.contains("sandbox") || arg.contains("skip-git") || arg.contains("dangerously")
        }));

        let agy = super::command_for_tool(Path::new("agy"), "agy", None)
            .unwrap()
            .1;
        assert!(agy.iter().any(|arg| arg == "--print"));
        assert!(
            !agy.iter().any(|arg| {
                arg == "--mode" || arg == "--sandbox" || arg.contains("dangerously")
            })
        );
    }

    fn draft(prompt: &str) -> ScheduledTaskDraft {
        ScheduledTaskDraft {
            name: "每日检查".into(),
            project_path: std::env::current_dir().unwrap().display().to_string(),
            tool_id: "codex".into(),
            model: Some("gpt-5".into()),
            prompt: prompt.into(),
            interval_minutes: 60,
            timeout_minutes: 20,
            enabled: true,
        }
    }

    #[test]
    fn saves_updates_and_reloads_without_losing_runs() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-schedule-{}-{}.json",
            std::process::id(),
            super::now_ms()
        ));
        let scheduler = Scheduler::default();
        scheduler.configure(path.clone()).unwrap();
        let saved = scheduler.save(draft("检查今天的任务")).unwrap();
        let updated = scheduler
            .update(
                &saved.id,
                ScheduledTaskDraft {
                    name: "晚间检查".into(),
                    ..draft("检查失败任务")
                },
            )
            .unwrap();
        assert_eq!(updated.name, "晚间检查");
        let reloaded = Scheduler::default();
        reloaded.configure(path.clone()).unwrap();
        assert_eq!(reloaded.list().unwrap()[0].prompt, "检查失败任务");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_blank_prompt_and_unknown_tool() {
        let scheduler = Scheduler::default();
        assert!(scheduler.save(draft("   ")).is_err());
        let mut bad = draft("有效指令");
        bad.tool_id = "shell".into();
        assert!(scheduler.save(bad).is_err());
    }
}

#[tauri::command]
pub fn list_scheduled_tasks(
    state: tauri::State<'_, Scheduler>,
) -> Result<Vec<ScheduledTask>, String> {
    state.list()
}

#[tauri::command]
pub fn save_scheduled_task(
    state: tauri::State<'_, Scheduler>,
    draft: ScheduledTaskDraft,
) -> Result<ScheduledTask, String> {
    state.save(draft)
}

#[tauri::command]
pub fn update_scheduled_task(
    state: tauri::State<'_, Scheduler>,
    id: String,
    draft: ScheduledTaskDraft,
) -> Result<ScheduledTask, String> {
    state.update(&id, draft)
}

#[tauri::command]
pub fn set_scheduled_task_enabled(
    state: tauri::State<'_, Scheduler>,
    id: String,
    enabled: bool,
) -> Result<ScheduledTask, String> {
    state.set_enabled(&id, enabled)
}

#[tauri::command]
pub fn remove_scheduled_task(state: tauri::State<'_, Scheduler>, id: String) -> Result<(), String> {
    state.remove(&id)
}

#[tauri::command]
pub fn run_scheduled_task_now(
    state: tauri::State<'_, Scheduler>,
    id: String,
) -> Result<ScheduledTask, String> {
    state.run_now(&id)
}
