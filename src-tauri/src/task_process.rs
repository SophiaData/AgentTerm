//! Bounded non-interactive child execution. No shell command strings or logs.
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use crate::task_document::ProcessIdentity;

pub(crate) const OUTPUT_LIMIT: usize = 256 * 1024;
const STOP_LIMIT: Duration = Duration::from_secs(2);

pub(crate) struct ProcessSpec {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub input: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedProcessOutput {
    pub exit_code: i32,
    pub output: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct ProcessControl(Arc<AtomicBool>);

impl ProcessControl {
    pub(crate) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Returns true only for the first request. Later clicks are harmless and
    /// do not create another stop operation.
    #[allow(dead_code)]
    pub(crate) fn request_stop(&self) -> bool {
        !self.0.swap(true, Ordering::AcqRel)
    }

    fn stop_requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    Start,
    Input,
    Timeout,
    OutputLimit,
    OutputRead,
    NonZero,
    Descendants,
    StopUnconfirmed,
    StopRequested,
    StopRequestUnconfirmed,
    StopRequestUnrecorded,
    StartIdentityUnrecorded,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessObservation {
    /// The recorded PID is definitely absent on the current OS.
    Exited,
    /// The recorded PID and OS start identity still match.
    Alive,
    /// The PID exists, but its start identity no longer matches the record.
    Reused,
    /// The OS could not prove either absence or identity.
    Unknown,
}

/// Read-only recovery probe. It never signals, opens a writable handle, or
/// changes the task ledger. PID absence is safe to classify as `Exited`; every
/// identity or permission ambiguity stays `Unknown`.
#[allow(dead_code)]
pub(crate) fn observe_process(identity: &ProcessIdentity) -> ProcessObservation {
    #[cfg(unix)]
    {
        return observe_unix(identity);
    }
    #[cfg(windows)]
    {
        return observe_windows(identity);
    }
    #[allow(unreachable_code)]
    ProcessObservation::Unknown
}

pub(crate) fn run(spec: ProcessSpec, timeout: Duration) -> Result<Vec<u8>, ProcessError> {
    run_with_start(spec, timeout, |_| true)
}

/// Run a scheduled Agent CLI without a shell, drain stdout/stderr concurrently,
/// and retain a bounded tail. Unlike the relay path, a non-zero exit is a
/// completed run whose code and output are returned to the scheduler.
pub(crate) fn run_scheduler_process(
    binary: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    timeout: Duration,
    output_limit: usize,
) -> Result<CapturedProcessOutput, ProcessError> {
    if timeout.is_zero() || output_limit == 0 {
        return Err(ProcessError::Start);
    }
    #[cfg(windows)]
    if !binary
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err(ProcessError::Start);
    }

    let mut command = Command::new(&binary);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in [
        "ELECTRON_RUN_AS_NODE",
        "ELECTRON_RENDERER_PORT",
        "TMUX",
        "TMUX_PANE",
        "STY",
    ] {
        command.env_remove(key);
    }
    let mut scope = ProcessScope::new().map_err(|_| ProcessError::Start)?;
    scope.configure(&mut command);
    let mut child = command.spawn().map_err(|_| ProcessError::Start)?;
    if scope.attach(&child).is_err() {
        let _ = child.kill();
        return Err(if reap(&mut child) {
            ProcessError::Start
        } else {
            ProcessError::StopUnconfirmed
        });
    }

    let stdout = child.stdout.take().ok_or(ProcessError::OutputRead)?;
    let stderr = child.stderr.take().ok_or(ProcessError::OutputRead)?;
    let per_stream_limit = (output_limit / 2).max(1);
    let stdout = spawn_read_tail(stdout, per_stream_limit);
    let stderr = spawn_read_tail(stderr, per_stream_limit);

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Err(_) => {
                let _ = scope.stop(&mut child);
                return Err(ProcessError::StopUnconfirmed);
            }
            Ok(None) if started.elapsed() >= timeout => {
                let stopped = scope.stop(&mut child);
                return Err(if stopped {
                    ProcessError::Timeout
                } else {
                    ProcessError::StopUnconfirmed
                });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    };

