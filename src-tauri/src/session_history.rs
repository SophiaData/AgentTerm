use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::journal;

pub const HISTORY_LIMIT: usize = 50;
const HISTORY_BYTES: u64 = 2 * 1024 * 1024;
pub const SESSION_TITLE_MAX_CHARS: usize = 60;
const SESSION_TITLE_MAX_BYTES: usize = 240;

pub fn normalize_session_title(raw: &str) -> Result<String, String> {
    if raw.chars().any(char::is_control) {
        return Err("会话名称不能包含控制字符或换行".to_string());
    }
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return Err("会话名称不能为空".to_string());
    }
    if normalized.chars().count() > SESSION_TITLE_MAX_CHARS
        || normalized.len() > SESSION_TITLE_MAX_BYTES
    {
        return Err(format!("会话名称最多 {SESSION_TITLE_MAX_CHARS} 个字符"));
    }
    Ok(normalized)
}

/// A persisted manual terminal session. Only identity, timestamps and end state
/// are kept; no output, credentials or environment are stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRecord {
    pub id: String,
    pub tool_id: String,
    pub tool_name: String,
    pub project_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub exit_reason: Option<String>,
    pub output_bytes: Option<u64>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryLine {
    schema: u32,
    revision: u64,
    event: HistoryEvent,
    sha256: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum HistoryEvent {
    Start {
        record: SessionRecord,
    },
    End {
        id: String,
        ended_at_ms: u64,
        exit_reason: Option<String>,
        output_bytes: Option<u64>,
    },
    Rename {
        id: String,
        title: String,
    },
}

/// The immutable, validated storage location. Reads and writes take the
/// companion lock and append verified event lines; a partial or damaged tail
/// fails closed and never resets the original file.
#[derive(Debug, Clone)]
pub struct SessionHistory {
    path: PathBuf,
}

impl SessionHistory {
    pub fn configure(path: PathBuf) -> Result<Self, String> {
        let parent = path
            .parent()
            .filter(|parent| parent.is_dir())
            .ok_or_else(|| "会话历史父目录必须已经存在".to_string())?;
        let parent = parent
            .canonicalize()
            .map_err(|_| "无法读取会话历史目录".to_string())?;
        let name = path
            .file_name()
            .ok_or_else(|| "会话历史路径无效".to_string())?;
        Ok(Self {
            path: parent.join(name),
        })
    }

    pub fn record_start(&self, record: SessionRecord) -> Result<(), String> {
        self.append(HistoryEvent::Start { record })
    }

    pub fn record_end(
        &self,
        id: &str,
        ended_at_ms: u64,
        reason: Option<String>,
        output_bytes: Option<u64>,
    ) -> Result<(), String> {
        self.append(HistoryEvent::End {
            id: id.to_string(),
            ended_at_ms,
            exit_reason: reason,
            output_bytes,
        })
    }

    pub fn record_rename(&self, id: &str, title: &str) -> Result<String, String> {
        let normalized = normalize_session_title(title)?;
        self.append(HistoryEvent::Rename {
            id: id.to_string(),
            title: normalized.clone(),
        })?;
        Ok(normalized)
    }

    /// Newest first. A read never creates or rewrites the store.
    pub fn list(&self) -> Result<Vec<SessionRecord>, String> {
        let guard_path = companion(&self.path);
        let data_exists = regular_or_missing(&self.path)?;
        let guard_exists = regular_or_missing(&guard_path)?;
        if !guard_exists {
            if data_exists {
                return Err("会话历史缺少锁文件；保留原文件，需核实".to_string());
            }
            return Ok(Vec::new());
        }
        let _lock = journal::lock_with_retry(&guard_path, false)
            .map_err(|_| "会话历史正被另一操作使用，请重试".to_string())?;
        let (_, mut records, _) = self.read_existing()?;
        trim_to_limit(&mut records);
        records.reverse();
        Ok(records)
    }

    fn read_existing(&self) -> Result<(u64, Vec<SessionRecord>, u64), String> {
        if !regular_or_missing(&self.path)? {
            return Ok((0, Vec::new(), 0));
        }
        let length = self.path.metadata().map_err(storage_error)?.len();
        let bytes =
            journal::read_bytes(&self.path, journal::HARD_READ_LIMIT).map_err(storage_error)?;
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            return Err(invalid_store());
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid_store())?;
        let mut revision = None;
        let mut records = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for line in text.split_terminator('\n') {
            if line.trim().is_empty() {
                return Err(invalid_store());
            }
            let saved: HistoryLine = serde_json::from_str(line).map_err(|_| invalid_store())?;
            if saved.schema != 2
                || !journal::revision_follows(revision, saved.revision)
                || saved.sha256 != history_hash(saved.revision, &saved.event)?
            {
                return Err(invalid_store());
            }
            apply_event(&mut records, &mut seen, saved.event)?;
            revision = Some(saved.revision);
        }
        Ok((revision.unwrap_or_default(), records, length))
    }

    fn append(&self, event: HistoryEvent) -> Result<(), String> {
        let guard_path = companion(&self.path);
        let initial_data_exists = regular_or_missing(&self.path)?;
        let guard_exists = regular_or_missing(&guard_path)?;
        if initial_data_exists && !guard_exists {
            return Err("会话历史缺少锁文件；保留原文件，需核实".to_string());
        }
        let _lock = journal::lock_with_retry(&guard_path, true)
            .map_err(|_| "会话历史正被另一操作使用，请重试".to_string())?;

        let (revision, mut records, existing_len) = self.read_existing()?;
        let mut seen = records
            .iter()
            .map(|record| record.id.clone())
            .collect::<std::collections::HashSet<_>>();
        apply_event(&mut records, &mut seen, event.clone())?;
        let revision = revision.checked_add(1).ok_or_else(invalid_store)?;
        let line = serde_json::to_vec(&HistoryLine {
            schema: 2,
            revision,
            sha256: history_hash(revision, &event)?,
            event,
        })
        .map_err(|_| "会话历史无法序列化".to_string())?;
        let mut bytes = line;
        bytes.push(b'\n');
        if existing_len + bytes.len() as u64 > HISTORY_BYTES {
            trim_to_limit(&mut records);
            let mut compacted = Vec::new();
            let mut compact_revision = revision;
            for record in records.into_iter() {
                let revision = compact_revision.checked_add(1).ok_or_else(invalid_store)?;
                let event = HistoryEvent::Start { record };
                let row = serde_json::to_vec(&HistoryLine {
                    schema: 2,
                    revision,
                    sha256: history_hash(revision, &event)?,
                    event,
                })
                .map_err(|_| "会话历史无法序列化".to_string())?;
                compacted.extend(row);
                compacted.push(b'\n');
                compact_revision = revision;
            }
            if compacted.len() as u64 > HISTORY_BYTES {
                return Err("会话历史有效记录本身超过 2 MiB".into());
            }
            journal::replace_and_archive(&self.path, &compacted).map_err(storage_error)?;
            return Ok(());
        }
        journal::append(&self.path, &bytes, existing_len == 0).map_err(storage_error)?;
        Ok(())
    }
}

