//! A bounded, append-only occupancy ledger. This does not launch agents.
//!
//! OS file locks serialize processes; durable records retain occupancy after
//! process exit. Checksums detect accidental damage, not hostile modification.
//! A truncated or invalid record fails closed and is never silently repaired.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::journal;

const HEADER: &[u8] = b"AGENTTERM_TASK_LEDGER\t1\n";
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FIELD_BYTES: usize = 1024;
const COMPACT_MARGIN: u64 = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Reserved,
    RecoveredReserved,
    Running,
    Verifying,
    WaitingInput,
    WaitingApproval,
    Pausing,
    RecoveryRequired,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::RecoveredReserved => "recoveredReserved",
            Self::Running => "running",
            Self::Verifying => "verifying",
            Self::WaitingInput => "waitingInput",
            Self::WaitingApproval => "waitingApproval",
            Self::Pausing => "pausing",
            Self::RecoveryRequired => "recoveryRequired",
        }
    }

    fn parse(value: &str) -> Result<Self, LedgerError> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "recoveredReserved" => Ok(Self::RecoveredReserved),
            "running" => Ok(Self::Running),
            "verifying" => Ok(Self::Verifying),
            "waitingInput" => Ok(Self::WaitingInput),
            "waitingApproval" => Ok(Self::WaitingApproval),
            "pausing" => Ok(Self::Pausing),
            "recoveryRequired" => Ok(Self::RecoveryRequired),
            _ => Err(LedgerError::InvalidData),
        }
    }

    pub fn is_reserved(self) -> bool {
        matches!(self, Self::Reserved | Self::RecoveredReserved)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTask {
    pub task_id: String,
    pub project_id: String,
    pub owner_id: String,
    pub phase: Phase,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub revision: u64,
    pub generation: u64,
    pub active: Option<ActiveTask>,
}

#[derive(Debug)]
pub enum LedgerError {
    NotInitialized,
    InvalidDirectory,
    InvalidPath,
    InvalidData,
    InvalidInput,
    LockUnavailable,
    Occupied,
    Stale,
    RecoveryRequired,
    CannotRelease,
    Capacity,
    Io(std::io::Error),
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::NotInitialized => "任务账本尚未初始化",
            Self::InvalidDirectory => "账本父目录必须已经存在",
            Self::InvalidPath => "账本必须是指定目录内的普通文件，不能是链接",
            Self::InvalidData => "账本版本未知或记录损坏；保持阻断，需人工核实",
            Self::InvalidInput => "任务、项目或执行身份无效",
            Self::LockUnavailable => "账本正在被另一操作占用，稍后重试",
            Self::Occupied => "已有主任务占用执行位置，当前任务须排队",
            Self::Stale => "任务代次或版本已变化，请重新读取",
            Self::RecoveryRequired => "原执行归属当前进程之外，需先核实，禁止自动释放",
            Self::CannotRelease => "任务已进入执行流程，不能通过取消预留释放",
            Self::Capacity => "账本已达到容量上限，需人工归档后继续",
            Self::Io(_) => "账本读写失败；不能假定操作未执行，请先重新核实",
        };
        f.write_str(message)
    }
}

impl std::error::Error for LedgerError {}

impl From<std::io::Error> for LedgerError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone)]
pub struct Ledger {
    path: PathBuf,
    guard_path: PathBuf,
}

// Explicit unlock releases the shared open-file-description lock even when a
// concurrent fork temporarily retains a descriptor before exec/close-on-exec.
struct OperationGuard(File);
impl Drop for OperationGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
struct DataGuard<'a>(&'a mut File);
impl Drop for DataGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl Ledger {
    /// Parent must already exist. This constructor performs no writes.
    pub fn at(path: &Path) -> Result<Self, LedgerError> {
        let parent = path.parent().ok_or(LedgerError::InvalidDirectory)?;
        if !parent.is_dir() {
            return Err(LedgerError::InvalidDirectory);
        }
        let parent = parent.canonicalize()?;
        let name = path.file_name().ok_or(LedgerError::InvalidPath)?;
        let path = parent.join(name);
        if path.exists() || fs::symlink_metadata(&path).is_ok() {
            validate_file_path(&path)?;
        }
        let mut guard_name = path.as_os_str().to_owned();
        guard_name.push(".lock");
        let guard_path = PathBuf::from(guard_name);
        if fs::symlink_metadata(&guard_path).is_ok() {
            validate_file_path(&guard_path)?;
        }
        Ok(Self { path, guard_path })
    }

    /// Explicit first initialization only. Existing or empty damaged files are
    /// read as-is, never truncated or reset. No directory is created.
    pub fn initialize(&self) -> Result<Snapshot, LedgerError> {
        self.initialize_with(|| {})
    }

