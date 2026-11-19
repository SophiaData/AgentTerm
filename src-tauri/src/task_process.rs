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

pub(crate) const OUTPUT_LIMIT: usize = 256 * 1024;
const STOP_LIMIT: Duration = Duration::from_secs(2);

pub(crate) struct ProcessSpec {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub input: Vec<u8>,
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
}

pub(crate) fn run(spec: ProcessSpec, timeout: Duration) -> Result<Vec<u8>, ProcessError> {
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
    let status = loop {
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
        return Err(ProcessError::StopUnconfirmed);
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
            "hang" => std::thread::sleep(Duration::from_secs(30)),
            "nonzero" => std::process::exit(7),
            "flood" => {
                let _ = std::io::stdout().write_all(&vec![b'x'; OUTPUT_LIMIT + 8192]);
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
    fn excessive_output_is_rejected() {
        assert_eq!(
            run(spec("flood"), Duration::from_secs(5)),
            Err(ProcessError::OutputLimit)
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