fn trim_to_limit(records: &mut Vec<SessionRecord>) {
    if records.len() > HISTORY_LIMIT {
        let excess = records.len() - HISTORY_LIMIT;
        records.drain(..excess);
    }
}

fn history_hash(revision: u64, event: &HistoryEvent) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(revision, event)).map_err(|_| invalid_store())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn apply_event(
    records: &mut Vec<SessionRecord>,
    seen: &mut std::collections::HashSet<String>,
    event: HistoryEvent,
) -> Result<(), String> {
    match event {
        HistoryEvent::Start { record } => {
            validate_record(&record)?;
            if !seen.insert(record.id.clone()) {
                return Err("会话历史已存在同一条记录".to_string());
            }
            records.push(record);
            Ok(())
        }
        HistoryEvent::End {
            id,
            ended_at_ms,
            exit_reason,
            output_bytes,
        } => {
            let record = records
                .iter_mut()
                .find(|existing| existing.id == id)
                .ok_or_else(|| "会话历史记录不存在".to_string())?;
            // First end wins: a late duplicate from the exit watcher must not
            // overwrite the explicit stop reason or the captured output size.
            if record.ended_at_ms.is_some() {
                return Ok(());
            }
            let mut updated = record.clone();
            updated.ended_at_ms = Some(ended_at_ms);
            updated.exit_reason = exit_reason;
            updated.output_bytes = output_bytes;
            validate_record(&updated)?;
            *record = updated;
            Ok(())
        }
        HistoryEvent::Rename { id, title } => {
            let normalized = normalize_session_title(&title)?;
            if normalized != title {
                return Err("会话名称格式无效；保留原文件，需核实".to_string());
            }
            let record = records
                .iter_mut()
                .find(|existing| existing.id == id)
                .ok_or_else(|| "会话历史记录不存在".to_string())?;
            let mut updated = record.clone();
            updated.title = Some(normalized);
            validate_record(&updated)?;
            *record = updated;
            Ok(())
        }
    }
}