    fn initialize_with(&self, after_create: impl FnOnce()) -> Result<Snapshot, LedgerError> {
        // The stable guard exists before the data file. Readers cannot enter
        // the create_new -> header-write window, even on first initialization.
        let _guard = self.operation_guard(true)?;
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&self.path)
        {
            Ok(mut file) => {
                after_create();
                lock(&file)?;
                let data = DataGuard(&mut file);
                data.0.write_all(HEADER)?;
                data.0.sync_all()?;
                // Persist the new directory entry on Unix as well as its data.
                #[cfg(unix)]
                File::open(self.path.parent().ok_or(LedgerError::InvalidPath)?)?.sync_all()?;
                Ok(Snapshot::default())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.with_data_locked(read_snapshot)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn snapshot(&self) -> Result<Snapshot, LedgerError> {
        self.with_locked(read_snapshot)
    }

    pub fn claim(
        &self,
        task_id: &str,
        project_id: &str,
        owner_id: &str,
    ) -> Result<Snapshot, LedgerError> {
        validate_field(task_id)?;
        validate_field(project_id)?;
        validate_field(owner_id)?;
        self.with_locked(|file| {
            let before = read_snapshot(file)?;
            if before.active.is_some() {
                return Err(LedgerError::Occupied);
            }
            let after = Snapshot {
                revision: next(before.revision)?,
                generation: next(before.generation)?,
                active: Some(ActiveTask {
                    task_id: task_id.to_string(),
                    project_id: project_id.to_string(),
                    owner_id: owner_id.to_string(),
                    phase: Phase::Reserved,
                }),
            };
            append(file, &after)?;
            Ok(after)
        })
    }

    /// Native dispatcher integration only. Never exposed as a frontend command:
    /// an adapter must record intent before it starts any process/external job.
    pub fn set_phase(
        &self,
        owner_id: &str,
        task_id: &str,
        generation: u64,
        expected_revision: u64,
        phase: Phase,
    ) -> Result<Snapshot, LedgerError> {
        self.with_locked(|file| {
            let mut state = read_snapshot(file)?;
            require_owner(&state, owner_id, task_id, generation, expected_revision)?;
            let task = state.active.as_mut().ok_or(LedgerError::Stale)?;
            if phase.is_reserved() || task.phase == Phase::RecoveryRequired {
                return Err(LedgerError::CannotRelease);
            }
            task.phase = phase;
            state.revision = next(state.revision)?;
            append(file, &state)?;
            Ok(state)
        })
    }

    /// Explicitly reopens a task after a human has confirmed that its prior
    /// dispatched process is gone. This only returns the task to a reserved
    /// state; the caller must create a new attempt with the new revision.
    pub fn resume_after_recovery(
        &self,
        new_owner: &str,
        task_id: &str,
        generation: u64,
        expected_revision: u64,
    ) -> Result<Snapshot, LedgerError> {
        validate_field(new_owner)?;
        self.with_locked(|file| {
            let mut state = read_snapshot(file)?;
            let task = state.active.as_mut().ok_or(LedgerError::Stale)?;
            if state.generation != generation
                || state.revision != expected_revision
                || task.task_id != task_id
            {
                return Err(LedgerError::Stale);
            }
            if task.owner_id == new_owner {
                if task.phase != Phase::RecoveryRequired {
                    return Err(LedgerError::Stale);
                }
            } else if !owner_has_exited(&task.owner_id) || task.phase == Phase::Reserved {
                // An exited worker does not prove its coordinator cannot hand
                // off to the next step. Check the old owner under the CAS lock.
                return Err(LedgerError::RecoveryRequired);
            }
            task.owner_id = new_owner.into();
            task.phase = Phase::RecoveredReserved;
            state.revision = next(state.revision)?;
            append(file, &state)?;
            Ok(state)
        })
    }

    /// Holds the ledger lock while a recovered task updates its session link.
    /// A concurrent dispatcher cannot leave the reservation during the write.
    pub(crate) fn while_recovered<T>(
        &self,
        owner_id: &str,
        task_id: &str,
        generation: u64,
        revision: u64,
        apply: impl FnOnce() -> T,
    ) -> Result<T, LedgerError> {
        self.with_locked(|file| {
            let state = read_snapshot(file)?;
            require_owner(&state, owner_id, task_id, generation, revision)?;
            if state
                .active
                .as_ref()
                .is_none_or(|task| task.phase != Phase::RecoveredReserved)
            {
                return Err(LedgerError::RecoveryRequired);
            }
            Ok(apply())
        })
    }

    /// Only the current process may cancel an untouched reservation. Once
    /// dispatch intent has been recorded, this API cannot free the slot.
    /// Verified stopping/completion is deliberately deferred to runtime work.
    pub fn cancel_reservation(
        &self,
        owner_id: &str,
        task_id: &str,
        generation: u64,
        expected_revision: u64,
    ) -> Result<Snapshot, LedgerError> {
        self.with_locked(|file| {
            let mut state = read_snapshot(file)?;
            require_owner(&state, owner_id, task_id, generation, expected_revision)?;
            if !state
                .active
                .as_ref()
                .ok_or(LedgerError::Stale)?
                .phase
                .is_reserved()
            {
                return Err(LedgerError::CannotRelease);
            }
            state.active = None;
            state.revision = next(state.revision)?;
            append(file, &state)?;
            Ok(state)
        })
    }

    /// Explicit human-confirmed completion/recovery exit. A task in
    /// `waitingInput` or `recoveryRequired` can release the single main-task
    /// slot only after the caller has verified that no recorded process is
    /// still alive. For a previous backend owner, OS absence is rechecked while
    /// the ledger lock is held.
    pub fn release_completed_or_recovered(
        &self,
        new_owner: &str,
        task_id: &str,
        generation: u64,
        expected_revision: u64,
    ) -> Result<Snapshot, LedgerError> {
        validate_field(new_owner)?;
        self.with_locked(|file| {
            let mut state = read_snapshot(file)?;
            let task = state.active.as_ref().ok_or(LedgerError::Stale)?;
            if state.generation != generation
                || state.revision != expected_revision
                || task.task_id != task_id
                || !matches!(task.phase, Phase::WaitingInput | Phase::RecoveryRequired)
            {
                return Err(LedgerError::CannotRelease);
            }
            if task.owner_id != new_owner && !owner_has_exited(&task.owner_id) {
                return Err(LedgerError::RecoveryRequired);
            }
            state.active = None;
            state.revision = next(state.revision)?;
            append(file, &state)?;
            Ok(state)
        })
    }

    /// Only an untouched reservation whose old process is demonstrably gone.
    /// The caller cannot supply a PID, owner, or a frontend "stopped" assertion.
    /// Old clients reject recoveredReserved as an unknown phase (fail closed).
    pub fn recover_reservation(
        &self,
        new_owner: &str,
        task_id: &str,
        generation: u64,
        revision: u64,
        release: bool,
    ) -> Result<Snapshot, LedgerError> {
        self.recover_with(
            new_owner,
            task_id,
            generation,
            revision,
            release,
            owner_has_exited,
        )
    }

    fn recover_with(
        &self,
        new_owner: &str,
        task_id: &str,
        generation: u64,
        revision: u64,
        release: bool,
        stopped: impl FnOnce(&str) -> bool,
    ) -> Result<Snapshot, LedgerError> {
        validate_field(new_owner)?;
        self.with_locked(|file| {
            let mut state = read_snapshot(file)?;
            let old = state.active.as_ref().ok_or(LedgerError::Stale)?;
            if state.generation != generation
                || state.revision != revision
                || old.task_id != task_id
            {
                return Err(LedgerError::Stale);
            }
            if !old.phase.is_reserved() {
                return Err(LedgerError::CannotRelease);
            }
            if old.owner_id == new_owner || !stopped(&old.owner_id) {
                return Err(LedgerError::RecoveryRequired);
            }
            if release {
                state.active = None;
            } else {
                let task = state.active.as_mut().ok_or(LedgerError::Stale)?;
                task.owner_id = new_owner.into();
                task.phase = Phase::RecoveredReserved;
            }
            state.revision = next(state.revision)?;
            append(file, &state)?;
            Ok(state)
        })
    }

    fn with_locked<T>(
        &self,
        operation: impl FnOnce(&mut File) -> Result<T, LedgerError>,
    ) -> Result<T, LedgerError> {
        let _guard = self.operation_guard(false)?;
        self.with_data_locked(operation)
    }

    fn operation_guard(&self, create: bool) -> Result<OperationGuard, LedgerError> {
        let guard = if create {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&self.guard_path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    validate_file_path(&self.guard_path)?;
                    OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&self.guard_path)?
                }
                Err(error) => return Err(error.into()),
            }
        } else {
            validate_file_path(&self.guard_path)?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.guard_path)?
        };
        lock(&guard)?;
        Ok(OperationGuard(guard))
    }

    fn with_data_locked<T>(
        &self,
        operation: impl FnOnce(&mut File) -> Result<T, LedgerError>,
    ) -> Result<T, LedgerError> {
        validate_file_path(&self.path)?;
        let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        lock(&file)?;
        let current = read_snapshot(&mut file)?;
        let file_len = file.metadata()?.len();
        if file_len.saturating_add(COMPACT_MARGIN) > MAX_BYTES {
            let mut compacted = HEADER.to_vec();
            compacted.extend_from_slice(record(&current).as_bytes());
            drop(file);
            journal::replace_and_archive(&self.path, &compacted)?;
            file = OpenOptions::new().read(true).write(true).open(&self.path)?;
            lock(&file)?;
        }
        let data = DataGuard(&mut file);
        // All operations acquire guard then data, in that order. Compaction
        // archives the prior journal and atomically installs one verified
        // snapshot before the next mutation. Process exit releases locks, not
        // durable occupancy.
        operation(data.0)
    }
}

