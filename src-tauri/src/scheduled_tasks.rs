use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Local, NaiveTime, TimeZone};

use crate::journal;
use crate::projects::normalize_project_dir;
use crate::task_process::{self, ProcessError};
use crate::tools::{candidate_search_dirs, resolve_binary};

const STORE_BYTES: u64 = 1024 * 1024;
const TASK_LIMIT: usize = 50;
const RUN_LIMIT: usize = 10;
const OUTPUT_LIMIT: usize = 12_000;
const MIN_INTERVAL_MINUTES: u64 = 1;
const MAX_INTERVAL_MINUTES: u64 = 10_080;
pub(crate) const MIN_TIMEOUT_MINUTES: u64 = 1;
pub(crate) const MAX_TIMEOUT_MINUTES: u64 = 180;
pub(crate) const TEXT_LIMIT: usize = 2_000;
const MODEL_LIMIT: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ScheduleSpec {
    Interval { minutes: u64 },
    Daily { hour: u8, minute: u8 },
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<ScheduleSpec>,
    pub timeout_minutes: u64,
    pub enabled: bool,
    pub next_run_at_ms: u64,
    pub runs: Vec<ScheduledRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
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
    pub schedule: ScheduleSpec,
    pub timeout_minutes: u64,
    pub enabled: bool,
    #[serde(default)]
    pub source_session_id: Option<String>,
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
            task.next_run_at_ms = initial_next_run(&effective_schedule(task), now_ms())?;
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
            let schedule = effective_schedule(current);
            current.runs.insert(0, run);
            current.runs.truncate(RUN_LIMIT);
            current.schedule = Some(schedule.clone());
            current.next_run_at_ms = advance_next_run(&schedule, now_ms())?;
            current.clone()
        } else {
            return Err("定时任务已在运行期间删除".into());
        };
        persist(&inner)?;
        Ok(saved)
    }

    pub fn tick(&self) -> Result<(), String> {
        self.tick_with(execute_task)
    }

    fn tick_with(
        &self,
        execute: impl FnOnce(&ScheduledTask) -> ScheduledRun,
    ) -> Result<(), String> {
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
        let run = execute(&task);
        let mut inner = self.lock()?;
        inner.running = false;
        load_if_configured(&mut inner)?;
        if let Some(current) = inner.tasks.iter_mut().find(|item| item.id == task.id) {
            let schedule = effective_schedule(current);
            current.runs.insert(0, run);
            current.runs.truncate(RUN_LIMIT);
            current.schedule = Some(schedule.clone());
            current.next_run_at_ms = advance_next_run(&schedule, now_ms())?;
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
    let source_session_id = match draft
        .source_session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => Some(clean_token(value, "来源会话", 160)?),
        None => existing.and_then(|task| task.source_session_id.clone()),
    };
    if !matches!(draft.tool_id.as_str(), "claude" | "codex" | "agy") {
        return Err("请选择 Claude Code、Codex 或 Agy".into());
    }
    validate_schedule(&draft.schedule)?;
    if !(MIN_TIMEOUT_MINUTES..=MAX_TIMEOUT_MINUTES).contains(&draft.timeout_minutes) {
        return Err("单次超时需在 1 到 180 分钟之间".into());
    }
    let project = normalize_project_dir(&draft.project_path)?;
    let now = now_ms();
    let next = if draft.enabled {
        existing
            .filter(|task| task.enabled && effective_schedule(task) == draft.schedule)
            .map(|task| task.next_run_at_ms)
            .map(Ok)
            .unwrap_or_else(|| initial_next_run(&draft.schedule, now))?
    } else {
        existing.map(|task| task.next_run_at_ms).unwrap_or(now)
    };
    let interval_minutes = match draft.schedule {
        ScheduleSpec::Interval { minutes } => minutes,
        ScheduleSpec::Daily { .. } => 1440,
    };
    Ok(ScheduledTask {
        id: existing.map(|task| task.id.clone()).unwrap_or_default(),
        name,
        project_path: project.display().to_string(),
        tool_id: draft.tool_id,
        model,
        prompt,
        interval_minutes,
        schedule: Some(draft.schedule),
        timeout_minutes: draft.timeout_minutes,
        enabled: draft.enabled,
        next_run_at_ms: next,
        runs: existing.map(|task| task.runs.clone()).unwrap_or_default(),
        source_session_id,
    })
}