fn validate_record(record: &SessionRecord) -> Result<(), String> {
    let title_ok = record
        .title
        .as_deref()
        .is_none_or(|title| normalize_session_title(title).is_ok_and(|clean| clean == title));
    let reason_ok = record.exit_reason.as_deref().is_none_or(|reason| {
        !reason.is_empty() && reason.len() <= 1024 && !reason.chars().any(char::is_control)
    });
    let ended_ok = record
        .ended_at_ms
        .is_none_or(|ended| ended >= record.started_at_ms);
    if record.id.is_empty()
        || record.id.len() > 256
        || record.tool_id.is_empty()
        || record.tool_id.len() > 256
        || record.tool_name.is_empty()
        || record.tool_name.len() > 256
        || !Path::new(&record.project_path).is_absolute()
        || record.project_path.len() > 4096
        || record.project_path.chars().any(char::is_control)
        || record.id.chars().any(char::is_control)
        || record.tool_id.chars().any(char::is_control)
        || record.tool_name.chars().any(char::is_control)
        || record.started_at_ms == 0
        || !title_ok
        || !reason_ok
        || !ended_ok
    {
        return Err("会话历史记录无效或重复；保留原文件，需核实".to_string());
    }
    Ok(())
}

fn invalid_store() -> String {
    "会话历史版本未知、记录截断或校验失败；保留原文件，需核实".to_string()
}

fn storage_error(_: std::io::Error) -> String {
    "会话历史读写失败；请重新读取并核实，原文件不会自动重置".to_string()
}

fn companion(path: &Path) -> PathBuf {
    journal::companion(path, ".lock")
}

fn regular_or_missing(path: &Path) -> Result<bool, String> {
    journal::regular_or_missing(path).map_err(|_| "会话历史必须是普通文件，不能是链接".to_string())
}