/// PID reuse is conservative: any live process at that PID blocks recovery,
/// regardless of the embedded lifetime timestamp. Only absence/exit is proof.
fn owner_has_exited(owner: &str) -> bool {
    let Some((pid, lifetime)) = owner.split_once('-') else {
        return false;
    };
    let Ok(pid) = pid.parse::<u32>() else {
        return false;
    };
    if pid == 0
        || pid > i32::MAX as u32
        || lifetime.parse::<u128>().ok().filter(|n| *n > 0).is_none()
    {
        return false;
    }
    process_has_exited(pid)
}

#[cfg(windows)]
fn process_has_exited(pid: u32) -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    // SYNCHRONIZE only: no memory access, no termination or privilege changes.
    let handle = unsafe { OpenProcess(0x0010_0000, 0, pid) };
    if handle.is_null() {
        return std::io::Error::last_os_error().raw_os_error() == Some(87);
    }
    let exited = unsafe { WaitForSingleObject(handle, 0) } == 0;
    unsafe {
        CloseHandle(handle);
    }
    exited
}

#[cfg(unix)]
fn process_has_exited(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // Signal zero checks existence only. EPERM and all unknown errors block.
    (unsafe { kill(pid as i32, 0) }) == -1
        && std::io::Error::last_os_error().raw_os_error() == Some(3)
}

#[cfg(not(any(windows, unix)))]
fn process_has_exited(_: u32) -> bool {
    false
}

fn validate_file_path(path: &Path) -> Result<(), LedgerError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(LedgerError::InvalidPath),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(LedgerError::NotInitialized)
        }
        Err(error) => Err(error.into()),
    }
}

fn lock(file: &File) -> Result<(), LedgerError> {
    file.try_lock().map_err(|_| LedgerError::LockUnavailable)
}

fn validate_field(value: &str) -> Result<(), LedgerError> {
    if value.trim().is_empty()
        || value.len() > MAX_FIELD_BYTES
        || value.chars().any(char::is_control)
    {
        Err(LedgerError::InvalidInput)
    } else {
        Ok(())
    }
}