fn validate_schedule(schedule: &ScheduleSpec) -> Result<(), String> {
    match schedule {
        ScheduleSpec::Interval { minutes }
            if !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(minutes) =>
        {
            Err("运行间隔需在 1 分钟到 7 天之间".into())
        }
        ScheduleSpec::Daily { hour, minute } if *hour > 23 || *minute > 59 => {
            Err("每天执行时间无效".into())
        }
        _ => Ok(()),
    }
}

fn effective_schedule(task: &ScheduledTask) -> ScheduleSpec {
    task.schedule.clone().unwrap_or(ScheduleSpec::Interval {
        minutes: task
            .interval_minutes
            .clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES),
    })
}

fn initial_next_run(schedule: &ScheduleSpec, now: u64) -> Result<u64, String> {
    match schedule {
        ScheduleSpec::Interval { minutes } => {
            Ok(now.saturating_add(minutes.saturating_mul(60_000)))
        }
        ScheduleSpec::Daily { hour, minute } => next_daily(now, *hour, *minute),
    }
}

fn advance_next_run(schedule: &ScheduleSpec, now: u64) -> Result<u64, String> {
    match schedule {
        ScheduleSpec::Interval { minutes } => {
            Ok(now.saturating_add(minutes.saturating_mul(60_000)))
        }
        ScheduleSpec::Daily { hour, minute } => next_daily(now, *hour, *minute),
    }
}