#[cfg(test)]
mod tests {
    use super::{SessionHistory, SessionRecord, normalize_session_title, now_ms};
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_store(label: &str) -> PathBuf {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "agentterm-session-history-{label}-{}-{}-{}.json",
            std::process::id(),
            now_ms(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    fn record(id: &str, started: u64) -> SessionRecord {
        SessionRecord {
            id: id.to_string(),
            tool_id: "claude".to_string(),
            tool_name: "Claude Code".to_string(),
            project_path: if cfg!(windows) {
                "E:\\AgentTerm"
            } else {
                "/tmp/agentterm"
            }
            .to_string(),
            title: None,
            started_at_ms: started,
            ended_at_ms: None,
            exit_reason: None,
            output_bytes: None,
        }
    }

    #[test]
    fn configure_rejects_missing_parent() {
        let missing = std::env::temp_dir()
            .join("agentterm-missing-parent-dir")
            .join("history.json");
        let error = SessionHistory::configure(missing).unwrap_err();
        assert!(error.contains("父目录必须已经存在"), "got: {error}");
    }

    #[test]
    fn start_end_and_restart_list_survive_instances() {
        let path = temp_store("roundtrip");
        let history = SessionHistory::configure(path.clone()).unwrap();
        history.record_start(record("s1", 10)).unwrap();
        history
            .record_end("s1", 20, Some("已停止".to_string()), None)
            .unwrap();
        history.record_start(record("s2", 30)).unwrap();

        let reopened = SessionHistory::configure(path).unwrap();
        let listed = reopened.list().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, "s2");
        assert_eq!(listed[1].id, "s1");
        assert_eq!(listed[1].ended_at_ms, Some(20));
        assert_eq!(listed[1].exit_reason.as_deref(), Some("已停止"));
    }

    #[test]
    fn duplicate_start_fails_closed() {
        let path = temp_store("duplicate");
        let history = SessionHistory::configure(path).unwrap();
        history.record_start(record("s1", 10)).unwrap();
        let error = history.record_start(record("s1", 11)).unwrap_err();
        assert!(error.contains("已存在"), "got: {error}");
    }

    #[test]
    fn a_late_end_does_not_overwrite_the_first_exit_reason() {
        let path = temp_store("late-end");
        let history = SessionHistory::configure(path).unwrap();
        history.record_start(record("s1", 10)).unwrap();
        history
            .record_end("s1", 20, Some("已停止".to_string()), Some(42))
            .unwrap();
        history
            .record_end("s1", 30, Some("会话已结束".to_string()), None)
            .unwrap();

        let listed = history.list().unwrap();
        assert_eq!(listed[0].exit_reason.as_deref(), Some("已停止"));
        assert_eq!(listed[0].output_bytes, Some(42));
        assert_eq!(listed[0].ended_at_ms, Some(20));
    }

    #[test]
    fn end_requires_existing_record() {
        let path = temp_store("end-missing");
        let history = SessionHistory::configure(path).unwrap();
        let error = history.record_end("nope", 20, None, None).unwrap_err();
        assert!(error.contains("不存在"), "got: {error}");
    }

    #[test]
    fn cap_keeps_newest_fifty() {
        let path = temp_store("cap");
        let history = SessionHistory::configure(path).unwrap();
        for index in 0..55 {
            history
                .record_start(record(&format!("s{index}"), index as u64 + 1))
                .unwrap();
        }
        let listed = history.list().unwrap();
        assert_eq!(listed.len(), 50);
        assert_eq!(listed[0].id, "s54");
        assert_eq!(listed[49].id, "s5");
    }

    #[test]
    fn corrupted_tail_preserves_file_and_fails() {
        let path = temp_store("corrupt");
        let history = SessionHistory::configure(path.clone()).unwrap();
        history.record_start(record("s1", 10)).unwrap();
        let before = std::fs::read(&path).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not-json\n")
            .unwrap();
        let error = SessionHistory::configure(path.clone())
            .unwrap()
            .list()
            .unwrap_err();
        assert!(error.contains("校验失败"), "got: {error}");
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after[..before.len()]);
        assert_eq!(after.len(), before.len() + 9);
    }

    #[test]
    fn tampered_checksum_fails_closed() {
        let path = temp_store("tamper");
        let history = SessionHistory::configure(path.clone()).unwrap();
        history.record_start(record("s1", 10)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut damaged = bytes.clone();
        let position = damaged.len() - 3;
        damaged[position] = if damaged[position] == b'0' {
            b'1'
        } else {
            b'0'
        };
        std::fs::write(&path, &damaged).unwrap();
        let error = SessionHistory::configure(path).unwrap().list().unwrap_err();
        assert!(error.contains("校验失败"), "got: {error}");
    }

    #[test]
    fn lock_conflict_reports_retry() {
        let path = temp_store("lock");
        let history = SessionHistory::configure(path.clone()).unwrap();
        history.record_start(record("s1", 10)).unwrap();
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(PathBuf::from(format!("{}.lock", path.display())))
            .unwrap();
        guard.try_lock().unwrap();
        let error = history.record_start(record("s2", 20)).unwrap_err();
        assert!(error.contains("另一操作"), "got: {error}");
    }

    #[test]
    fn invalid_records_are_rejected() {
        let path = temp_store("invalid");
        let history = SessionHistory::configure(path).unwrap();
        let mut bad = record("bad", 20);
        bad.ended_at_ms = Some(5);
        let error = history.record_start(bad).unwrap_err();
        assert!(error.contains("无效"), "got: {error}");
        assert!(history.list().unwrap().is_empty());
    }

    #[test]
    fn truncated_tail_without_newline_preserves_file_and_fails() {
        let path = temp_store("truncated");
        let history = SessionHistory::configure(path.clone()).unwrap();
        history.record_start(record("s1", 10)).unwrap();
        let before = std::fs::read(&path).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"schema\":2")
            .unwrap();
        let error = SessionHistory::configure(path.clone())
            .unwrap()
            .list()
            .unwrap_err();
        assert!(error.contains("校验失败"), "got: {error}");
        assert_eq!(
            &std::fs::read(&path).unwrap()[..before.len()],
            before.as_slice()
        );
    }

