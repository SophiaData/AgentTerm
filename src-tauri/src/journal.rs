use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const HARD_READ_LIMIT: u64 = 64 * 1024 * 1024;
const LOCK_RETRIES: usize = 8;
const LOCK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(20);

pub(crate) struct JournalLock(File);

impl Drop for JournalLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
impl JournalLock {
    pub(crate) fn duplicate(&self) -> std::io::Result<File> {
        self.0.try_clone()
    }
}

pub(crate) fn companion(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

pub(crate) fn regular_or_missing(path: &Path) -> std::io::Result<bool> {
    recover_interrupted(path)?;
    plain_regular_or_missing(path)
}

fn plain_regular_or_missing(path: &Path) -> std::io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(std::io::Error::other(
            "journal path must be a regular file, not a link",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub(crate) fn lock(path: &Path, create: bool) -> std::io::Result<JournalLock> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .open(path)?;
    file.try_lock()?;
    Ok(JournalLock(file))
}

/// Bounded retry for read/modify/write callers that can overlap a short
/// maintenance transaction. The operation remains fail-closed after the
/// retry budget and never starts an unbounded polling loop.
pub(crate) fn lock_with_retry(path: &Path, create: bool) -> std::io::Result<JournalLock> {
    let mut last_error = None;
    for attempt in 0..=LOCK_RETRIES {
        match lock(path, create) {
            Ok(lock) => return Ok(lock),
            Err(error) => {
                last_error = Some(error);
                if attempt < LOCK_RETRIES {
                    std::thread::sleep(LOCK_RETRY_DELAY);
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("journal lock unavailable")))
}

/// A compacted journal starts at the retained revision rather than at one.
/// Every later row after the first must still be consecutive.
pub(crate) fn revision_follows(previous: Option<u64>, current: u64) -> bool {
    match previous {
        None => current > 0,
        Some(previous) => previous.checked_add(1) == Some(current),
    }
}

pub(crate) fn read_bytes(path: &Path, hard_limit: u64) -> std::io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let length = file.metadata()?.len();
    if length > hard_limit {
        return Err(std::io::Error::other("journal hard read limit exceeded"));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(hard_limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > hard_limit {
        return Err(std::io::Error::other("journal hard read limit exceeded"));
    }
    Ok(bytes)
}

pub(crate) fn append(path: &Path, bytes: &[u8], create_new: bool) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .append(true)
        .create_new(create_new)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_parent(path)?;
    Ok(())
}

/// Atomically install a compacted journal while retaining the previous file as
/// an archive. The current file stays bounded; prior history is not deleted.
pub(crate) fn replace_and_archive(path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    recover_interrupted(path)?;
    let nonce = unique_nonce();
    let candidate = companion(path, &format!(".compact-{nonce}.tmp"));
    let archive = companion(path, &format!(".archive-{nonce}"));
    let marker = companion(path, ".compacting");
    let mut candidate_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&candidate)?;
    candidate_file.write_all(bytes)?;
    candidate_file.sync_all()?;
    drop(candidate_file);

    let had_existing = plain_regular_or_missing(path)?;
    if had_existing {
        write_marker(&marker, &candidate, &archive)?;
        fs::rename(path, &archive)?;
        sync_parent(path)?;
    }
    if let Err(error) = fs::rename(&candidate, path) {
        if had_existing {
            let _ = fs::rename(&archive, path);
            let _ = fs::remove_file(&marker);
            let _ = sync_parent(path);
        }
        let _ = fs::remove_file(&candidate);
        return Err(error);
    }
    sync_parent(path)?;
    if had_existing {
        fs::remove_file(&marker)?;
        sync_parent(path)?;
    }
    Ok(archive)
}

fn write_marker(marker: &Path, candidate: &Path, archive: &Path) -> std::io::Result<()> {
    let candidate = candidate
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::other("compaction candidate name is invalid"))?;
    let archive = archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::other("compaction archive name is invalid"))?;
    let body = format!("{candidate}\n{archive}\n");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    sync_parent(marker)
}

/// Finish or roll back a compaction whose process died between the two
/// renames. The marker is written and synced before the original file moves;
/// recovery therefore never mistakes an interrupted compaction for first use.
pub(crate) fn recover_interrupted(path: &Path) -> std::io::Result<()> {
    let marker = companion(path, ".compacting");
    let metadata = match fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
        return Err(std::io::Error::other(
            "compaction marker must be a small regular file",
        ));
    }
    let body = fs::read_to_string(&marker)?;
    let mut lines = body.lines();
    let candidate_name = lines.next().unwrap_or_default();
    let archive_name = lines.next().unwrap_or_default();
    if lines.next().is_some()
        || !safe_companion_name(path, candidate_name, ".compact-", ".tmp")
        || !safe_companion_name(path, archive_name, ".archive-", "")
    {
        return Err(std::io::Error::other("compaction marker is invalid"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("journal path has no parent"))?;
    let candidate = parent.join(candidate_name);
    let archive = parent.join(archive_name);
    if !plain_regular_or_missing(path)? {
        if plain_regular_or_missing(&candidate)? {
            fs::rename(&candidate, path)?;
        } else if plain_regular_or_missing(&archive)? {
            fs::rename(&archive, path)?;
        } else {
            return Err(std::io::Error::other(
                "interrupted journal compaction has no recoverable file",
            ));
        }
        sync_parent(path)?;
    } else if plain_regular_or_missing(&candidate)? {
        fs::remove_file(&candidate)?;
    }
    fs::remove_file(&marker)?;
    sync_parent(path)
}

fn safe_companion_name(path: &Path, name: &str, prefix: &str, suffix: &str) -> bool {
    let original = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    !name.contains('/')
        && !name.contains('\\')
        && name.starts_with(&format!("{original}{prefix}"))
        && name.ends_with(suffix)
}

pub(crate) fn sync_parent(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn unique_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentterm-journal-{label}-{}-{}.log",
            std::process::id(),
            unique_nonce()
        ))
    }

    #[test]
    fn recovery_installs_a_synced_candidate_after_the_first_rename() {
        let path = test_path("candidate");
        let candidate = companion(&path, ".compact-test.tmp");
        let archive = companion(&path, ".archive-test");
        let marker = companion(&path, ".compacting");
        fs::write(&path, b"old").unwrap();
        fs::write(&candidate, b"new").unwrap();
        write_marker(&marker, &candidate, &archive).unwrap();
        fs::rename(&path, &archive).unwrap();

        recover_interrupted(&path).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read(&archive).unwrap(), b"old");
        assert!(!marker.exists());
        assert!(!candidate.exists());
    }

    #[test]
    fn recovery_rolls_back_to_the_archive_when_the_candidate_is_missing() {
        let path = test_path("archive");
        let candidate = companion(&path, ".compact-test.tmp");
        let archive = companion(&path, ".archive-test");
        let marker = companion(&path, ".compacting");
        fs::write(&path, b"durable").unwrap();
        write_marker(&marker, &candidate, &archive).unwrap();
        fs::rename(&path, &archive).unwrap();

        recover_interrupted(&path).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"durable");
        assert!(!archive.exists());
        assert!(!marker.exists());
    }
}
