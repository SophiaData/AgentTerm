//! Bounded task metadata journal. No directories, project writes or raw CLI logs.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

const HEADER: &str = "AGENTTERM_TASK_DOCUMENT\t1\n";
const JOURNAL_LIMIT: u64 = 8 * 1024 * 1024;
const FILE_LIMIT: u64 = 1024 * 1024;
const RECORD_LIMIT: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub assignee: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskPlan {
    pub task_id: String,
    pub generation: u64,
    pub project_id: String,
    pub goal: String,
    pub owner: String,
    pub scope: String,
    pub output_dir: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AttemptState {
    Running,
    Reported,
    Blocked,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub executable: String,
    /// OS start token where the platform exposes one (for example Linux
    /// `/proc/<pid>/stat` starttime). PID alone is never a stop proof.
    #[serde(default)]
    pub start_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StopEvidence {
    pub requested: bool,
    pub confirmed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptEvent {
    pub id: String,
    pub sequence: u64,
    pub attempt_id: String,
    pub step_id: String,
    pub state: AttemptState,
    pub summary: String,
    /// Added after the process is actually created. Missing on old documents
    /// means the launch boundary was never durably observed.
    #[serde(default)]
    pub process_identity: Option<ProcessIdentity>,
    /// Stop evidence is attached to the running start event when a stop was
    /// requested, and to the terminal event when the outcome is known.
    #[serde(default)]
    pub stop: Option<StopEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub relative_path: String,
    pub sha256: String,
    pub bytes: u64,
    pub version: u64,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionLink {
    pub id: String,
    pub tool_id: String,
    pub tool_name: String,
    pub project_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Document {
    pub revision: u64,
    pub plan: TaskPlan,
    pub events: Vec<AttemptEvent>,
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub session_link: Option<SessionLink>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    document: Document,
    sha256: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPreview {
    pub id: String,
    pub sha256: String,
    pub content: String,
}

#[derive(Clone)]
pub struct DocumentStore {
    path: PathBuf,
    guard: PathBuf,
}

struct DocumentGuard(File);
impl Drop for DocumentGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn io_error(_: impl std::fmt::Display) -> String {
    "任务文档读写失败；保留占用，请重新核实".into()
}
fn invalid() -> String {
    "任务文档版本未知、记录损坏或不符合契约；请核实原件".into()
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && !value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
}
fn regular(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.file_type().is_file() {
        return Err("只能读取普通文件，不能使用链接".into());
    }
    Ok(())
}

impl DocumentStore {
    /// Native binding only: storage location is derived from the fixed ledger
    /// directory, never a renderer-supplied storage path.
    pub fn at(directory: &Path, task_id: &str, generation: u64) -> Result<Self, String> {
        if !text(task_id, 1024) || generation == 0 || !directory.is_dir() {
            return Err(invalid());
        }
        let directory = directory.canonicalize().map_err(io_error)?;
        let name = format!(
            "agentterm-task-{}-{generation}.log",
            hash(task_id.as_bytes())
        );
        let path = directory.join(&name);
        let guard = directory.join(format!("{name}.lock"));
        Ok(Self { path, guard })
    }
    fn lock(&self, create: bool) -> Result<DocumentGuard, String> {
        if self.guard.exists() || fs::symlink_metadata(&self.guard).is_ok() {
            regular(&self.guard)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .open(&self.guard)
            .map_err(io_error)?;
        file.try_lock()
            .map_err(|_| "任务文档正被其他操作占用，请重试".to_string())?;
        Ok(DocumentGuard(file))
    }
    fn open(&self) -> Result<File, String> {
        regular(&self.path)?;
        OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.path)
            .map_err(io_error)
    }
    pub fn exists(&self) -> bool {
        self.path.exists() || fs::symlink_metadata(&self.path).is_ok()
    }
    pub fn create(&self, plan: TaskPlan) -> Result<Document, String> {
        validate_plan(&plan)?;
        validate_roots(&plan)?;
        let _guard = self.lock(true)?;
        if self.exists() {
            let existing = read(&mut self.open()?)?;
            if existing.plan == plan {
                return Ok(existing);
            }
            return Err("任务文档已经存在，不能覆盖原计划".into());
        }
        let document = Document {
            revision: 1,
            plan,
            events: vec![],
            artifacts: vec![],
            session_link: None,
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
            .map_err(io_error)?;
        file.write_all(HEADER.as_bytes()).map_err(io_error)?;
        append(&mut file, &document)?;
        #[cfg(unix)]
        File::open(self.path.parent().ok_or_else(invalid)?)
            .and_then(|parent| parent.sync_all())
            .map_err(io_error)?;
        Ok(document)
    }
    pub fn snapshot(&self) -> Result<Document, String> {
        let _guard = self.lock(false)?;
        read(&mut self.open()?)
    }

    /// Associates one real AgentTerm session with the task record. The link is
    /// write-once so a task cannot silently jump between unrelated sessions.
    pub fn associate_session(&self, link: SessionLink) -> Result<Document, String> {
        validate_session_link(&link)?;
        self.update(|document| match &document.session_link {
            Some(existing) if existing == &link => Ok(false),
            Some(_) => Err("任务已经关联其他会话，不能静默换绑".into()),
            None => {
                document.session_link = Some(link);
                Ok(true)
            }
        })
    }
    fn update(
        &self,
        apply: impl FnOnce(&mut Document) -> Result<bool, String>,
    ) -> Result<Document, String> {
        let _guard = self.lock(false)?;
        let mut file = self.open()?;
        let mut document = read(&mut file)?;
        if apply(&mut document)? {
            document.revision = document.revision.checked_add(1).ok_or_else(invalid)?;
            validate_document(&document)?;
            append(&mut file, &document)?;
        }
        Ok(document)
    }
    /// Native scheduler only. Renderer commands cannot inject run/report events.
    pub fn record(&self, event: AttemptEvent) -> Result<Document, String> {
        self.update(|document| {
            validate_event(&event, &document.plan)?;
            if let Some(existing) = document.events.iter().find(|item| item.id == event.id) {
                if existing == &event {
                    return Ok(false);
                }
                return Err("事件标识已存在但内容不同，拒绝覆盖".into());
            }
            if document.events.len() >= 128
                || document
                    .events
                    .iter()
                    .any(|item| item.sequence == event.sequence)
            {
                return Err("事件序号冲突或已达到数量上限".into());
            }
            document.events.push(event);
            document.events.sort_by_key(|item| item.sequence);
            Ok(true)
        })
    }
    /// Fills the previously recorded start event after the OS accepted the
    /// child into its containment scope. A conflicting identity is rejected.
    pub fn record_process_identity(
        &self,
        event_id: &str,
        identity: ProcessIdentity,
    ) -> Result<Document, String> {
        validate_process_identity(&identity)?;
        self.update(|document| {
            let event = document
                .events
                .iter_mut()
                .find(|item| item.id == event_id)
                .ok_or_else(|| "启动事件不存在，不能写入进程身份".to_string())?;
            if event.state != AttemptState::Running {
                return Err("只有运行中的启动事件可以写入进程身份".into());
            }
            match &event.process_identity {
                Some(existing) if existing == &identity => Ok(false),
                Some(_) => Err("启动事件已有不同进程身份，拒绝覆盖".into()),
                None => {
                    event.process_identity = Some(identity);
                    Ok(true)
                }
            }
        })
    }
    /// Records an explicit stop request before the native process is asked to
    /// stop. Repeating the same request is idempotent; changing its reason is
    /// rejected so the journal cannot rewrite the request history.
    pub fn record_stop_request(&self, event_id: &str, reason: &str) -> Result<Document, String> {
        if !text(reason, 240) {
            return Err("停止原因不能为空且不得超过240字节".into());
        }
        self.update(|document| {
            let event = document
                .events
                .iter_mut()
                .find(|item| item.id == event_id)
                .ok_or_else(|| "启动事件不存在，不能记录停止请求".to_string())?;
            if event.state != AttemptState::Running {
                return Err("只有运行中的启动事件可以记录停止请求".into());
            }
            match &event.stop {
                Some(existing)
                    if existing.requested && !existing.confirmed && existing.reason == reason =>
                {
                    Ok(false)
                }
                Some(_) => Err("启动事件已有不同停止请求，拒绝覆盖".into()),
                None => {
                    event.stop = Some(StopEvidence {
                        requested: true,
                        confirmed: false,
                        reason: reason.into(),
                    });
                    Ok(true)
                }
            }
        })
    }
    pub fn reference(&self, relative_path: &str, label: &str) -> Result<Document, String> {
        if !text(label, 120) {
            return Err("请输入不超过120字节的产物名称".into());
        }
        self.update(|document| {
            let (path, bytes) = artifact_bytes(&document.plan, relative_path)?;
            let relative_path = path
                .strip_prefix(&document.plan.output_dir)
                .map_err(io_error)?
                .to_string_lossy()
                .replace('\\', "/");
            let digest = hash(&bytes);
            if document
                .artifacts
                .iter()
                .any(|item| item.relative_path == relative_path && item.sha256 == digest)
            {
                return Ok(false);
            }
            if document.artifacts.len() >= 32 {
                return Err("产物版本数量达到上限32".into());
            }
            let version = document
                .artifacts
                .iter()
                .filter(|item| item.relative_path == relative_path)
                .map(|item| item.version)
                .max()
                .unwrap_or(0)
                + 1;
            let id = hash(format!("{relative_path}\0{digest}").as_bytes());
            document.artifacts.push(Artifact {
                id,
                relative_path,
                sha256: digest,
                bytes: bytes.len() as u64,
                version,
                label: label.into(),
            });
            Ok(true)
        })
    }
    pub fn preview(&self, id: &str) -> Result<ArtifactPreview, String> {
        let document = self.snapshot()?;
        let artifact = document
            .artifacts
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(|| "产物引用不存在".to_string())?;
        let (_, bytes) = artifact_bytes(&document.plan, &artifact.relative_path)?;
        if hash(&bytes) != artifact.sha256 {
            return Err("文件内容已变化，当前内容不再属于此版本；请登记新版本".into());
        }
        Ok(ArtifactPreview {
            id: id.into(),
            sha256: artifact.sha256.clone(),
            content: String::from_utf8(bytes).map_err(io_error)?,
        })
    }
}

pub fn validate_new_plan(plan: &TaskPlan) -> Result<(), String> {
    validate_plan(plan)?;
    validate_roots(plan)?;
    Ok(())
}
fn validate_plan(plan: &TaskPlan) -> Result<(), String> {
    if !text(&plan.task_id, 1024)
        || plan.generation == 0
        || !text(&plan.project_id, 4096)
        || !text(&plan.goal, 8192)
        || !text(&plan.owner, 120)
        || !text(&plan.scope, 4096)
        || !text(&plan.output_dir, 4096)
        || plan.steps.len() != 2
        || plan.steps[0].id != "worker"
        || plan.steps[1].id != "reviewer"
        || plan
            .steps
            .iter()
            .any(|step| !text(&step.title, 240) || !text(&step.assignee, 120))
    {
        return Err(invalid());
    }
    Ok(())
}
fn validate_roots(plan: &TaskPlan) -> Result<PathBuf, String> {
    let project = Path::new(&plan.project_id);
    let output = Path::new(&plan.output_dir);
    if !project.is_absolute()
        || !output.is_absolute()
        || !project.is_dir()
        || !output.is_dir()
        || project.canonicalize().map_err(io_error)? != project
        || output.canonicalize().map_err(io_error)? != output
        || !output.starts_with(project)
    {
        return Err("产物目录须是项目内已存在且未改变的真实目录".into());
    }
    Ok(output.to_path_buf())
}
fn validate_event(event: &AttemptEvent, plan: &TaskPlan) -> Result<(), String> {
    let parts: Vec<&str> = event.attempt_id.split(':').collect();
    if parts.len() != 3
        || parts[0].parse::<u64>().ok() != Some(plan.generation)
        || !matches!(event.step_id.as_str(), "worker" | "reviewer")
        || !text(&event.summary, 8192)
    {
        return Err(invalid());
    }
    let revision = parts[1].parse::<u64>().map_err(|_| invalid())?;
    let role = if event.step_id == "worker" { "0" } else { "1" };
    let end = event.state != AttemptState::Running;
    let sequence = revision
        .checked_mul(2)
        .and_then(|v| v.checked_add(u64::from(end)))
        .ok_or_else(invalid)?;
    let id = format!(
        "{}:{}",
        event.attempt_id,
        if end { "result" } else { "start" }
    );
    if revision == 0 || parts[2] != role || event.sequence != sequence || event.id != id {
        return Err(invalid());
    }
    if let Some(identity) = &event.process_identity {
        if event.state != AttemptState::Running {
            return Err(invalid());
        }
        validate_process_identity(identity)?;
    }
    if let Some(stop) = &event.stop
        && (!text(&stop.reason, 240)
            || (event.state == AttemptState::Running && (!stop.requested || stop.confirmed))
            || (stop.confirmed && !end))
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_process_identity(identity: &ProcessIdentity) -> Result<(), String> {
    if identity.pid == 0
        || !text(&identity.executable, 4096)
        || identity
            .start_token
            .as_deref()
            .is_some_and(|value| !text(value, 256))
    {
        return Err(invalid());
    }
    Ok(())
}
fn validate_session_link(link: &SessionLink) -> Result<(), String> {
    if !text(&link.id, 256)
        || !text(&link.tool_id, 120)
        || !text(&link.tool_name, 240)
        || !text(&link.project_path, 4096)
    {
        return Err(invalid());
    }
    let path = Path::new(&link.project_path);
    if !path.is_absolute() || !path.is_dir() || path.canonicalize().map_err(io_error)? != path {
        return Err("会话项目路径必须是已存在的真实目录".into());
    }
    Ok(())
}
fn validate_document(document: &Document) -> Result<(), String> {
    validate_plan(&document.plan)?;
    if document.revision == 0 || document.events.len() > 128 || document.artifacts.len() > 32 {
        return Err(invalid());
    }
    if let Some(link) = &document.session_link {
        validate_session_link(link)?;
    }
    let mut previous = 0;
    for event in &document.events {
        validate_event(event, &document.plan)?;
        if event.sequence <= previous {
            return Err(invalid());
        }
        previous = event.sequence;
    }
    let mut ids = std::collections::HashSet::new();
    let mut versions = std::collections::HashMap::new();
    for artifact in &document.artifacts {
        let next = versions.entry(&artifact.relative_path).or_insert(0_u64);
        *next += 1;
        if !safe_relative(&artifact.relative_path)
            || !text(&artifact.label, 120)
            || artifact.sha256.len() != 64
            || !artifact
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || artifact.id
                != hash(format!("{}\0{}", artifact.relative_path, artifact.sha256).as_bytes())
            || artifact.version != *next
            || artifact.bytes > FILE_LIMIT
            || !ids.insert(&artifact.id)
        {
            return Err(invalid());
        }
    }
    Ok(())
}
fn read(file: &mut File) -> Result<Document, String> {
    if file.metadata().map_err(io_error)?.len() > JOURNAL_LIMIT {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take(JOURNAL_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    let text = std::str::from_utf8(&bytes).map_err(io_error)?;
    if bytes.len() as u64 > JOURNAL_LIMIT || !text.starts_with(HEADER) || !text.ends_with('\n') {
        return Err(invalid());
    }
    let mut last: Option<Document> = None;
    for line in text[HEADER.len()..].lines() {
        if line.len() > RECORD_LIMIT {
            return Err(invalid());
        }
        let record: Record = serde_json::from_str(line).map_err(|_| invalid())?;
        validate_document(&record.document)?;
        let encoded = serde_json::to_vec(&record.document).map_err(io_error)?;
        if hash(&encoded) != record.sha256 {
            return Err(invalid());
        }
        if let Some(before) = &last {
            if record.document.revision != before.revision + 1
                || record.document.plan != before.plan
                || !session_link_history_preserved(
                    before.session_link.as_ref(),
                    record.document.session_link.as_ref(),
                )
                || !before.events.iter().all(|old| {
                    record
                        .document
                        .events
                        .iter()
                        .any(|current| event_history_preserved(old, current))
                })
                || !record.document.artifacts.starts_with(&before.artifacts)
            {
                return Err(invalid());
            }
        } else if record.document.revision != 1
            || !record.document.events.is_empty()
            || !record.document.artifacts.is_empty()
            || record.document.session_link.is_some()
        {
            return Err(invalid());
        }
        last = Some(record.document);
    }
    last.ok_or_else(invalid)
}
fn session_link_history_preserved(
    old: Option<&SessionLink>,
    current: Option<&SessionLink>,
) -> bool {
    match (old, current) {
        (None, _) => true,
        (Some(before), Some(after)) => before == after,
        (Some(_), None) => false,
    }
}
fn event_history_preserved(old: &AttemptEvent, current: &AttemptEvent) -> bool {
    if old == current {
        return true;
    }
    if old.id != current.id
        || old.sequence != current.sequence
        || old.attempt_id != current.attempt_id
        || old.step_id != current.step_id
        || old.state != current.state
        || old.summary != current.summary
        || old.state != AttemptState::Running
    {
        return false;
    }
    let identity_added = old.process_identity.is_none() && current.process_identity.is_some();
    let identity_preserved = old.process_identity == current.process_identity || identity_added;
    let stop_added = old.stop.is_none()
        && current
            .stop
            .as_ref()
            .is_some_and(|stop| stop.requested && !stop.confirmed);
    let stop_preserved = old.stop == current.stop || stop_added;
    identity_preserved && stop_preserved && (identity_added || stop_added)
}
fn append(file: &mut File, document: &Document) -> Result<(), String> {
    let encoded = serde_json::to_vec(document).map_err(io_error)?;
    let record = Record {
        document: document.clone(),
        sha256: hash(&encoded),
    };
    let mut line = serde_json::to_vec(&record).map_err(io_error)?;
    line.push(b'\n');
    if line.len() > RECORD_LIMIT
        || file.metadata().map_err(io_error)?.len() + line.len() as u64 > JOURNAL_LIMIT
    {
        return Err("任务文档容量已满，请人工归档".into());
    }
    file.write_all(&line)
        .and_then(|_| file.sync_all())
        .map_err(io_error)
}
fn safe_relative(raw: &str) -> bool {
    text(raw, 4096)
        && !raw.contains('\\')
        && !raw.contains(':')
        && Path::new(raw)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
fn artifact_bytes(plan: &TaskPlan, relative: &str) -> Result<(PathBuf, Vec<u8>), String> {
    let root = validate_roots(plan)?;
    if !safe_relative(relative) {
        return Err("只接受产物目录内的相对文件路径".into());
    }
    let requested = root.join(relative);
    regular(&requested)?;
    let path = requested.canonicalize().map_err(io_error)?;
    if !path.starts_with(&root) {
        return Err("文件真实路径超出产物目录".into());
    }
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    if !matches!(extension, "sql" | "md" | "txt" | "json")
        || path
            .file_name()
            .is_some_and(|s| s.to_string_lossy().starts_with('.'))
    {
        return Err("仅支持普通 SQL、Markdown、TXT 或 JSON 产物".into());
    }
    let file = File::open(&path).map_err(io_error)?;
    if !file.metadata().map_err(io_error)?.is_file()
        || file.metadata().map_err(io_error)?.len() > FILE_LIMIT
    {
        return Err("产物须为不超过1MiB的普通文本文件".into());
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > FILE_LIMIT || std::str::from_utf8(&bytes).is_err() {
        return Err("产物须为不超过1MiB的UTF-8文本".into());
    }
    Ok((path, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (DocumentStore, TaskPlan) {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let id = format!(
            "document-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let plan = TaskPlan {
            task_id: id.clone(),
            generation: 1,
            project_id: root.to_string_lossy().into_owned(),
            goal: "核对草稿".into(),
            owner: "owner".into(),
            scope: "local only".into(),
            output_dir: root.to_string_lossy().into_owned(),
            steps: vec![
                Step {
                    id: "worker".into(),
                    title: "执行".into(),
                    assignee: "执行者".into(),
                },
                Step {
                    id: "reviewer".into(),
                    title: "审查".into(),
                    assignee: "验收者".into(),
                },
            ],
        };
        (DocumentStore::at(&root, &id, 1).unwrap(), plan)
    }
    fn event(end: bool) -> AttemptEvent {
        AttemptEvent {
            id: format!("1:2:0:{}", if end { "result" } else { "start" }),
            sequence: if end { 5 } else { 4 },
            attempt_id: "1:2:0".into(),
            step_id: "worker".into(),
            state: if end {
                AttemptState::Reported
            } else {
                AttemptState::Running
            },
            summary: "证据说明".into(),
            process_identity: None,
            stop: None,
        }
    }
    #[test]
    fn plan_survives_reopen_and_cannot_be_overwritten() {
        let (store, plan) = fixture();
        assert!(!store.exists());
        assert!(store.snapshot().is_err());
        let created = store.create(plan.clone()).unwrap();
        let reopened = DocumentStore::at(store.path.parent().unwrap(), &plan.task_id, 1).unwrap();
        assert_eq!(reopened.snapshot().unwrap(), created);
        assert_eq!(store.create(plan.clone()).unwrap(), created);
        let mut changed = plan;
        changed.goal = "different".into();
        assert!(store.create(changed).is_err());
        assert_eq!(store.snapshot().unwrap(), created);
    }

    #[test]
    fn session_link_is_write_once_and_survives_reopen() {
        let (store, plan) = fixture();
        store.create(plan.clone()).unwrap();
        let link = SessionLink {
            id: "session-1".into(),
            tool_id: "claude".into(),
            tool_name: "Claude Code".into(),
            project_path: plan.project_id.clone(),
        };
        let linked = store.associate_session(link.clone()).unwrap();
        assert_eq!(linked.session_link, Some(link.clone()));
        assert_eq!(store.associate_session(link).unwrap(), linked);
        let different = SessionLink {
            id: "session-2".into(),
            ..linked.session_link.clone().unwrap()
        };
        assert!(store.associate_session(different).is_err());
        assert_eq!(store.snapshot().unwrap().session_link, linked.session_link);
    }
    #[test]
    fn duplicate_and_out_of_order_events_are_idempotent() {
        let (store, plan) = fixture();
        store.create(plan).unwrap();
        store.record(event(true)).unwrap();
        let current = store.record(event(false)).unwrap();
        assert_eq!(current.events, vec![event(false), event(true)]);
        assert_eq!(store.record(event(true)).unwrap(), current);
        let mut conflict = event(true);
        conflict.summary = "changed".into();
        assert!(store.record(conflict).is_err());
        let mut wrong = event(true);
        wrong.attempt_id = "2:2:0".into();
        assert!(store.record(wrong).is_err());
    }

    #[test]
    fn process_identity_is_bound_to_running_event_and_cannot_be_replaced() {
        let (store, plan) = fixture();
        store.create(plan).unwrap();
        store.record(event(false)).unwrap();
        let identity = ProcessIdentity {
            pid: 42,
            executable: "C:/agent.exe".into(),
            start_token: Some("start-1".into()),
        };
        validate_process_identity(&identity).unwrap();
        let saved = store
            .record_process_identity("1:2:0:start", identity.clone())
            .unwrap();
        validate_document(&saved).unwrap();
        assert_eq!(saved.events[0].process_identity, Some(identity.clone()));
        assert_eq!(
            store
                .record_process_identity("1:2:0:start", identity.clone())
                .unwrap(),
            saved
        );
        let mut replacement = identity;
        replacement.pid = 43;
        assert!(
            store
                .record_process_identity("1:2:0:start", replacement)
                .is_err()
        );
    }

    #[test]
    fn stop_request_is_persisted_before_stop_and_is_idempotent() {
        let (store, plan) = fixture();
        store.create(plan).unwrap();
        store.record(event(false)).unwrap();
        let saved = store
            .record_stop_request("1:2:0:start", "用户请求停止")
            .unwrap();
        assert_eq!(
            saved.events[0].stop,
            Some(StopEvidence {
                requested: true,
                confirmed: false,
                reason: "用户请求停止".into(),
            })
        );
        assert_eq!(
            store
                .record_stop_request("1:2:0:start", "用户请求停止")
                .unwrap(),
            saved
        );
        assert!(
            store
                .record_stop_request("1:2:0:start", "另一原因")
                .is_err()
        );
    }
    #[test]
    fn artifact_hash_is_verified_and_new_version_preserves_old_reference() {
        let (store, plan) = fixture();
        store.create(plan.clone()).unwrap();
        let name = format!("{}.sql", plan.task_id);
        let path = Path::new(&plan.output_dir).join(&name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"select 1;").unwrap();
        drop(file);
        let first = store.reference(&name, "SQL").unwrap();
        assert_eq!(store.reference(&name, "SQL").unwrap(), first);
        let artifact = &first.artifacts[0];
        assert_eq!(artifact.sha256, hash(b"select 1;"));
        assert_eq!(store.preview(&artifact.id).unwrap().content, "select 1;");
        fs::write(&path, b"select 2;").unwrap();
        assert!(
            store
                .preview(&artifact.id)
                .unwrap_err()
                .contains("内容已变化")
        );
        let second = store.reference(&name, "SQL").unwrap();
        assert_eq!(second.artifacts.len(), 2);
        assert_eq!(second.artifacts[1].version, 2);
        assert_eq!(second.artifacts[0], *artifact);
    }
    #[test]
    fn outside_missing_binary_or_oversized_artifacts_are_rejected() {
        let (store, plan) = fixture();
        store.create(plan.clone()).unwrap();
        for path in [
            "../outside.sql",
            "/outside.sql",
            "C:/outside.sql",
            "missing.sql",
            "..\\outside.sql",
        ] {
            assert!(store.reference(path, "SQL").is_err());
        }
        let name = format!("{}.txt", plan.task_id);
        let path = Path::new(&plan.output_dir).join(&name);
        fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(store.reference(&name, "binary").is_err());
        fs::write(&path, vec![b'x'; FILE_LIMIT as usize + 1]).unwrap();
        assert!(store.reference(&name, "big").is_err());
        assert_eq!(store.snapshot().unwrap().revision, 1);
    }
    #[cfg(unix)]
    #[test]
    fn symlink_artifact_is_rejected() {
        use std::os::unix::fs::symlink;
        let (store, plan) = fixture();
        store.create(plan.clone()).unwrap();
        let target = store.path.clone();
        let name = format!("{}.sql", plan.task_id);
        symlink(target, Path::new(&plan.output_dir).join(&name)).unwrap();
        assert!(store.reference(&name, "link").is_err());
    }
    #[test]
    fn missing_parent_is_never_created() {
        let path = std::env::temp_dir().join(format!("missing-task-doc-{}", std::process::id()));
        assert!(DocumentStore::at(&path, "task", 1).is_err());
        assert!(!path.exists());
    }
    #[test]
    fn truncated_or_unknown_schema_is_not_repaired() {
        for content in [
            b"AGENTTERM_TASK_DOCUMENT\t99\n".as_slice(),
            b"AGENTTERM_TASK_DOCUMENT\t1\n{\"partial\"",
        ] {
            let (store, plan) = fixture();
            store.create(plan.clone()).unwrap();
            fs::write(&store.path, content).unwrap();
            assert!(store.snapshot().is_err());
            assert!(store.create(plan).is_err());
            assert_eq!(fs::read(&store.path).unwrap(), content);
        }
    }
    #[test]
    fn damaged_checksum_fails_closed() {
        let (store, plan) = fixture();
        store.create(plan).unwrap();
        let text = fs::read_to_string(&store.path)
            .unwrap()
            .replace("核对草稿", "改变草稿");
        fs::write(&store.path, text).unwrap();
        assert!(store.snapshot().is_err());
    }
    #[test]
    fn occupied_guard_prevents_read_or_write() {
        let (store, plan) = fixture();
        store.create(plan).unwrap();
        let guard = store.lock(false).unwrap();
        assert!(store.snapshot().is_err());
        assert!(store.record(event(false)).is_err());
        drop(guard);
        assert_eq!(store.snapshot().unwrap().revision, 1);
    }
    #[test]
    fn document_unlock_does_not_wait_for_duplicated_handles() {
        let (store, plan) = fixture();
        store.create(plan).unwrap();
        let guard = store.lock(false).unwrap();
        let duplicate = guard.0.try_clone().unwrap();
        drop(guard);
        assert_eq!(store.snapshot().unwrap().revision, 1);
        drop(duplicate);
    }
}