fn next(value: u64) -> Result<u64, LedgerError> {
    value.checked_add(1).ok_or(LedgerError::Capacity)
}

fn require_owner(
    state: &Snapshot,
    owner_id: &str,
    task_id: &str,
    generation: u64,
    revision: u64,
) -> Result<(), LedgerError> {
    let task = state.active.as_ref().ok_or(LedgerError::Stale)?;
    if state.generation != generation || state.revision != revision || task.task_id != task_id {
        return Err(LedgerError::Stale);
    }
    if task.owner_id != owner_id {
        return Err(LedgerError::RecoveryRequired);
    }
    Ok(())
}

fn encode(value: &str) -> String {
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        use std::fmt::Write;
        write!(&mut result, "{byte:02x}").expect("writing to String");
    }
    result
}

fn decode(value: &str) -> Result<String, LedgerError> {
    if value.is_empty()
        || value.len() > MAX_FIELD_BYTES * 2
        || !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(LedgerError::InvalidData);
    }
    let bytes: Result<Vec<_>, _> = (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16))
        .collect();
    let text = String::from_utf8(bytes.map_err(|_| LedgerError::InvalidData)?)
        .map_err(|_| LedgerError::InvalidData)?;
    validate_field(&text).map_err(|_| LedgerError::InvalidData)?;
    Ok(text)
}

// FNV-1a is a damage detector only. The local OS account remains the trust boundary.
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn record(state: &Snapshot) -> String {
    let payload = match &state.active {
        Some(task) => format!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            state.revision,
            state.generation,
            task.phase.name(),
            encode(&task.task_id),
            encode(&task.project_id),
            encode(&task.owner_id)
        ),
        None => format!("{}\t{}\tidle\t\t\t", state.revision, state.generation),
    };
    format!("{payload}\t{:016x}\n", checksum(payload.as_bytes()))
}

fn parse_record(line: &str) -> Result<Snapshot, LedgerError> {
    let (payload, hash) = line.rsplit_once('\t').ok_or(LedgerError::InvalidData)?;
    if hash != format!("{:016x}", checksum(payload.as_bytes())) {
        return Err(LedgerError::InvalidData);
    }
    let fields: Vec<_> = payload.split('\t').collect();
    if fields.len() != 6 {
        return Err(LedgerError::InvalidData);
    }
    let revision = fields[0].parse().map_err(|_| LedgerError::InvalidData)?;
    let generation = fields[1].parse().map_err(|_| LedgerError::InvalidData)?;
    let active = if fields[2] == "idle" {
        if fields[3..].iter().any(|field| !field.is_empty()) {
            return Err(LedgerError::InvalidData);
        }
        None
    } else {
        Some(ActiveTask {
            task_id: decode(fields[3])?,
            project_id: decode(fields[4])?,
            owner_id: decode(fields[5])?,
            phase: Phase::parse(fields[2])?,
        })
    };
    let state = Snapshot {
        revision,
        generation,
        active,
    };
    if record(&state).trim_end_matches('\n') != line {
        return Err(LedgerError::InvalidData);
    }
    Ok(state)
}

fn validate_transition(before: &Snapshot, after: &Snapshot) -> Result<(), LedgerError> {
    if before.revision.checked_add(1) != Some(after.revision) {
        return Err(LedgerError::InvalidData);
    }
    let valid = match (&before.active, &after.active) {
        (None, Some(task)) => {
            before.generation.checked_add(1) == Some(after.generation)
                && task.phase == Phase::Reserved
        }
        (Some(task), None) => {
            before.generation == after.generation
                && matches!(
                    task.phase,
                    Phase::Reserved
                        | Phase::RecoveredReserved
                        | Phase::WaitingInput
                        | Phase::RecoveryRequired
                )
        }
        (Some(old), Some(new)) => {
            before.generation == after.generation
                && old.task_id == new.task_id
                && old.project_id == new.project_id
                && ((new.phase == Phase::RecoveredReserved && old.owner_id != new.owner_id)
                    || (old.phase == Phase::RecoveryRequired
                        && new.phase == Phase::RecoveredReserved)
                    || (old.owner_id == new.owner_id
                        && old.phase != Phase::RecoveryRequired
                        && !new.phase.is_reserved()))
        }
        (None, None) => false,
    };
    if valid {
        Ok(())
    } else {
        Err(LedgerError::InvalidData)
    }
}

fn read_snapshot(file: &mut File) -> Result<Snapshot, LedgerError> {
    if file.metadata()?.len() > MAX_BYTES {
        return Err(LedgerError::Capacity);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(LedgerError::Capacity);
    }
    if !bytes.starts_with(HEADER) || !bytes.ends_with(b"\n") {
        return Err(LedgerError::InvalidData);
    }
    let body = std::str::from_utf8(&bytes[HEADER.len()..]).map_err(|_| LedgerError::InvalidData)?;
    let mut state: Option<Snapshot> = None;
    for line in body.split_terminator('\n') {
        let after = parse_record(line)?;
        if after.revision == 0 {
            return Err(LedgerError::InvalidData);
        }
        if let Some(before) = &state {
            validate_transition(before, &after)?;
        }
        state = Some(after);
    }
    Ok(state.unwrap_or_default())
}