fn next_daily(now_ms: u64, hour: u8, minute: u8) -> Result<u64, String> {
    let time = NaiveTime::from_hms_opt(u32::from(hour), u32::from(minute), 0)
        .ok_or_else(|| "每天执行时间无效".to_string())?;
    let now = Local
        .timestamp_millis_opt(now_ms as i64)
        .single()
        .ok_or_else(|| "无法读取本地时间".to_string())?;
    let mut date = now.date_naive();
    for _ in 0..3 {
        let candidate = Local
            .from_local_datetime(&date.and_time(time))
            .single()
            .ok_or_else(|| "无法计算下一次执行时间".to_string())?;
        let candidate_ms = u64::try_from(candidate.timestamp_millis())
            .map_err(|_| "无法计算下一次执行时间".to_string())?;
        if candidate_ms > now_ms {
            return Ok(candidate_ms);
        }
        date = date
            .succ_opt()
            .ok_or_else(|| "无法计算下一次执行时间".to_string())?;
    }
    Err("无法计算下一次执行时间".into())
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
            .any(|ch| !(ch.is_ascii_alphanumeric() || "._:/@+-".contains(ch)))
    {
        return Err(format!(
            "{label}只能包含字母、数字和 . _ : / @ + -，且最多 {limit} 个字符"
        ));
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
    let timeout = Duration::from_secs(task.timeout_minutes.saturating_mul(60));
    let captured =
        task_process::run_scheduler_process(program, args, cwd, timeout, OUTPUT_LIMIT * 4)
            .map_err(|error| process_error_text(error, task.timeout_minutes))?;
    Ok((
        captured.exit_code,
        trim_output(&String::from_utf8_lossy(&captured.output)),
    ))
}

fn process_error_text(error: ProcessError, timeout_minutes: u64) -> String {
    match error {
        ProcessError::Start => "无法安全启动任务 CLI".into(),
        ProcessError::Input => "无法向任务 CLI 写入输入".into(),
        ProcessError::Timeout => {
            format!("任务超时（{timeout_minutes} 分钟），已停止整个进程树")
        }
        ProcessError::OutputLimit => "任务输出超过限制".into(),
        ProcessError::OutputRead => "无法读取任务输出".into(),
        ProcessError::NonZero => "任务 CLI 返回失败状态".into(),
        ProcessError::Descendants => "任务退出后仍有子进程，已停止整个进程树".into(),
        ProcessError::StopUnconfirmed => "无法确认任务进程树已经停止".into(),
        ProcessError::StopRequested => "任务已被停止".into(),
        ProcessError::StopRequestUnconfirmed => "无法确认任务进程树已经停止".into(),
        ProcessError::StopRequestUnrecorded => "任务停止状态未能记录".into(),
        ProcessError::StartIdentityUnrecorded => "任务启动身份无法记录".into(),
    }
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
    let mut args = Vec::new();
    #[cfg(windows)]
    if !binary
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err("Windows 定时任务只允许直接启动 .exe CLI".into());
    }
    let program = binary.to_path_buf();
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
    let existing_len = if journal::regular_or_missing(path).map_err(storage_error)? {
        path.metadata().map_err(storage_error)?.len()
    } else {
        0
    };
    let (revision, _) = read_existing(path)?;
    let next_revision = revision
        .checked_add(1)
        .ok_or_else(|| "定时任务版本号已耗尽".to_string())?;
    let mut stored_tasks = tasks.to_vec();
    let line = loop {
        let record = StoreRecord {
            schema: 1,
            revision: next_revision,
            sha256: hash(next_revision, &stored_tasks)?,
            tasks: stored_tasks.clone(),
        };
        let mut line = serde_json::to_vec(&record).map_err(|_| "定时任务无法保存".to_string())?;
        line.push(b'\n');
        if line.len() as u64 <= STORE_BYTES {
            break line;
        }
        let mut to_remove = (line.len() as u64 - STORE_BYTES)
            .div_ceil((OUTPUT_LIMIT + 256) as u64)
            .max(1) as usize;
        while to_remove > 0 {
            let index = stored_tasks
                .iter()
                .enumerate()
                .filter(|(_, task)| !task.runs.is_empty())
                .max_by_key(|(_, task)| task.runs.len())
                .map(|(index, _)| index)
                .ok_or_else(|| "定时任务有效内容超过 1 MiB，请减少任务数量".to_string())?;
            stored_tasks[index].runs.pop();
            to_remove -= 1;
        }
    };
    if existing_len.saturating_add(line.len() as u64) > STORE_BYTES {
        journal::replace_and_archive(path, &line).map_err(storage_error)?;
        Ok(())
    } else {
        journal::append(path, &line, existing_len == 0).map_err(storage_error)
    }
}

fn read_locked(path: &Path) -> Result<(u64, Vec<ScheduledTask>), String> {
    let _lock = lock_store(path)?;
    read_existing(path)
}