    #[test]
    fn rename_persists_across_reopen_and_rejects_invalid_titles() {
        let path = temp_store("rename");
        let history = SessionHistory::configure(path.clone()).unwrap();
        history.record_start(record("s1", 10)).unwrap();

        let saved = history
            .record_rename("s1", "  修复   首页地址乱码  ")
            .unwrap();
        assert_eq!(saved, "修复 首页地址乱码");
        let updated = history.record_rename("s1", "排查 SQL 性能问题").unwrap();
        assert_eq!(updated, "排查 SQL 性能问题");

        assert!(history.record_rename("s1", "   ").is_err());
        assert!(history.record_rename("s1", "换行\n不行").is_err());
        assert!(history.record_rename("s1", &"字".repeat(61)).is_err());
        assert!(history.record_rename("missing", "有效名称").is_err());
        assert_eq!(
            normalize_session_title("  Codex  调试 ").unwrap(),
            "Codex 调试"
        );

        let reopened = SessionHistory::configure(path).unwrap();
        let listed = reopened.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title.as_deref(), Some("排查 SQL 性能问题"));
    }

    #[test]
    fn legacy_start_line_without_title_still_verifies_checksum() {
        let path = temp_store("legacy-no-title");
        let project_path = if cfg!(windows) {
            r"E:\\AgentTerm"
        } else {
            "/tmp/agentterm"
        };
        let legacy_event = format!(
            r#"{{"kind":"start","record":{{"id":"legacy-1","toolId":"claude","toolName":"Claude Code","projectPath":"{project_path}","startedAtMs":100,"endedAtMs":null,"exitReason":null,"outputBytes":null}}}}"#
        );
        let hash_payload = format!("[1,{legacy_event}]");
        let digest = format!("{:x}", Sha256::digest(hash_payload.as_bytes()));
        let legacy_line =
            format!(r#"{{"schema":2,"revision":1,"event":{legacy_event},"sha256":"{digest}"}}"#);
        std::fs::write(&path, format!("{legacy_line}\n")).unwrap();
        std::fs::write(PathBuf::from(format!("{}.lock", path.display())), b"").unwrap();

        let history = SessionHistory::configure(path).unwrap();
        let listed = history.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "legacy-1");
        assert_eq!(listed[0].title, None);

        history.record_rename("legacy-1", "旧记录改名").unwrap();
        let after = history.list().unwrap();
        assert_eq!(after[0].title.as_deref(), Some("旧记录改名"));
    }

    #[test]
    fn append_compaction_keeps_recent_history_and_revision_sequence() {
        let path = temp_store("compact");
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        let mut bytes = Vec::new();
        let mut newest = 0;
        for revision in 1..=400 {
            let mut item = record(&format!("history-{revision}"), revision);
            item.tool_name = "Claude Code ".to_string() + &"x".repeat(240);
            item.project_path = root.to_string() + &"y".repeat(4000);
            let event = super::HistoryEvent::Start { record: item };
            let mut row = serde_json::to_vec(&super::HistoryLine {
                schema: 2,
                revision,
                sha256: super::history_hash(revision, &event).unwrap(),
                event,
            })
            .unwrap();
            row.push(b'\n');
            if bytes.len() as u64 + row.len() as u64 > super::HISTORY_BYTES {
                break;
            }
            bytes.extend_from_slice(&row);
            newest = revision;
        }
        assert!(newest > super::HISTORY_LIMIT as u64);
        std::fs::write(&path, bytes).unwrap();
        std::fs::write(PathBuf::from(format!("{}.lock", path.display())), b"").unwrap();
        let history = SessionHistory::configure(path.clone()).unwrap();
        history
            .record_start({
                let mut item = record("history-new", newest + 1);
                item.tool_name = "Claude Code ".to_string() + &"x".repeat(240);
                item.project_path = root.to_string() + &"y".repeat(4000);
                item
            })
            .unwrap();
        let listed = history.list().unwrap();
        assert_eq!(listed.len(), super::HISTORY_LIMIT);
        assert_eq!(listed[0].id, "history-new");
        assert!(std::fs::metadata(&path).unwrap().len() <= super::HISTORY_BYTES);
        assert!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().contains(".archive-"))
        );
    }
}