fn append(file: &mut File, state: &Snapshot) -> Result<(), LedgerError> {
    let encoded = record(state);
    let size = file.seek(SeekFrom::End(0))?;
    if size + encoded.len() as u64 > MAX_BYTES {
        return Err(LedgerError::Capacity);
    }
    file.write_all(encoded.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Barrier};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fresh_path() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::var_os("AGENTTERM_LEDGER_TEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        assert!(root.is_dir(), "test directory must already exist");
        root.join(format!(
            "agentterm-ledger-test-{}-{}-{}.log",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    fn fresh() -> Ledger {
        let ledger = Ledger::at(&fresh_path()).unwrap();
        ledger.initialize().unwrap();
        ledger
    }

    #[test]
    fn operation_unlock_is_explicit_even_with_a_duplicated_handle() {
        let ledger = fresh();
        let guard = ledger.operation_guard(false).unwrap();
        let duplicate = guard.0.try_clone().unwrap();
        drop(guard);
        assert!(ledger.snapshot().is_ok());
        drop(duplicate);
        let duplicate = ledger
            .with_data_locked(|file| Ok(file.try_clone()?))
            .unwrap();
        assert!(ledger.snapshot().is_ok());
        drop(duplicate);
    }

    #[test]
    fn explicit_initialization_and_reopening_preserve_occupancy() {
        let path = fresh_path();
        let ledger = Ledger::at(&path).unwrap();
        assert!(matches!(
            ledger.snapshot(),
            Err(LedgerError::NotInitialized)
        ));
        ledger.initialize().unwrap();
        let claimed = ledger.claim("task-1", "项目甲", "process-1").unwrap();
        assert_eq!(Ledger::at(&path).unwrap().initialize().unwrap(), claimed);
        assert!(matches!(
            Ledger::at(&path)
                .unwrap()
                .claim("task-2", "project-b", "process-2"),
            Err(LedgerError::Occupied)
        ));
    }

    #[test]
    fn duplicate_click_and_cross_project_claim_are_rejected() {
        let ledger = fresh();
        ledger.claim("a", "one", "owner").unwrap();
        for (task, project) in [("a", "one"), ("b", "one"), ("c", "two")] {
            assert!(matches!(
                ledger.claim(task, project, "owner"),
                Err(LedgerError::Occupied)
            ));
        }
        assert_eq!(ledger.snapshot().unwrap().revision, 1);
    }

    #[test]
    fn first_creation_hides_the_empty_file_from_other_operations() {
        let ledger = Ledger::at(&fresh_path()).unwrap();
        let creator = ledger.clone();
        let (created_tx, created_rx) = std::sync::mpsc::channel();
        let (continue_tx, continue_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            creator.initialize_with(|| {
                created_tx.send(()).unwrap();
                continue_rx.recv().unwrap();
            })
        });
        created_rx.recv().unwrap();
        // Freeze precisely after create_new and before the data-file lock.
        let read_result = ledger.snapshot();
        let init_result = ledger.initialize();
        let claim_result = ledger.claim("early", "project", "owner");
        continue_tx.send(()).unwrap();
        thread.join().unwrap().unwrap();
        assert!(matches!(read_result, Err(LedgerError::LockUnavailable)));
        assert!(matches!(init_result, Err(LedgerError::LockUnavailable)));
        assert!(matches!(claim_result, Err(LedgerError::LockUnavailable)));
        assert_eq!(ledger.snapshot().unwrap(), Snapshot::default());
        ledger.claim("after-init", "project", "owner").unwrap();
    }

    #[test]
    fn simultaneous_first_initialization_never_poison_the_ledger() {
        let ledger = Ledger::at(&fresh_path()).unwrap();
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let ledger = ledger.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    ledger.initialize()
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(results.iter().any(Result::is_ok));
        assert!(
            results
                .iter()
                .all(|result| matches!(result, Ok(_) | Err(LedgerError::LockUnavailable)))
        );
        assert_eq!(ledger.initialize().unwrap(), Snapshot::default());
        ledger.claim("task", "project", "owner").unwrap();
    }

    #[test]
    fn missing_guard_does_not_make_reading_create_files() {
        let ledger = Ledger::at(&fresh_path()).unwrap();
        assert!(matches!(
            ledger.snapshot(),
            Err(LedgerError::NotInitialized)
        ));
        assert!(!ledger.path.exists());
        assert!(!ledger.guard_path.exists());
        // An empty guard is normal after a process dies before data creation.
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&ledger.guard_path)
            .unwrap();
        assert_eq!(ledger.initialize().unwrap(), Snapshot::default());
    }

    #[test]
    fn waiting_review_and_recovery_hold_the_slot() {
        for phase in [
            Phase::Running,
            Phase::Verifying,
            Phase::WaitingInput,
            Phase::WaitingApproval,
            Phase::Pausing,
            Phase::RecoveryRequired,
        ] {
            let ledger = fresh();
            ledger.claim("a", "one", "owner").unwrap();
            let state = ledger.set_phase("owner", "a", 1, 1, phase).unwrap();
            assert_eq!(state.active.unwrap().phase, phase);
            assert!(matches!(
                ledger.claim("b", "two", "owner"),
                Err(LedgerError::Occupied)
            ));
            assert!(matches!(
                ledger.cancel_reservation("owner", "a", 1, 2),
                Err(LedgerError::CannotRelease)
            ));
            assert!(matches!(
                ledger.set_phase("owner", "a", 1, 2, Phase::Reserved),
                Err(LedgerError::CannotRelease)
            ));
        }
    }

    #[test]
    fn completed_task_can_be_explicitly_released_after_verification() {
        let ledger = fresh();
        let mut state = ledger.claim("task", "project", "owner").unwrap();
        for phase in [Phase::Running, Phase::Verifying, Phase::WaitingInput] {
            state = ledger
                .set_phase("owner", "task", 1, state.revision, phase)
                .unwrap();
        }
        let released = ledger
            .release_completed_or_recovered("owner", "task", 1, state.revision)
            .unwrap();
        assert!(released.active.is_none());
        ledger.claim("next", "project", "owner").unwrap();
    }

    #[test]
    fn explicit_recovery_resume_fences_old_owner_and_returns_reserved_slot() {
        let ledger = fresh();
        let mut old = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let old_owner = format!("{}-1", old.id());
        old.wait().unwrap();
        ledger.claim("task", "project", &old_owner).unwrap();
        let recovery = ledger
            .set_phase(&old_owner, "task", 1, 1, Phase::RecoveryRequired)
            .unwrap();
        let resumed = ledger
            .resume_after_recovery("new-owner", "task", 1, recovery.revision)
            .unwrap();
        assert_eq!(resumed.revision, recovery.revision + 1);
        assert_eq!(resumed.active.as_ref().unwrap().owner_id, "new-owner");
        assert_eq!(
            resumed.active.as_ref().unwrap().phase,
            Phase::RecoveredReserved
        );
        assert!(matches!(
            ledger.set_phase("old-owner", "task", 1, recovery.revision, Phase::Running),
            Err(LedgerError::Stale | LedgerError::RecoveryRequired)
        ));
        assert!(matches!(
            ledger.resume_after_recovery("other-owner", "task", 1, recovery.revision),
            Err(LedgerError::Stale)
        ));
    }

    #[test]
    fn live_coordinator_blocks_recovery_even_when_marked_recovery_required() {
        let ledger = fresh();
        let owner = format!("{}-1", std::process::id());
        ledger.claim("task", "project", &owner).unwrap();
        ledger
            .set_phase(&owner, "task", 1, 1, Phase::RecoveryRequired)
            .unwrap();
        assert!(
            ledger
                .resume_after_recovery("new-owner", "task", 1, 2)
                .is_err()
        );
        assert_eq!(ledger.snapshot().unwrap().revision, 2);
        let resumed = ledger.resume_after_recovery(&owner, "task", 1, 2).unwrap();
        assert_eq!(resumed.revision, 3);
        assert!(
            ledger
                .while_recovered(&owner, "task", 1, 3, || true)
                .unwrap()
        );
        ledger
            .set_phase(&owner, "task", 1, 3, Phase::Running)
            .unwrap();
        assert!(
            ledger
                .while_recovered(&owner, "task", 1, 4, || true)
                .is_err()
        );
    }

    #[test]
    fn cancellation_is_only_for_current_untouched_reservation() {
        let ledger = fresh();
        ledger.claim("a", "one", "owner").unwrap();
        assert!(matches!(
            ledger.cancel_reservation("other-process", "a", 1, 1),
            Err(LedgerError::RecoveryRequired)
        ));
        assert!(
            ledger
                .cancel_reservation("owner", "a", 1, 1)
                .unwrap()
                .active
                .is_none()
        );
        let second = ledger.claim("b", "two", "owner").unwrap();
        assert_eq!((second.revision, second.generation), (3, 2));
        assert!(matches!(
            ledger.cancel_reservation("owner", "a", 1, 1),
            Err(LedgerError::Stale)
        ));
        assert_eq!(ledger.snapshot().unwrap(), second);
    }

    #[test]
    fn stale_revision_cannot_change_newer_phase() {
        let ledger = fresh();
        ledger.claim("a", "one", "owner").unwrap();
        ledger
            .set_phase("owner", "a", 1, 1, Phase::Running)
            .unwrap();
        assert!(matches!(
            ledger.set_phase("owner", "a", 1, 1, Phase::WaitingInput),
            Err(LedgerError::Stale)
        ));
        assert_eq!(
            ledger.snapshot().unwrap().active.unwrap().phase,
            Phase::Running
        );
    }

    #[test]
    fn unknown_empty_and_corrupt_data_fail_closed_without_repair() {
        for bytes in [
            Vec::new(),
            b"AGENTTERM_TASK_LEDGER\t2\n".to_vec(),
            [HEADER, b"partial record"].concat(),
            [HEADER, b"1\t1\tidle\t\t\t\t0000000000000000\n"].concat(),
        ] {
            let path = fresh_path();
            fs::write(&path, &bytes).unwrap();
            let ledger = Ledger::at(&path).unwrap();
            assert!(matches!(ledger.initialize(), Err(LedgerError::InvalidData)));
            assert!(matches!(
                ledger.claim("a", "one", "owner"),
                Err(LedgerError::InvalidData)
            ));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn partial_append_after_claim_does_not_free_occupancy() {
        let ledger = fresh();
        ledger.claim("a", "one", "owner").unwrap();
        OpenOptions::new()
            .append(true)
            .open(&ledger.path)
            .unwrap()
            .write_all(b"2\t")
            .unwrap();
        assert!(matches!(ledger.snapshot(), Err(LedgerError::InvalidData)));
        assert!(matches!(
            ledger.claim("b", "two", "owner"),
            Err(LedgerError::InvalidData)
        ));
    }

    #[test]
    fn checksum_and_transition_corruption_are_detected() {
        let ledger = fresh();
        let original = ledger.claim("a", "one", "owner").unwrap();
        // A valid checksum cannot legitimize an illegal duplicate generation.
        let mut invalid = original;
        invalid.revision = 2;
        invalid.generation = 2;
        OpenOptions::new()
            .append(true)
            .open(&ledger.path)
            .unwrap()
            .write_all(record(&invalid).as_bytes())
            .unwrap();
        assert!(matches!(ledger.snapshot(), Err(LedgerError::InvalidData)));
    }

    #[test]
    fn missing_parent_and_non_file_are_rejected_without_creation() {
        let missing = fresh_path();
        assert!(matches!(
            Ledger::at(&missing.join("ledger.log")),
            Err(LedgerError::InvalidDirectory)
        ));
        assert!(!missing.exists());
        assert!(matches!(
            Ledger::at(&std::env::temp_dir()),
            Err(LedgerError::InvalidPath)
        ));
    }

    #[test]
    fn invalid_identity_cannot_append_a_record() {
        let ledger = fresh();
        for invalid in ["", " ", "a\nb", "a\tb"] {
            assert!(matches!(
                ledger.claim(invalid, "one", "owner"),
                Err(LedgerError::InvalidInput)
            ));
        }
        assert!(matches!(
            ledger.claim(&"x".repeat(MAX_FIELD_BYTES + 1), "one", "owner"),
            Err(LedgerError::InvalidInput)
        ));
        assert_eq!(ledger.snapshot().unwrap(), Snapshot::default());
    }

    #[test]
    fn size_limit_fails_closed() {
        let ledger = fresh();
        let file = OpenOptions::new().write(true).open(&ledger.path).unwrap();
        file.set_len(MAX_BYTES + 1).unwrap();
        assert!(matches!(ledger.snapshot(), Err(LedgerError::Capacity)));
        assert!(matches!(
            ledger.claim("a", "one", "owner"),
            Err(LedgerError::Capacity)
        ));
    }

    #[test]
    fn simultaneous_windows_share_one_slot() {
        let ledger = fresh();
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let ledger = ledger.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    ledger.claim(
                        &format!("task-{index}"),
                        &format!("project-{index}"),
                        "same-process",
                    )
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(results.iter().all(|result| matches!(
            result,
            Ok(_) | Err(LedgerError::Occupied | LedgerError::LockUnavailable)
        )));
        assert_eq!(ledger.snapshot().unwrap().revision, 1);
    }

    // These subprocess entry points are ignored in the ordinary suite and
    // invoked by the tests below. They create files only in an existing folder.
    #[test]
    #[ignore]
    fn process_claim_helper() {
        let path = PathBuf::from(std::env::var_os("AGENTTERM_LEDGER_PROCESS_PATH").unwrap());
        let ledger = Ledger::at(&path).unwrap();
        let result = ledger.claim("child", "child-project", &std::process::id().to_string());
        std::process::exit(match result {
            Ok(_) => 0,
            Err(LedgerError::Occupied | LedgerError::LockUnavailable) => 21,
            Err(_) => 22,
        });
    }

    fn child(path: &Path) -> std::process::Child {
        Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "process_claim_helper", "--test-threads=1"])
            .env("AGENTTERM_LEDGER_PROCESS_PATH", path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn multiple_processes_cannot_reserve_two_tasks() {
        let ledger = fresh();
        let mut children: Vec<_> = (0..6).map(|_| child(&ledger.path)).collect();
        let codes: Vec<_> = children
            .iter_mut()
            .map(|child| child.wait().unwrap().code().unwrap())
            .collect();
        assert_eq!(codes.iter().filter(|&&code| code == 0).count(), 1);
        assert!(codes.iter().all(|code| [0, 21].contains(code)));
        assert_eq!(ledger.snapshot().unwrap().revision, 1);
    }

    #[test]
    fn process_exit_releases_os_lock_but_retains_task() {
        let ledger = fresh();
        assert!(child(&ledger.path).wait().unwrap().success());
        let state = ledger.snapshot().unwrap();
        assert_eq!(state.active.as_ref().unwrap().task_id, "child");
        assert!(matches!(
            ledger.claim("next", "other", "new-process"),
            Err(LedgerError::Occupied)
        ));
        assert!(matches!(
            ledger.cancel_reservation("new-process", "child", state.generation, state.revision),
            Err(LedgerError::RecoveryRequired)
        ));
    }

    #[test]
    fn another_process_cannot_ignore_the_os_lock() {
        let ledger = fresh();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&ledger.path)
            .unwrap();
        file.try_lock().unwrap();
        assert_eq!(child(&ledger.path).wait().unwrap().code(), Some(21));
        drop(file);
        assert_eq!(ledger.snapshot().unwrap(), Snapshot::default());
        assert!(child(&ledger.path).wait().unwrap().success());
    }

    #[test]
    fn recovery_preserves_task_and_fences_old_owner_and_duplicate_tickets() {
        let ledger = fresh();
        ledger.claim("task", "project", "old").unwrap();
        let after = ledger
            .recover_with("new", "task", 1, 1, false, |_| true)
            .unwrap();
        assert_eq!(after.generation, 1);
        assert_eq!(after.revision, 2);
        let task = after.active.unwrap();
        assert_eq!(task.task_id, "task");
        assert_eq!(task.owner_id, "new");
        assert_eq!(task.phase, Phase::RecoveredReserved);
        assert!(
            ledger
                .recover_with("another", "task", 1, 1, true, |_| true)
                .is_err()
        );
        assert!(
            ledger
                .set_phase("old", "task", 1, 1, Phase::Running)
                .is_err()
        );
        assert!(
            ledger
                .set_phase("old", "task", 1, 2, Phase::Running)
                .is_err()
        );
        assert!(ledger.cancel_reservation("old", "task", 1, 2).is_err());
        assert!(ledger.claim("other", "project2", "new").is_err());
        assert!(
            ledger
                .cancel_reservation("new", "task", 1, 2)
                .unwrap()
                .active
                .is_none()
        );
        assert_eq!(ledger.snapshot().unwrap().revision, 3);
    }

    #[test]
    fn unknown_alive_and_dispatched_states_never_release() {
        for phase in [
            Phase::Reserved,
            Phase::Running,
            Phase::Verifying,
            Phase::WaitingInput,
            Phase::WaitingApproval,
            Phase::Pausing,
            Phase::RecoveryRequired,
        ] {
            let ledger = fresh();
            ledger.claim("task", "project", "old").unwrap();
            let before = if phase == Phase::Reserved {
                ledger.snapshot().unwrap()
            } else {
                ledger.set_phase("old", "task", 1, 1, phase).unwrap()
            };
            for release in [false, true] {
                assert!(
                    ledger
                        .recover_with("new", "task", 1, before.revision, release, |_| false)
                        .is_err()
                );
                if phase != Phase::Reserved {
                    assert!(
                        ledger
                            .recover_with("new", "task", 1, before.revision, release, |_| true)
                            .is_err()
                    );
                }
                assert_eq!(ledger.snapshot().unwrap(), before);
            }
        }
        assert!(!owner_has_exited("unknown-owner"));
        assert!(!owner_has_exited("0-1"));
        assert!(!owner_has_exited("4294967295-1"));
        assert!(!owner_has_exited(&format!("{}-1", std::process::id()))); // reused PID blocks
    }

    #[test]
    fn recovery_checks_version_under_lock_and_only_one_contender_wins() {
        let ledger = fresh();
        ledger.claim("task", "project", "old").unwrap();
        let barrier = Arc::new(Barrier::new(6));
        let threads: Vec<_> = (0..6)
            .map(|n| {
                let ledger = ledger.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    ledger.recover_with(&format!("new-{n}"), "task", 1, 1, false, |_| true)
                })
            })
            .collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(ledger.snapshot().unwrap().revision, 2);
        let before = ledger.snapshot().unwrap();
        assert!(
            ledger
                .recover_with("newer", "task", 2, 2, true, |_| true)
                .is_err()
        );
        assert_eq!(ledger.snapshot().unwrap(), before);
    }

    #[test]
    fn actual_exited_child_owner_can_release_a_reservation() {
        let source = fresh();
        let mut process = child(&source.path);
        let pid = process.id();
        assert!(process.wait().unwrap().success());
        // If the OS already reused this PID, recovery conservatively blocks.
        assert!(owner_has_exited(&format!("{pid}-1")));
        let ledger = fresh();
        ledger
            .claim("task", "project", &format!("{pid}-1"))
            .unwrap();
        let recovered = ledger
            .recover_reservation("current", "task", 1, 1, false)
            .unwrap();
        assert_eq!(recovered.active.unwrap().phase, Phase::RecoveredReserved);
        ledger.cancel_reservation("current", "task", 1, 2).unwrap();
        ledger
            .claim("next", "project", &format!("{pid}-1"))
            .unwrap();
        assert!(
            ledger
                .recover_reservation("current", "next", 2, 4, true)
                .unwrap()
                .active
                .is_none()
        );
    }

    #[test]
    fn recovery_corruption_and_stale_versions_preserve_original_bytes() {
        let ledger = fresh();
        ledger.claim("task", "project", "old").unwrap();
        let original = fs::read(&ledger.path).unwrap();
        assert!(
            ledger
                .recover_with("new", "task", 1, 0, true, |_| true)
                .is_err()
        );
        assert_eq!(fs::read(&ledger.path).unwrap(), original);
        OpenOptions::new()
            .append(true)
            .open(&ledger.path)
            .unwrap()
            .write_all(b"truncated")
            .unwrap();
        let damaged = fs::read(&ledger.path).unwrap();
        assert!(
            ledger
                .recover_with("new", "task", 1, 1, true, |_| true)
                .is_err()
        );
        assert_eq!(fs::read(&ledger.path).unwrap(), damaged);
    }

    #[test]
    fn append_compaction_archives_history_and_allows_the_next_transition() {
        let ledger = fresh();
        let long = "x".repeat(900);
        let mut state = Snapshot {
            revision: 1,
            generation: 1,
            active: Some(ActiveTask {
                task_id: long.clone(),
                project_id: long.clone(),
                owner_id: long,
                phase: Phase::Reserved,
            }),
        };
        let mut bytes = HEADER.to_vec();
        bytes.extend_from_slice(record(&state).as_bytes());
        for revision in 2..=1542 {
            state.revision = revision;
            state.active.as_mut().unwrap().phase = if revision % 2 == 0 {
                Phase::Running
            } else {
                Phase::Verifying
            };
            bytes.extend_from_slice(record(&state).as_bytes());
        }
        assert!(bytes.len() as u64 <= MAX_BYTES);
        fs::write(&ledger.path, &bytes).unwrap();

        let active = state.active.as_ref().unwrap();
        let updated = ledger
            .set_phase(
                &active.owner_id,
                &active.task_id,
                1,
                1542,
                Phase::WaitingInput,
            )
            .unwrap();
        assert_eq!(updated.revision, 1543);
        let compacted_len = fs::metadata(&ledger.path).unwrap().len();
        assert!(
            compacted_len < 64 * 1024,
            "ledger remained {compacted_len} bytes"
        );
        assert!(
            fs::read_dir(ledger.path.parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().contains(".archive-"))
        );
    }
}
