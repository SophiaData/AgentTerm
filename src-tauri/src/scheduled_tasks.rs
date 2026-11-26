use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approval_pending: bool,
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
    pub approval_pending: bool,
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
    /// Empty when idle; otherwise the task id currently executing.
    running_task: String,
    cancel: Arc<AtomicBool>,
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

    #[cfg(test)]
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
            task.approval_pending = false;
            task.next_run_at_ms = next_run(&effective_schedule(task), now_ms())?;
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
            // "Agent 创建的任务必须由用户确认" is a product invariant, so the
            // backend enforces it too: a disabled-by-default draft can never be
            // run through IPC just because a caller skipped the panel button.
            if task.approval_pending {
                return Err("任务尚未确认启用，请先在面板中确认".into());
            }
            inner.running = true;
            inner.running_task = task.id.clone();
            inner.cancel = Arc::new(AtomicBool::new(false));
            task
        };
        // Execute on a worker thread so the panel stays responsive and the run
        // can be cancelled. A manual run never advances the schedule, so
        // pressing "run now" shortly before the next slot does not swallow it.
        let scheduler = self.handle();
        let scheduled = task.clone();
        let spawn = std::thread::Builder::new()
            .name("agentterm-scheduled-run".into())
            .spawn(move || {
                let cancel = scheduler.cancel_flag();
                let run = execute_task_with_cancel(&scheduled, &cancel);
                scheduler.complete_run(&scheduled.id, run, false);
            });
        if let Err(error) = spawn {
            self.complete_run(
                &task.id,
                failed_run(&format!("无法启动定时任务：{error}")),
                false,
            );
            return Err(format!("无法启动定时任务：{error}"));
        }
        Ok(task)
    }

    pub fn tick(&self) -> Result<(), String> {
        self.tick_with(execute_task_with_cancel)
    }

    fn tick_with(
        &self,
        execute: impl FnOnce(&ScheduledTask, &AtomicBool) -> ScheduledRun,
    ) -> Result<(), String> {
        let due = {
            let mut inner = self.lock()?;
            load_if_configured(&mut inner)?;
            if inner.running {
                return Ok(());
            }
            let now = now_ms();
            let task_id = inner
                .tasks
                .iter()
                .filter(|task| task.enabled && !task.approval_pending && task.next_run_at_ms <= now)
                .min_by_key(|task| task.next_run_at_ms)
                .map(|task| task.id.clone());
            let Some(task_id) = task_id else {
                return Ok(());
            };
            let task = inner
                .tasks
                .iter_mut()
                .find(|task| task.id == task_id)
                .ok_or_else(|| "定时任务不存在".to_string())?;
            let schedule = effective_schedule(task);
            // Advance durably before starting a process with external side
            // effects. If the disk is unavailable, the task is not executed.
            // A crash after this write can skip one occurrence, but can never
            // execute the same occurrence every 30 seconds.
            task.schedule = Some(schedule.clone());
            task.next_run_at_ms = next_run(&schedule, now)?;
            let task_to_run = task.clone();
            persist(&inner)?;
            inner.running = true;
            inner.running_task = task_to_run.id.clone();
            inner.cancel = Arc::new(AtomicBool::new(false));
            task_to_run
        };
        let task = due;
        let cancel = self.cancel_flag();
        // A scheduled run has already advanced its slot, so a cancelled or
        // failed run is recorded but not retried in place.
        let run = execute(&task, &cancel);
        self.complete_run(&task.id, run, true);
        Ok(())
    }

    fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.state
            .lock()
            .map(|inner| Arc::clone(&inner.cancel))
            .unwrap_or_else(|_| Arc::new(AtomicBool::new(false)))
    }

    /// Persist the finished run and clear the running marker, even when the
    /// underlying call failed. Never leaves the scheduler stuck as "running".
    fn complete_run(&self, task_id: &str, run: ScheduledRun, normalize_schedule: bool) {
        let Ok(mut inner) = self.state.lock() else {
            return;
        };
        inner.running = false;
        inner.running_task.clear();
        if let Err(error) = load_if_configured(&mut inner) {
            eprintln!("AgentTerm could not reload scheduled tasks after a run: {error}");
            return;
        }
        let Some(current) = inner.tasks.iter_mut().find(|item| item.id == task_id) else {
            return;
        };
        if normalize_schedule {
            current.schedule = Some(effective_schedule(current));
        }
        current.runs.insert(0, run);
        current.runs.truncate(RUN_LIMIT);
        if let Err(error) = persist(&inner) {
            eprintln!("AgentTerm could not persist a scheduled run result: {error}");
        }
    }

    /// Id of the task currently executing, or `None` when idle.
    pub fn running_task(&self) -> Result<core::option::Option<String>, String> {
        let mut inner = self.lock()?;
        load_if_configured(&mut inner)?;
        if inner.running && !inner.running_task.is_empty() {
            Ok(Some(inner.running_task.clone()))
        } else {
            Ok(None)
        }
    }

    /// Ask the specified running task to stop its process tree.
    /// A stale request must not stop another task that has since started.
    pub fn cancel_run(&self, task_id: &str) -> Result<bool, String> {
        let inner = self.lock()?;
        if !inner.running || inner.running_task != task_id {
            return Ok(false);
        }
        inner.cancel.store(true, Ordering::SeqCst);
        Ok(true)
    }

    /// Cancel any in-flight run and wait for its result to be persisted, so a
    /// graceful exit never leaves a run unrecorded or a child process orphaned.
    pub fn shutdown(&self, timeout: Duration) -> Result<(), String> {
        {
            let inner = self.lock()?;
            if !inner.running {
                return Ok(());
            }
            inner.cancel.store(true, Ordering::SeqCst);
        }
        let deadline = Instant::now() + timeout;
        loop {
            {
                let inner = self.lock()?;
                if !inner.running {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err("定时任务仍在运行，请稍后重试退出".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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
    if prompt.starts_with('-') {
        return Err("任务指令不能以命令选项符号 - 开头".into());
    }
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
    // Keep an existing slot only when the schedule itself is unchanged, so a
    // paused task still reports the time it will run at once enabled instead of
    // "now". A changed schedule always gets a freshly computed slot.
    let next = existing
        .filter(|task| effective_schedule(task) == draft.schedule)
        .map(|task| task.next_run_at_ms)
        .map(Ok)
        .unwrap_or_else(|| next_run(&draft.schedule, now))?;
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
        approval_pending: draft.approval_pending,
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

fn next_run(schedule: &ScheduleSpec, now: u64) -> Result<u64, String> {
    match schedule {
        ScheduleSpec::Interval { minutes } => {
            Ok(now.saturating_add(minutes.saturating_mul(60_000)))
        }
        ScheduleSpec::Daily { hour, minute } => next_daily(now, *hour, *minute),
    }
}

fn next_daily(now_ms: u64, hour: u8, minute: u8) -> Result<u64, String> {
    next_daily_in(Local, now_ms, hour, minute)
}

fn next_daily_in<Tz: TimeZone>(
    timezone: Tz,
    now_ms: u64,
    hour: u8,
    minute: u8,
) -> Result<u64, String> {
    let time = NaiveTime::from_hms_opt(u32::from(hour), u32::from(minute), 0)
        .ok_or_else(|| "每天执行时间无效".to_string())?;
    let now = timezone
        .timestamp_millis_opt(now_ms as i64)
        .earliest()
        .ok_or_else(|| "无法读取本地时间".to_string())?;
    let mut date = now.date_naive();
    for _ in 0..370 {
        // Choose the first occurrence before comparing with now, so a clock
        // that repeats during fall-back cannot schedule the same day twice.
        let candidate = timezone
            .from_local_datetime(&date.and_time(time))
            .earliest()
            .and_then(|value| u64::try_from(value.timestamp_millis()).ok());
        if let Some(candidate_ms) = candidate.filter(|value| *value > now_ms) {
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

fn execute_task_with_cancel(task: &ScheduledTask, cancel: &AtomicBool) -> ScheduledRun {
    let started = now_ms();
    let result = run_process(task, cancel);
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

fn failed_run(message: &str) -> ScheduledRun {
    let now = now_ms();
    ScheduledRun {
        started_at_ms: now,
        finished_at_ms: now,
        status: "失败".into(),
        exit_code: None,
        output: message.to_string(),
    }
}

fn run_process(task: &ScheduledTask, cancel: &AtomicBool) -> Result<(i32, String), String> {
    let binary = resolve_tool(&task.tool_id)?;
    let cwd = normalize_project_dir(&task.project_path)?;
    let (program, mut args) = command_for_tool(&binary, &task.tool_id, task.model.as_deref())?;
    args.push(task.prompt.clone());
    let timeout = Duration::from_secs(task.timeout_minutes.saturating_mul(60));
    let cancelled = || cancel.load(Ordering::SeqCst);
    let captured = task_process::run_scheduler_process_with_cancel(
        program,
        args,
        cwd,
        timeout,
        OUTPUT_LIMIT * 4,
        &cancelled,
    )
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
    let text = std::str::from_utf8(&bytes).map_err(|_| "定时任务存储不是有效文本".to_string())?;
    // A crash mid-append can leave a torn final line (or no trailing newline).
    // Only the tail may be dropped; anything damaged in the middle still fails
    // closed, and the original file is archived before any repair is written.
    let complete = bytes.ends_with(b"\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let line_count = if complete {
        lines.len().saturating_sub(1)
    } else {
        lines.len()
    };
    let mut revision = None;
    let mut tasks = Vec::new();
    let mut torn_tail = !complete;
    for (index, line) in lines.iter().take(line_count).enumerate() {
        let is_last = index + 1 == line_count;
        // Only an unparseable final line counts as a torn write. A line that
        // parses but fails schema/revision/digest is tampering or corruption,
        // and must fail closed instead of silently rolling back to the previous
        // revision (which could resurrect a deleted task).
        let record: StoreRecord = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(_) => {
                if is_last {
                    torn_tail = true;
                    break;
                }
                return Err("定时任务存储校验失败".into());
            }
        };
        if record.schema != 1
            || !journal::revision_follows(revision, record.revision)
            || !digest_matches(&record)?
        {
            return Err("定时任务存储校验失败".into());
        }
        validate_tasks(&record.tasks)?;
        revision = Some(record.revision);
        tasks = record.tasks;
    }
    if torn_tail && let Err(error) = repair_store(path, revision, &tasks) {
        eprintln!("AgentTerm could not repair the scheduled task journal: {error}");
    }
    Ok((revision.unwrap_or_default(), tasks))
}

/// Rewrite the journal as a single canonical record after dropping a torn tail.
/// The caller already holds the store lock; the damaged file is archived by
/// `replace_and_archive` instead of being deleted.
fn repair_store(path: &Path, revision: Option<u64>, tasks: &[ScheduledTask]) -> Result<(), String> {
    let revision = revision.unwrap_or_default().max(1);
    let record = StoreRecord {
        schema: 1,
        revision,
        sha256: hash(revision, tasks)?,
        tasks: tasks.to_vec(),
    };
    let mut line = serde_json::to_vec(&record).map_err(|_| "定时任务无法保存".to_string())?;
    line.push(b'\n');
    journal::replace_and_archive(path, &line).map_err(storage_error)?;
    Ok(())
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

// 0.2.0-rc.8 always serialized approval_pending, so records it wrote hash
// differently from the current canonical form. Accept both without rewriting
// history: new writes stay canonical, old lines keep validating.
fn digest_matches(record: &StoreRecord) -> Result<bool, String> {
    if record.sha256 == hash(record.revision, &record.tasks)? {
        return Ok(true);
    }
    Ok(record.sha256 == hash_with_approval_field(record.revision, &record.tasks)?)
}

fn hash_with_approval_field(revision: u64, tasks: &[ScheduledTask]) -> Result<String, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ApprovalFieldTask<'a> {
        id: &'a str,
        name: &'a str,
        project_path: &'a str,
        tool_id: &'a str,
        model: Option<&'a str>,
        prompt: &'a str,
        interval_minutes: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        schedule: Option<&'a ScheduleSpec>,
        timeout_minutes: u64,
        enabled: bool,
        approval_pending: bool,
        next_run_at_ms: u64,
        runs: &'a [ScheduledRun],
        #[serde(skip_serializing_if = "Option::is_none")]
        source_session_id: Option<&'a str>,
    }

    let stored: Vec<ApprovalFieldTask> = tasks
        .iter()
        .map(|task| ApprovalFieldTask {
            id: &task.id,
            name: &task.name,
            project_path: &task.project_path,
            tool_id: &task.tool_id,
            model: task.model.as_deref(),
            prompt: &task.prompt,
            interval_minutes: task.interval_minutes,
            schedule: task.schedule.as_ref(),
            timeout_minutes: task.timeout_minutes,
            enabled: task.enabled,
            approval_pending: task.approval_pending,
            next_run_at_ms: task.next_run_at_ms,
            runs: &task.runs,
            source_session_id: task.source_session_id.as_deref(),
        })
        .collect();
    let bytes =
        serde_json::to_vec(&(revision, &stored)).map_err(|_| "定时任务无法校验".to_string())?;
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
    use serde::Serialize;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::io::Write;
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

    #[test]
    fn daily_schedule_skips_a_nonexistent_dst_clock_time() {
        use chrono::TimeZone;
        let timezone = chrono_tz::America::New_York;
        let now = timezone
            .with_ymd_and_hms(2025, 3, 8, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        let next = super::next_daily_in(timezone, now, 2, 30).unwrap();
        let next = timezone.timestamp_millis_opt(next as i64).single().unwrap();
        assert_eq!(next.date_naive().to_string(), "2025-03-10");
        assert_eq!(next.format("%H:%M").to_string(), "02:30");
    }

    #[test]
    fn daily_schedule_runs_only_once_when_the_local_clock_repeats() {
        let timezone = chrono_tz::America::New_York;
        let repeated = timezone.with_ymd_and_hms(2025, 11, 2, 1, 30, 0);
        let first = repeated.earliest().unwrap().timestamp_millis() as u64;
        let second = repeated.latest().unwrap().timestamp_millis() as u64;

        assert_eq!(super::next_daily_in(timezone, first - 1, 1, 30), Ok(first));
        let next_day = timezone
            .with_ymd_and_hms(2025, 11, 3, 1, 30, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(super::next_daily_in(timezone, first, 1, 30), Ok(next_day));
        assert_eq!(super::next_daily_in(timezone, second, 1, 30), Ok(next_day));
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
            approval_pending: false,
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

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacyTask {
        id: String,
        name: String,
        project_path: String,
        tool_id: String,
        model: Option<String>,
        prompt: String,
        interval_minutes: u64,
        timeout_minutes: u64,
        enabled: bool,
        next_run_at_ms: u64,
        runs: Vec<serde_json::Value>,
    }

    #[test]
    fn legacy_record_without_approval_pending_remains_readable() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-legacy-schedule-{}-{}.json",
            std::process::id(),
            super::now_ms()
        ));
        let legacy_task = LegacyTask {
            id: "legacy-task".into(),
            name: "旧任务".into(),
            project_path: "E:\\AgentTerm".into(),
            tool_id: "claude".into(),
            model: None,
            prompt: "检查项目状态".into(),
            interval_minutes: 60,
            timeout_minutes: 20,
            enabled: true,
            next_run_at_ms: 1,
            runs: Vec::new(),
        };
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(1_u64, vec![legacy_task.clone()])).unwrap())
        );
        let task_json = serde_json::to_string(&legacy_task).unwrap();
        let record =
            format!(r#"{{"schema":1,"revision":1,"tasks":[{task_json}],"sha256":"{digest}"}}"#);
        fs::write(&path, format!("{record}\n")).unwrap();

        let scheduler = Scheduler::default();
        scheduler.configure(path.clone()).unwrap();
        let tasks = scheduler.list().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "legacy-task");
        assert!(!tasks[0].approval_pending);
        let _ = fs::remove_file(path);
    }

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ApprovalFieldTask {
        id: String,
        name: String,
        project_path: String,
        tool_id: String,
        model: Option<String>,
        prompt: String,
        interval_minutes: u64,
        timeout_minutes: u64,
        enabled: bool,
        approval_pending: bool,
        next_run_at_ms: u64,
        runs: Vec<serde_json::Value>,
    }

    #[test]
    fn rc8_record_with_serialized_approval_field_remains_readable() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-rc8-schedule-{}-{}.json",
            std::process::id(),
            super::now_ms()
        ));
        let task = ApprovalFieldTask {
            id: "rc8-task".into(),
            name: "rc8 任务".into(),
            project_path: "E:\\AgentTerm".into(),
            tool_id: "claude".into(),
            model: None,
            prompt: "检查项目状态".into(),
            interval_minutes: 60,
            timeout_minutes: 20,
            enabled: false,
            approval_pending: false,
            next_run_at_ms: 1,
            runs: Vec::new(),
        };
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(1_u64, vec![task.clone()])).unwrap())
        );
        let task_json = serde_json::to_string(&task).unwrap();
        let record =
            format!(r#"{{"schema":1,"revision":1,"tasks":[{task_json}],"sha256":"{digest}"}}"#);
        fs::write(&path, format!("{record}\n")).unwrap();

        let scheduler = Scheduler::default();
        scheduler.configure(path.clone()).unwrap();
        let tasks = scheduler.list().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "rc8-task");

        let updated = scheduler
            .update(
                "rc8-task",
                ScheduledTaskDraft {
                    enabled: true,
                    ..draft("检查项目状态")
                },
            )
            .unwrap();
        assert!(updated.enabled);
        let reloaded = Scheduler::default();
        reloaded.configure(path.clone()).unwrap();
        assert!(reloaded.list().unwrap()[0].enabled);
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
    fn a_pending_task_cannot_be_run_through_ipc() {
        let mut task = super::normalize_draft(draft("有效指令"), None).unwrap();
        task.id = "task-pending-run".into();
        task.enabled = false;
        task.approval_pending = true;
        let scheduler = Scheduler::default();
        scheduler.lock().unwrap().tasks.push(task);

        let error = scheduler.run_now("task-pending-run").unwrap_err();
        assert!(error.contains("尚未确认启用"), "{error}");
        // The refusal must not leave the scheduler stuck as running.
        assert!(!scheduler.lock().unwrap().running);
        assert!(scheduler.running_task().unwrap().is_none());
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
        scheduler.configure(path.clone()).unwrap();
        let mut calls = 0;
        scheduler
            .tick_with(|selected, _cancel| {
                let persisted = super::read_store(&path).unwrap();
                assert!(persisted[0].next_run_at_ms > super::now_ms());
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
            .tick_with(|_, _| panic!("advanced task must not run twice"))
            .unwrap();
        let saved = scheduler.list().unwrap().remove(0);
        assert_eq!(calls, 1);
        assert_eq!(saved.runs.len(), 1);
        assert!(saved.next_run_at_ms > super::now_ms());
    }

    #[test]
    fn agent_created_task_waits_for_explicit_enable() {
        let mut task = super::normalize_draft(draft("有效指令"), None).unwrap();
        task.id = "task-pending".into();
        task.enabled = false;
        task.approval_pending = true;
        task.next_run_at_ms = 1;
        let scheduler = Scheduler::default();
        scheduler.lock().unwrap().tasks.push(task.clone());
        scheduler
            .tick_with(|_, _| panic!("pending approval must not execute"))
            .unwrap();
        let enabled = scheduler.set_enabled(&task.id, true).unwrap();
        assert!(enabled.enabled);
        assert!(!enabled.approval_pending);
    }

    #[test]
    fn prompt_cannot_be_parsed_as_a_cli_option() {
        let error = Scheduler::default()
            .save(draft("--dangerously-skip-permissions"))
            .unwrap_err();
        assert!(error.contains("不能以命令选项符号"), "{error}");
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

    #[test]
    fn a_torn_tail_is_dropped_and_the_store_repairs_itself() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-schedule-torn-{}-{}.json",
            std::process::id(),
            super::now_ms()
        ));
        let mut task = super::normalize_draft(draft("有效指令"), None).unwrap();
        task.id = "task-torn".into();
        super::write_store(&path, std::slice::from_ref(&task)).unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(br#"{"schema":1,"revision":2,"tasks":[{"id":"#)
            .unwrap();
        file.sync_all().unwrap();
        drop(file);

        let scheduler = Scheduler::default();
        scheduler.configure(path.clone()).unwrap();
        let tasks = scheduler.list().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "task-torn");

        let repaired = fs::read(&path).unwrap();
        assert!(repaired.ends_with(b"\n"));
        assert_eq!(std::str::from_utf8(&repaired).unwrap().lines().count(), 1);
        assert_eq!(super::read_store(&path).unwrap()[0].id, "task-torn");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_valid_but_unverifiable_last_record_still_fails_closed() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-schedule-tamper-{}-{}.json",
            std::process::id(),
            super::now_ms()
        ));
        let mut task = super::normalize_draft(draft("有效指令"), None).unwrap();
        task.id = "task-keep".into();
        super::write_store(&path, std::slice::from_ref(&task)).unwrap();
        // A complete, parseable record whose digest does not match is tampering,
        // not a torn write: rolling back would resurrect earlier state.
        let tampered = concat!(
            r#"{"schema":1,"revision":2,"tasks":[],"sha256":""#,
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(tampered.as_bytes()).unwrap();
        file.write_all(b"\"}\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let before = fs::read(&path).unwrap();

        let scheduler = Scheduler::default();
        scheduler.configure(path.clone()).unwrap();
        assert!(scheduler.list().is_err());
        // The original bytes are untouched, so nothing was silently dropped.
        assert_eq!(fs::read(&path).unwrap(), before);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn cancelling_reports_and_clears_the_running_task() {
        let scheduler = Scheduler::default();
        {
            let mut inner = scheduler.lock().unwrap();
            inner.running = true;
            inner.running_task = "task-running".into();
        }
        assert_eq!(
            scheduler.running_task().unwrap().as_deref(),
            Some("task-running")
        );
        assert!(!scheduler.cancel_run("task-obsolete").unwrap());
        assert!(
            !scheduler
                .cancel_flag()
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(scheduler.cancel_run("task-running").unwrap());
        assert!(
            scheduler
                .cancel_flag()
                .load(std::sync::atomic::Ordering::SeqCst)
        );

        scheduler.complete_run(
            "task-running",
            super::ScheduledRun {
                started_at_ms: 1,
                finished_at_ms: 2,
                status: "任务已被停止".into(),
                exit_code: None,
                output: String::new(),
            },
            false,
        );
        assert!(scheduler.running_task().unwrap().is_none());
        assert!(!scheduler.cancel_run("task-running").unwrap());
    }

    #[test]
    fn shutdown_returns_immediately_when_idle_and_times_out_while_busy() {
        let scheduler = Scheduler::default();
        assert!(
            scheduler
                .shutdown(std::time::Duration::from_millis(20))
                .is_ok()
        );
        scheduler.lock().unwrap().running = true;
        assert!(
            scheduler
                .shutdown(std::time::Duration::from_millis(30))
                .is_err()
        );
    }

    #[test]
    fn a_paused_draft_reports_the_slot_it_would_run_at() {
        let scheduler = Scheduler::default();
        let mut draft = draft("有效指令");
        draft.enabled = false;
        let saved = scheduler
            .save(ScheduledTaskDraft {
                name: draft.name.clone(),
                project_path: draft.project_path.clone(),
                tool_id: draft.tool_id.clone(),
                model: draft.model.clone(),
                prompt: draft.prompt.clone(),
                schedule: draft.schedule.clone(),
                timeout_minutes: draft.timeout_minutes,
                enabled: false,
                approval_pending: false,
                source_session_id: None,
            })
            .unwrap();
        assert!(!saved.enabled);
        assert!(saved.next_run_at_ms > super::now_ms());

        let updated = scheduler
            .update(
                &saved.id,
                ScheduledTaskDraft {
                    enabled: true,
                    ..draft
                },
            )
            .unwrap();
        assert_eq!(updated.next_run_at_ms, saved.next_run_at_ms);
    }
}

#[tauri::command(async)]
pub fn list_scheduled_tasks(
    state: tauri::State<'_, Scheduler>,
) -> Result<Vec<ScheduledTask>, String> {
    state.list()
}

#[tauri::command(async)]
pub fn set_scheduled_task_enabled(
    state: tauri::State<'_, Scheduler>,
    id: String,
    enabled: bool,
) -> Result<ScheduledTask, String> {
    state.set_enabled(&id, enabled)
}

#[tauri::command(async)]
pub fn remove_scheduled_task(state: tauri::State<'_, Scheduler>, id: String) -> Result<(), String> {
    state.remove(&id)
}

/// Starts the run and returns immediately; the panel polls for progress and
/// offers "停止运行" while the task is executing.
#[tauri::command(async)]
pub fn run_scheduled_task_now(
    state: tauri::State<'_, Scheduler>,
    id: String,
) -> Result<ScheduledTask, String> {
    state.run_now(&id)
}

#[tauri::command(async)]
pub fn scheduled_task_running(
    state: tauri::State<'_, Scheduler>,
) -> Result<core::option::Option<String>, String> {
    state.running_task()
}

#[tauri::command(async)]
pub fn cancel_scheduled_task(
    state: tauri::State<'_, Scheduler>,
    id: String,
) -> Result<bool, String> {
    state.cancel_run(&id)
}