    let settle = Instant::now();
    while scope.has_processes() && settle.elapsed() < Duration::from_millis(100) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let had_descendants = scope.has_processes();
    let stopped = scope.stop(&mut child);
    if !stopped {
        return Err(ProcessError::StopUnconfirmed);
    }
    let read_deadline = Instant::now() + Duration::from_secs(3);
    let stdout = receive_tail(&stdout, read_deadline).ok_or(ProcessError::StopUnconfirmed)?;
    let stderr = receive_tail(&stderr, read_deadline).ok_or(ProcessError::StopUnconfirmed)?;
    let mut output = stdout.map_err(|_| ProcessError::OutputRead)?;
    let stderr = stderr.map_err(|_| ProcessError::OutputRead)?;
    if !stderr.is_empty() {
        if !output.is_empty() {
            output.push(b'\n');
        }
        output.extend_from_slice(&stderr);
    }
    if had_descendants {
        return Err(ProcessError::Descendants);
    }
    Ok(CapturedProcessOutput {
        exit_code: status.code().unwrap_or(-1),
        output,
    })
}

fn spawn_read_tail(
    reader: impl Read + Send + 'static,
    limit: usize,
) -> std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(read_tail(reader, limit));
    });
    receiver
}

fn receive_tail(
    receiver: &std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>,
    deadline: Instant,
) -> Option<std::io::Result<Vec<u8>>> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .unwrap_or_default();
    receiver.recv_timeout(remaining).ok()
}