fn read_existing(path: &Path) -> Result<(u64, Vec<ScheduledTask>), String> {
    if !path.exists() {
        return Ok((0, Vec::new()));
    }
    if !journal::regular_or_missing(path).map_err(storage_error)? {
        return Ok((0, Vec::new()));
    }
    let bytes = journal::read_bytes(path, journal::HARD_READ_LIMIT).map_err(storage_error)?;
    if bytes.is_empty() {
        return Ok((0, Vec::new()));
    }
    if !bytes.ends_with(b"\n") {
        return Err("定时任务存储校验失败".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "定时任务存储不是有效文本".to_string())?;
    let mut revision = None;
    let mut tasks = Vec::new();
    for line in text.split_terminator('\n') {
        let record: StoreRecord =
            serde_json::from_str(line).map_err(|_| "定时任务存储校验失败".to_string())?;
        if record.schema != 1
            || !journal::revision_follows(revision, record.revision)
            || record.sha256 != hash(record.revision, &record.tasks)?
        {
            return Err("定时任务存储校验失败".into());
        }
        validate_tasks(&record.tasks)?;
        revision = Some(record.revision);
        tasks = record.tasks;
    }
    Ok((revision.unwrap_or_default(), tasks))
}

fn lock_store(path: &Path) -> Result<journal::JournalLock, String> {
    journal::lock(&journal::companion(path, ".lock"), true)
        .map_err(|_| "定时任务正被另一操作使用，请重试".to_string())
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
        if let Some(schedule) = task.schedule.as_ref() {
            validate_schedule(schedule)?;
        }
        if !seen.insert(&task.id)
            || task.runs.len() > RUN_LIMIT
            || (task.schedule.is_none()
                && !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(&task.interval_minutes))
        {
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
    use super::{ScheduleSpec, ScheduledTaskDraft, Scheduler};
    use chrono::{TimeZone, Timelike};
    use std::fs;
    use std::path::Path;

    #[test]
    fn scheduled_commands_leave_permissions_to_each_agent() {
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        let claude_name = format!("claude{suffix}");
        let codex_name = format!("codex{suffix}");
        let agy_name = format!("agy{suffix}");
        let claude = super::command_for_tool(Path::new(&claude_name), "claude", None)
            .unwrap()
            .1;
        assert!(claude.iter().any(|arg| arg == "--print"));
        assert!(!claude.iter().any(|arg| {
            arg.contains("permission") || arg.contains("sandbox") || arg.contains("dangerously")
        }));

        let codex = super::command_for_tool(Path::new(&codex_name), "codex", None)
            .unwrap()
            .1;
        assert!(codex.iter().any(|arg| arg == "exec"));
        assert!(!codex.iter().any(|arg| {
            arg.contains("sandbox") || arg.contains("skip-git") || arg.contains("dangerously")
        }));

        let agy = super::command_for_tool(Path::new(&agy_name), "agy", None)
            .unwrap()
            .1;
        assert!(agy.iter().any(|arg| arg == "--print"));
        assert!(
            !agy.iter().any(|arg| {
                arg == "--mode" || arg == "--sandbox" || arg.contains("dangerously")
            })
        );
    }

    #[test]
    fn daily_schedule_targets_the_next_local_clock_time() {
        let next = super::next_daily(super::now_ms(), 9, 30).unwrap();
        let local = chrono::Local
            .timestamp_millis_opt(next as i64)
            .single()
            .unwrap();
        assert_eq!(local.hour(), 9);
        assert_eq!(local.minute(), 30);
        assert!(next > super::now_ms());
    }

    fn draft(prompt: &str) -> ScheduledTaskDraft {
        ScheduledTaskDraft {
            name: "每日检查".into(),
            project_path: std::env::current_dir().unwrap().display().to_string(),
            tool_id: "codex".into(),
            model: Some("gpt-5".into()),
            prompt: prompt.into(),
            schedule: ScheduleSpec::Interval { minutes: 60 },
            timeout_minutes: 20,
            enabled: true,
            source_session_id: None,
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

    #[test]
    fn model_rejects_command_metacharacters() {
        for model in ["x&calc", "x|calc", "x>out", "x<in", "x^calc", "x\"calc"] {
            let mut bad = draft("有效指令");
            bad.model = Some(model.into());
            let error = Scheduler::default().save(bad).unwrap_err();
            assert!(error.contains("模型名称"), "{model}: {error}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn scheduled_command_rejects_cmd_and_bat_wrappers() {
        for binary in ["codex.cmd", "codex.bat"] {
            let error = super::command_for_tool(Path::new(binary), "codex", None).unwrap_err();
            assert!(error.contains(".exe"), "{binary}: {error}");
        }
    }

    #[test]
    fn concurrent_manual_runs_are_rejected() {
        let scheduler = Scheduler::default();
        scheduler.lock().unwrap().running = true;
        let error = scheduler.run_now("missing").unwrap_err();
        assert!(error.contains("已有定时任务正在运行"), "{error}");
    }

    #[test]
    fn tick_runs_one_due_task_and_advances_its_next_schedule() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-schedule-tick-{}-{}.json",
            std::process::id(),
            super::now_ms()
        ));
        let mut task = super::normalize_draft(draft("有效指令"), None).unwrap();
        task.id = "task-due".into();
        task.next_run_at_ms = 1;
        super::write_store(&path, std::slice::from_ref(&task)).unwrap();
        let scheduler = Scheduler::default();
        scheduler.configure(path).unwrap();
        let mut calls = 0;
        scheduler
            .tick_with(|selected| {
                calls += 1;
                assert_eq!(selected.id, "task-due");
                super::ScheduledRun {
                    started_at_ms: 10,
                    finished_at_ms: 11,
                    status: "成功".into(),
                    exit_code: Some(0),
                    output: "ok".into(),
                }
            })
            .unwrap();
        scheduler
            .tick_with(|_| panic!("advanced task must not run twice"))
            .unwrap();
        let saved = scheduler.list().unwrap().remove(0);
        assert_eq!(calls, 1);
        assert_eq!(saved.runs.len(), 1);
        assert!(saved.next_run_at_ms > super::now_ms());
    }

    #[test]
    fn append_compaction_keeps_scheduled_storage_usable() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-schedule-compact-{}-{}.json",
            std::process::id(),
            super::now_ms()
        ));
        let mut task = super::normalize_draft(draft("有效指令"), None).unwrap();
        task.id = "task-compact".into();
        task.runs = (0..super::RUN_LIMIT)
            .map(|index| super::ScheduledRun {
                started_at_ms: index as u64 + 1,
                finished_at_ms: index as u64 + 2,
                status: "成功".into(),
                exit_code: Some(0),
                output: "x".repeat(super::OUTPUT_LIMIT),
            })
            .collect();
        for _ in 0..20 {
            super::write_store(&path, std::slice::from_ref(&task)).unwrap();
        }
        assert!(std::fs::metadata(&path).unwrap().len() <= super::STORE_BYTES);
        let loaded = super::read_store(&path).unwrap();
        assert_eq!(loaded[0].id, "task-compact");
        assert!(
            std::fs::read_dir(std::env::temp_dir())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("agentterm-schedule-compact-")
                    && entry.file_name().to_string_lossy().contains(".archive-"))
        );
    }

    #[test]
    fn compaction_reduces_oldest_run_outputs_to_the_effective_snapshot_limit() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-schedule-effective-limit-{}-{}.json",
            std::process::id(),
            super::now_ms()
        ));
        let tasks = (0..super::TASK_LIMIT)
            .map(|task_index| {
                let mut task = super::normalize_draft(draft("有效指令"), None).unwrap();
                task.id = format!("task-{task_index}");
                task.runs = (0..super::RUN_LIMIT)
                    .map(|run_index| super::ScheduledRun {
                        started_at_ms: run_index as u64 + 1,
                        finished_at_ms: run_index as u64 + 2,
                        status: "成功".into(),
                        exit_code: Some(0),
                        output: "x".repeat(super::OUTPUT_LIMIT),
                    })
                    .collect();
                task
            })
            .collect::<Vec<_>>();
        super::write_store(&path, &tasks).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() <= super::STORE_BYTES);
        let loaded = super::read_store(&path).unwrap();
        assert_eq!(loaded.len(), super::TASK_LIMIT);
        assert!(
            loaded.iter().map(|task| task.runs.len()).sum::<usize>()
                < super::TASK_LIMIT * super::RUN_LIMIT
        );
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
pub async fn run_scheduled_task_now(
    state: tauri::State<'_, Scheduler>,
    id: String,
) -> Result<ScheduledTask, String> {
    let scheduler = state.handle();
    tauri::async_runtime::spawn_blocking(move || scheduler.run_now(&id))
        .await
        .map_err(|error| format!("定时任务执行线程异常：{error}"))?
}