fn read_tail(mut reader: impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut tail = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(tail),
            Ok(count) => {
                let incoming = &chunk[..count];
                if incoming.len() >= limit {
                    tail.clear();
                    tail.extend_from_slice(&incoming[incoming.len() - limit..]);
                } else {
                    let remove = tail
                        .len()
                        .saturating_add(incoming.len())
                        .saturating_sub(limit);
                    if remove > 0 {
                        tail.drain(..remove);
                    }
                    tail.extend_from_slice(incoming);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

/// Calls `on_started` after the child has been attached to its containment
/// scope. A false return stops the child and refuses to continue without a
/// durable launch identity.
pub(crate) fn run_with_start(
    spec: ProcessSpec,
    timeout: Duration,
    on_started: impl FnOnce(ProcessIdentity) -> bool,
) -> Result<Vec<u8>, ProcessError> {
    run_with_start_control(spec, timeout, ProcessControl::new(), on_started, || true)
}

pub(crate) fn run_with_start_control(
    spec: ProcessSpec,
    timeout: Duration,
    control: ProcessControl,
    on_started: impl FnOnce(ProcessIdentity) -> bool,
    on_stop_requested: impl FnOnce() -> bool,
) -> Result<Vec<u8>, ProcessError> {
    if timeout.is_zero() || timeout > Duration::from_secs(300) || spec.input.len() > 32 * 1024 {
        return Err(ProcessError::Start);
    }
    // Windows batch/PowerShell wrappers would introduce shell interpretation.
    #[cfg(windows)]
    if !spec
        .binary
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err(ProcessError::Start);
    }
    let started = Instant::now();
    let mut command = Command::new(&spec.binary);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Do not inherit terminal multiplexers or desktop child-process settings.
    for key in [
        "ELECTRON_RUN_AS_NODE",
        "ELECTRON_RENDERER_PORT",
        "TMUX",
        "TMUX_PANE",
        "STY",
    ] {
        command.env_remove(key);
    }
    let mut scope = ProcessScope::new().map_err(|_| ProcessError::Start)?;
    scope.configure(&mut command);
    let mut child = command.spawn().map_err(|_| ProcessError::Start)?;
    if scope.attach(&child).is_err() {
        let _ = child.kill();
        return Err(if reap(&mut child) {
            ProcessError::Start
        } else {
            ProcessError::StopUnconfirmed
        });
    }
    let identity = ProcessIdentity {
        pid: child.id(),
        executable: spec.binary.to_string_lossy().into_owned(),
        start_token: process_start_token(child.id()),
    };
    let mut on_stop_requested = Some(on_stop_requested);
    if !on_started(identity) {
        return Err(if scope.stop(&mut child) {
            ProcessError::StartIdentityUnrecorded
        } else {
            ProcessError::StopUnconfirmed
        });
    }
    if control.stop_requested() {
        let recorded = on_stop_requested.take().is_some_and(|callback| callback());
        return if scope.stop(&mut child) {
            Err(if recorded {
                ProcessError::StopRequested
            } else {
                ProcessError::StopRequestUnrecorded
            })
        } else {
            Err(ProcessError::StopRequestUnconfirmed)
        };
    }
    let mut stdin = child.stdin.take().ok_or(ProcessError::Input)?;
    let mut stdout = child.stdout.take().ok_or(ProcessError::OutputRead)?;
    let (input_tx, input_rx) = mpsc::sync_channel(1);
    let input_thread = std::thread::spawn(move || {
        let result = stdin.write_all(&spec.input);
        drop(stdin);
        let _ = input_tx.send(result.is_ok());
    });
    let oversized = Arc::new(AtomicBool::new(false));
    let reader_oversized = Arc::clone(&oversized);
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    let output_thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        let result = loop {
            match stdout.read(&mut chunk) {
                Ok(0) => break Ok(bytes),
                Ok(count) => {
                    if bytes.len().saturating_add(count) <= OUTPUT_LIMIT {
                        bytes.extend_from_slice(&chunk[..count]);
                    } else {
                        reader_oversized.store(true, Ordering::Release);
                        // Drain until containment stops the producer; never grow the buffer.
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break Err(ProcessError::OutputRead),
            }
        };
        let _ = output_tx.send(result);
    });
    let mut stop_requested = false;
    let mut stop_request_recorded = None;
    let status = loop {
        if control.stop_requested() {
            stop_requested = true;
            stop_request_recorded =
                Some(on_stop_requested.take().is_some_and(|callback| callback()));
            break Err(ProcessError::StopRequested);
        }
        if oversized.load(Ordering::Acquire) {
            break Err(ProcessError::OutputLimit);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Err(_) => break Err(ProcessError::StopUnconfirmed),
            Ok(None) => {}
        }
        if started.elapsed() >= timeout {
            break Err(ProcessError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // Windows job accounting can lag the process handle becoming signalled.
    // A short bounded settle interval avoids mistaking that lag for a child.
    let settle = Instant::now();
    while status.is_ok() && scope.has_processes() && settle.elapsed() < Duration::from_millis(100) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let had_descendants = status.is_ok() && scope.has_processes();
    // Also kills descendants after the direct child exits. A leftover descendant
    // blocks handoff, even if cleanup succeeds.
    stop_requested |= control.stop_requested();
    if stop_requested && stop_request_recorded.is_none() {
        stop_request_recorded = Some(on_stop_requested.take().is_some_and(|callback| callback()));
    }
    let stopped = scope.stop(&mut child);
    let input = input_rx.recv_timeout(STOP_LIMIT);
    let output = output_rx.recv_timeout(STOP_LIMIT);
    if input.is_ok() {
        let _ = input_thread.join();
    }
    if output.is_ok() {
        let _ = output_thread.join();
    }
    if !stopped || input.is_err() || output.is_err() {
        // Keep the ledger occupied. An escaped Unix descendant may retain a pipe;
        // do not hang the coordinator by joining an unconfirmed reader.
        return Err(if stop_requested {
            ProcessError::StopRequestUnconfirmed
        } else {
            ProcessError::StopUnconfirmed
        });
    }
    if stop_requested {
        return Err(if stop_request_recorded == Some(true) {
            ProcessError::StopRequested
        } else {
            ProcessError::StopRequestUnrecorded
        });
    }
    let status = status?;
    if oversized.load(Ordering::Acquire) {
        return Err(ProcessError::OutputLimit);
    }
    if had_descendants {
        return Err(ProcessError::Descendants);
    }
    if !status.success() {
        return Err(ProcessError::NonZero);
    }
    if input != Ok(true) {
        return Err(ProcessError::Input);
    }
    output.map_err(|_| ProcessError::OutputRead)?
}

#[cfg(target_os = "linux")]
pub(crate) fn process_start_token(pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_start_token(&text)
}

#[cfg(target_os = "linux")]
fn parse_start_token(text: &str) -> Option<String> {
    text.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)
        .map(ToOwned::to_owned)
}

#[cfg(unix)]
fn observe_unix(identity: &ProcessIdentity) -> ProcessObservation {
    if identity.pid == 0 || identity.pid > i32::MAX as u32 {
        return ProcessObservation::Unknown;
    }
    // Signal zero only checks existence. A missing /proc mount (notably on
    // macOS) is not evidence that a process has exited.
    if unsafe { libc::kill(identity.pid as i32, 0) } != 0 {
        return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            ProcessObservation::Exited
        } else {
            ProcessObservation::Unknown
        };
    }
    #[cfg(target_os = "linux")]
    {
        observe_linux(identity)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Other Unix platforms have no supported birth-identity probe yet.
        // A live or reused PID must keep recovery blocked.
        ProcessObservation::Unknown
    }
}

#[cfg(target_os = "linux")]
fn observe_linux(identity: &ProcessIdentity) -> ProcessObservation {
    let stat_path = format!("/proc/{}/stat", identity.pid);
    let text = match std::fs::read_to_string(stat_path) {
        Ok(text) => text,
        Err(_) => return ProcessObservation::Unknown,
    };
    let Some(expected_token) = identity.start_token.as_deref() else {
        return ProcessObservation::Unknown;
    };
    let Some(actual_token) = parse_start_token(&text) else {
        return ProcessObservation::Unknown;
    };
    if actual_token != expected_token {
        return ProcessObservation::Reused;
    }
    let expected_executable = match std::path::Path::new(&identity.executable).canonicalize() {
        Ok(path) => path,
        Err(_) => return ProcessObservation::Unknown,
    };
    let actual_executable = match std::fs::read_link(format!("/proc/{}/exe", identity.pid))
        .and_then(|path| path.canonicalize())
    {
        Ok(path) => path,
        Err(_) => return ProcessObservation::Unknown,
    };
    if actual_executable != expected_executable {
        return ProcessObservation::Reused;
    }
    ProcessObservation::Alive
}

#[cfg(windows)]
pub(crate) fn process_start_token(pid: u32) -> Option<String> {
    windows_process::start_token(pid)
}

#[cfg(windows)]
fn observe_windows(identity: &ProcessIdentity) -> ProcessObservation {
    windows_process::observe(identity)
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn process_start_token(_pid: u32) -> Option<String> {
    None
}

#[cfg(windows)]
#[allow(dead_code)]
mod windows_process {
    use super::{ProcessIdentity, ProcessObservation};
    use std::ffi::c_void;

    type Handle = *mut c_void;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_INVALID_PARAMETER: i32 = 87;

    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetExitCodeProcess(handle: Handle, code: *mut u32) -> i32;
        fn GetProcessTimes(
            handle: Handle,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }

    struct HandleGuard(Handle);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn open(pid: u32) -> Result<HandleGuard, ProcessObservation> {
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return Err(
                if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_INVALID_PARAMETER) {
                    ProcessObservation::Exited
                } else {
                    ProcessObservation::Unknown
                },
            );
        }
        Ok(HandleGuard(handle))
    }

    fn token(handle: Handle) -> Result<String, ProcessObservation> {
        let mut creation = FileTime::default();
        let mut exit = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        let ok =
            unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
        if ok == 0 {
            return Err(ProcessObservation::Unknown);
        }
        Ok(((u64::from(creation.high) << 32) | u64::from(creation.low)).to_string())
    }

    pub(super) fn start_token(pid: u32) -> Option<String> {
        let handle = open(pid).ok()?;
        token(handle.0).ok()
    }

    pub(super) fn observe(identity: &ProcessIdentity) -> ProcessObservation {
        let handle = match open(identity.pid) {
            Ok(handle) => handle,
            Err(observation) => return observation,
        };
        let mut exit_code = 0;
        if unsafe { GetExitCodeProcess(handle.0, &mut exit_code) } == 0 {
            return ProcessObservation::Unknown;
        }
        if exit_code != STILL_ACTIVE {
            return ProcessObservation::Exited;
        }
        let Some(expected_token) = identity.start_token.as_deref() else {
            return ProcessObservation::Unknown;
        };
        match token(handle.0) {
            Ok(actual) if actual == expected_token => ProcessObservation::Alive,
            Ok(_) => ProcessObservation::Reused,
            Err(observation) => observation,
        }
    }
}

fn reap(child: &mut Child) -> bool {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Err(_) => return false,
            Ok(None) if started.elapsed() >= STOP_LIMIT => return false,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(unix)]
struct ProcessScope {
    pgid: Option<i32>,
}

#[cfg(unix)]
impl ProcessScope {
    fn new() -> std::io::Result<Self> {
        Ok(Self { pgid: None })
    }
    fn configure(&self, command: &mut Command) {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    fn attach(&mut self, child: &Child) -> std::io::Result<()> {
        self.pgid = Some(child.id() as i32);
        Ok(())
    }
    fn has_processes(&self) -> bool {
        self.pgid.is_some_and(|pid| unsafe { libc::kill(-pid, 0) == 0 }
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH))
    }
    fn stop(&mut self, child: &mut Child) -> bool {
        if let Some(pid) = self.pgid {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        if !reap(child) {
            return false;
        }
        let started = Instant::now();
        while self.has_processes() && started.elapsed() < STOP_LIMIT {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.has_processes() {
            return false;
        }
        self.pgid = None; // Never signal a potentially reused group after reaping.
        true
    }
}

#[cfg(unix)]
impl Drop for ProcessScope {
    fn drop(&mut self) {
        if let Some(pid) = self.pgid {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(windows)]
use windows_scope::ProcessScope;

#[cfg(windows)]
mod windows_scope {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::{io::AsRawHandle, process::CommandExt};
    type Handle = *mut c_void;
    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_limit: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process: usize,
        peak_job: usize,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Accounting {
        user_time: i64,
        kernel_time: i64,
        period_user: i64,
        period_kernel: i64,
        page_faults: u32,
        total_processes: u32,
        active_processes: u32,
        terminated: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ThreadEntry {
        size: u32,
        usage: u32,
        thread_id: u32,
        owner_pid: u32,
        base_priority: i32,
        delta_priority: i32,
        flags: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: Handle, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, data: *const c_void, size: u32) -> i32;
        fn QueryInformationJobObject(
            job: Handle,
            class: i32,
            data: *mut c_void,
            size: u32,
            length: *mut u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Thread32First(snapshot: Handle, entry: *mut ThreadEntry) -> i32;
        fn Thread32Next(snapshot: Handle, entry: *mut ThreadEntry) -> i32;
        fn OpenThread(access: u32, inherit: i32, tid: u32) -> Handle;
        fn ResumeThread(thread: Handle) -> u32;
    }
    struct OwnedHandle(Handle);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    pub(super) struct ProcessScope {
        job: OwnedHandle,
    }
    impl ProcessScope {
        pub(super) fn new() -> std::io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = OwnedHandle(handle);
            let limits = ExtendedLimits {
                basic: BasicLimits {
                    flags: 0x2000,
                    ..Default::default()
                }, // KILL_ON_JOB_CLOSE
                ..Default::default()
            };
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    9,
                    (&limits as *const ExtendedLimits).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self { job })
        }
        pub(super) fn configure(&self, command: &mut Command) {
            // Suspended before assignment: the CLI cannot spawn an uncontained
            // descendant in the spawn -> AssignProcessToJobObject interval.
            command.creation_flags(0x00000004 | 0x08000000);
        }
        pub(super) fn attach(&mut self, child: &Child) -> std::io::Result<()> {
            if unsafe { AssignProcessToJobObject(self.job.0, child.as_raw_handle()) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let snapshot = unsafe { CreateToolhelp32Snapshot(4, 0) };
            if snapshot == -1_isize as Handle {
                return Err(std::io::Error::last_os_error());
            }
            let snapshot = OwnedHandle(snapshot);
            let mut entry = ThreadEntry {
                size: std::mem::size_of::<ThreadEntry>() as u32,
                ..Default::default()
            };
            let mut found = unsafe { Thread32First(snapshot.0, &mut entry) };
            while found != 0 {
                if entry.owner_pid == child.id() {
                    let thread = unsafe { OpenThread(2, 0, entry.thread_id) };
                    if thread.is_null() {
                        return Err(std::io::Error::last_os_error());
                    }
                    let thread = OwnedHandle(thread);
                    if unsafe { ResumeThread(thread.0) } == u32::MAX {
                        return Err(std::io::Error::last_os_error());
                    }
                    return Ok(());
                }
                found = unsafe { Thread32Next(snapshot.0, &mut entry) };
            }
            Err(std::io::Error::other("Suspended child thread unavailable"))
        }
        pub(super) fn has_processes(&self) -> bool {
            let mut info = Accounting::default();
            let ok = unsafe {
                QueryInformationJobObject(
                    self.job.0,
                    1,
                    (&mut info as *mut Accounting).cast(),
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                )
            };
            ok == 0 || info.active_processes != 0
        }
        pub(super) fn stop(&self, child: &mut Child) -> bool {
            if unsafe { TerminateJobObject(self.job.0, 1) } == 0 {
                return false;
            }
            if !reap(child) {
                return false;
            }
            let started = Instant::now();
            while self.has_processes() && started.elapsed() < STOP_LIMIT {
                std::thread::sleep(Duration::from_millis(10));
            }
            !self.has_processes()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_receiver_honors_its_deadline_when_reader_is_stuck() {
        let (_sender, receiver) = std::sync::mpsc::channel();
        let started = Instant::now();
        let result = receive_tail(&receiver, Instant::now() + Duration::from_millis(30));
        assert!(result.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
    }
    fn spec(mode: &str) -> ProcessSpec {
        ProcessSpec {
            binary: std::env::current_exe().unwrap(),
            args: vec![
                "--ignored".into(),
                "--exact".into(),
                helper_name(),
                "--nocapture".into(),
            ],
            cwd: std::env::temp_dir(),
            input: mode.as_bytes().to_vec(),
        }
    }
    fn helper_name() -> String {
        // Works both as a standalone rustc test and inside the desktop crate.
        format!(
            "{}::process_fixture",
            module_path!()
                .split("::")
                .skip(1)
                .collect::<Vec<_>>()
                .join("::")
        )
    }
    #[test]
    #[ignore = "subprocess fixture invoked by runtime tests"]
    #[allow(clippy::zombie_processes)] // This fixture intentionally leaves a child for containment.
    fn process_fixture() {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        match input.as_str() {
            "" => {
                let _ = std::io::stdout().write_all(&vec![b'z'; 1024 * 1024 + 8192]);
            }
            "hang" => std::thread::sleep(Duration::from_secs(30)),
            "hangdescendant" => {
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args(["--ignored", "--exact", &helper_name(), "--nocapture"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                child.stdin.take().unwrap().write_all(b"hang").unwrap();
                std::mem::forget(child);
                std::thread::sleep(Duration::from_secs(30));
            }
            "nonzero" => std::process::exit(7),
            "flood" => {
                let _ = std::io::stdout().write_all(&vec![b'x'; 1024 * 1024 + 8192]);
            }
            "descendant" => {
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args(["--ignored", "--exact", &helper_name(), "--nocapture"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                child.stdin.take().unwrap().write_all(b"hang").unwrap();
                // Deliberately leave a live descendant to exercise containment.
                std::mem::forget(child);
            }
            _ => print!("INPUT={input}"),
        }
    }
    #[test]
    fn input_is_delivered_and_child_exits() {
        let output = run(spec("hello 中文"), Duration::from_secs(5)).unwrap();
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("INPUT=hello 中文")
        );
    }

    #[test]
    fn start_observer_receives_process_identity() {
        let mut identity = None;
        let output = run_with_start(spec("hello"), Duration::from_secs(5), |value| {
            identity = Some(value);
            true
        })
        .unwrap();
        assert!(String::from_utf8(output).unwrap().contains("INPUT=hello"));
        let identity = identity.expect("observer must run after process attachment");
        assert!(identity.pid > 0);
        assert!(!identity.executable.is_empty());
    }

    #[test]
    #[cfg(any(target_os = "linux", windows))]
    fn process_probe_requires_matching_start_identity() {
        let pid = std::process::id();
        let identity = ProcessIdentity {
            pid,
            executable: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            start_token: process_start_token(pid),
        };
        assert_eq!(observe_process(&identity), ProcessObservation::Alive);

        let mut reused = identity.clone();
        reused.start_token = Some("different-start-token".into());
        assert_eq!(observe_process(&reused), ProcessObservation::Reused);

        let mut legacy = identity;
        legacy.start_token = None;
        assert_eq!(observe_process(&legacy), ProcessObservation::Unknown);
    }

    #[test]
    fn live_process_without_start_identity_is_never_reported_exited() {
        let identity = ProcessIdentity {
            pid: std::process::id(),
            executable: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            start_token: None,
        };
        assert_eq!(observe_process(&identity), ProcessObservation::Unknown);
    }

    #[test]
    fn process_probe_classifies_an_absent_pid_as_exited() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let identity = ProcessIdentity {
            pid,
            executable: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            start_token: Some("never-started".into()),
        };
        assert_eq!(observe_process(&identity), ProcessObservation::Exited);
    }

    #[test]
    #[cfg(unix)]
    fn invalid_unix_pid_never_targets_a_process_group_or_proves_exit() {
        for pid in [0, i32::MAX as u32 + 1, u32::MAX] {
            let identity = ProcessIdentity {
                pid,
                executable: "unused".into(),
                start_token: None,
            };
            assert_eq!(observe_process(&identity), ProcessObservation::Unknown);
        }
    }

    #[test]
    fn rejected_start_observer_stops_without_running_the_task() {
        assert_eq!(
            run_with_start(spec("hello"), Duration::from_secs(5), |_| false),
            Err(ProcessError::StartIdentityUnrecorded)
        );
    }

    #[test]
    fn stop_request_is_idempotent_and_bounded() {
        let control = ProcessControl::new();
        let worker_control = control.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let handle = std::thread::spawn(move || {
            run_with_start_control(
                spec("hang"),
                Duration::from_secs(5),
                worker_control,
                |_| {
                    started_tx.send(()).unwrap();
                    true
                },
                || true,
            )
        });
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("child start must be observed");
        assert!(control.request_stop());
        assert!(!control.request_stop());
        assert_eq!(handle.join().unwrap(), Err(ProcessError::StopRequested));
    }
    #[test]
    fn missing_binary_cannot_start() {
        let mut request = spec("x");
        request.binary = request.cwd.join("agentterm-missing-native-cli.exe");
        assert_eq!(
            run(request, Duration::from_secs(1)),
            Err(ProcessError::Start)
        );
    }
    #[test]
    fn invalid_executable_cannot_start() {
        let mut request = spec("x");
        request.binary = request.cwd.clone();
        assert_eq!(
            run(request, Duration::from_secs(1)),
            Err(ProcessError::Start)
        );
    }
    #[test]
    fn nonzero_does_not_pass() {
        assert_eq!(
            run(spec("nonzero"), Duration::from_secs(5)),
            Err(ProcessError::NonZero)
        );
    }
    #[test]
    fn timeout_stops_the_child() {
        let started = Instant::now();
        assert_eq!(
            run(spec("hang"), Duration::from_millis(100)),
            Err(ProcessError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(8));
    }
    #[test]
    fn timeout_stops_the_entire_process_tree() {
        let started = Instant::now();
        assert_eq!(
            run(spec("hangdescendant"), Duration::from_millis(100)),
            Err(ProcessError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(8));
    }
    #[test]
    fn excessive_output_is_rejected() {
        assert_eq!(
            run(spec("flood"), Duration::from_secs(5)),
            Err(ProcessError::OutputLimit)
        );
    }

    #[test]
    fn scheduler_process_drains_large_output_and_keeps_a_bounded_tail() {
        let captured = run_scheduler_process(
            std::env::current_exe().unwrap(),
            vec![
                "--ignored".into(),
                "--exact".into(),
                helper_name(),
                "--nocapture".into(),
            ],
            std::env::temp_dir(),
            Duration::from_secs(15),
            4096,
        )
        .unwrap();
        assert_eq!(captured.exit_code, 0);
        assert!(captured.output.len() <= 4096);
        assert!(
            captured.output.iter().filter(|byte| **byte == b'z').count() > 1024,
            "tail: {}",
            String::from_utf8_lossy(&captured.output)
        );
    }

    #[cfg(windows)]
    #[test]
    fn scheduler_process_rejects_batch_wrappers() {
        assert_eq!(
            run_scheduler_process(
                PathBuf::from("agentterm-test.cmd"),
                Vec::new(),
                std::env::temp_dir(),
                Duration::from_secs(1),
                1024,
            ),
            Err(ProcessError::Start)
        );
    }
    #[test]
    fn surviving_descendant_blocks_handoff() {
        let result = run(spec("descendant"), Duration::from_secs(5));
        #[cfg(windows)]
        assert_eq!(result, Err(ProcessError::Descendants));
        #[cfg(unix)]
        assert!(matches!(
            result,
            Err(ProcessError::Descendants | ProcessError::StopUnconfirmed)
        ));
    }
}
