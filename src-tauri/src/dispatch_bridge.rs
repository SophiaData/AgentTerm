//! Native companion for the local DSH worker. Reuses the desktop occupancy
//! ledger and process-tree supervision; it does not start a Tauri window.
use crate::task_ledger::{Ledger, Phase};
use crate::task_process::{self, ProcessSpec};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

// Each native invocation records intent before spawning. Only an outcome whose
// process-tree cleanup was confirmed gets a durable stopped marker. Missing or
// interrupted evidence keeps recovery closed; PID absence alone is insufficient.
fn execution_directory(ledger: &Path, attempt: &str) -> PathBuf {
    ledger.with_extension("dispatch").join(attempt)
}

fn verify_stopped(directory: &Path) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    if directory
        .symlink_metadata()
        .map_err(|_| "无法核对停止证据")?
        .file_type()
        .is_symlink()
    {
        return Err("停止证据目录不能为链接".into());
    }
    for entry in std::fs::read_dir(directory).map_err(|_| "无法核对停止证据")? {
        let path = entry.map_err(|_| "停止证据无效")?.path();
        let metadata = path.symlink_metadata().map_err(|_| "停止证据无效")?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > 32
            || std::fs::read(&path).map_err(|_| "停止证据不可读")? != b"started\nstopped\n"
        {
            return Err("仍有执行缺少进程树停止证明，保留占用，禁止重跑".into());
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    action: String,
    task_id: String,
    attempt_id: String,
    project_path: Option<PathBuf>,
    state: Option<String>,
    binary: Option<PathBuf>,
    args: Option<Vec<String>>,
    cwd: Option<PathBuf>,
    input: Option<String>,
    timeout_seconds: Option<u64>,
    identity_path: Option<PathBuf>,
}

fn data_directory() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or("LOCALAPPDATA 不可用".into())
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|p| PathBuf::from(p).join("Library/Application Support"))
            .ok_or("HOME 不可用".into())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
            .ok_or("本地数据目录不可用".into())
    }
}

#[tauri::command(async)]
pub fn open_dispatch_dashboard() -> Result<(), String> {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], 47321));
    std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1))
        .map_err(|_| "请先启动本机 DSH 任务服务，再打开协调面板".to_string())?;
    let url = "http://127.0.0.1:47321/";
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let system = std::env::var_os("SystemRoot").ok_or("无法定位系统浏览器入口")?;
        let mut command = std::process::Command::new(PathBuf::from(system).join("explorer.exe"));
        command.arg(url).creation_flags(0x08000000);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg(url);
        command
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    let mut child = command.spawn().map_err(|_| "无法打开协调面板")?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn main_stdio() -> Result<(), String> {
    let mut buffer = String::new();
    std::io::stdin()
        .take(65537)
        .read_to_string(&mut buffer)
        .map_err(|_| "无法读取请求")?;
    if buffer.len() > 65536 {
        return Err("请求过长".into());
    }
    let request: Request = serde_json::from_str(&buffer).map_err(|_| "原生请求无效")?;
    let path = data_directory()?.join("agentterm-main-task-ledger.log");
    let result = operate(&path, request)?;
    println!("{result}");
    Ok(())
}

fn operate(path: &Path, request: Request) -> Result<Value, String> {
    for id in [&request.task_id, &request.attempt_id] {
        if id.is_empty()
            || id.len() > 80
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("任务身份无效".into());
        }
    }
    // Non-PID owner deliberately prevents the legacy UI from treating a dead
    // companion PID as proof the separately supervised execution has stopped.
    let owner = format!("dsh-dispatch:{}:{}", request.task_id, request.attempt_id);
    let ledger = Ledger::at(path).map_err(|e| e.to_string())?;
    let mut snapshot = ledger.initialize().map_err(|e| e.to_string())?;
    if request.action == "claim" && snapshot.active.is_none() {
        let project = request
            .project_path
            .as_ref()
            .ok_or("缺少项目")?
            .canonicalize()
            .map_err(|_| "项目不存在")?;
        if !project.is_dir() {
            return Err("项目不是目录".into());
        }
        snapshot = ledger
            .claim(&request.task_id, &project.to_string_lossy(), &owner)
            .map_err(|e| e.to_string())?;
    }
    if ["release", "abandon"].contains(&request.action.as_str()) && snapshot.active.is_none() {
        return Ok(json!({"ok":true}));
    }
    let active = snapshot.active.as_ref().ok_or("没有当前占用")?;
    if active.owner_id != owner || active.task_id != request.task_id {
        return Err("另一个主任务正在占用，请先处理该任务".into());
    }
    match request.action.as_str() {
        "claim" => {
            if active.phase == Phase::Reserved {
                ledger
                    .set_phase(
                        &owner,
                        &request.task_id,
                        snapshot.generation,
                        snapshot.revision,
                        Phase::Running,
                    )
                    .map_err(|e| e.to_string())?;
            } else if active.phase != Phase::Running {
                return Err("旧执行尚未核实".into());
            }
            Ok(json!({"ok":true}))
        }
        "finish" => {
            let phase = if request.state.as_deref() == Some("awaiting_review") {
                Phase::WaitingInput
            } else {
                Phase::RecoveryRequired
            };
            if active.phase != phase {
                ledger
                    .set_phase(
                        &owner,
                        &request.task_id,
                        snapshot.generation,
                        snapshot.revision,
                        phase,
                    )
                    .map_err(|e| e.to_string())?;
            }
            Ok(json!({"ok":true}))
        }
        "release" | "abandon" => {
            let expected = if request.action == "release" {
                Phase::WaitingInput
            } else {
                Phase::RecoveryRequired
            };
            if active.phase != expected {
                return Err("任务阶段不符合人工释放条件".into());
            }
            verify_stopped(&execution_directory(path, &request.attempt_id))?;
            ledger
                .release_completed_or_recovered(
                    &owner,
                    &request.task_id,
                    snapshot.generation,
                    snapshot.revision,
                )
                .map_err(|e| e.to_string())?;
            Ok(json!({"ok":true}))
        }
        "exec" => {
            if active.phase != Phase::Running {
                return Err("任务不在执行阶段".into());
            }
            let binary = request.binary.ok_or("缺少执行程序")?;
            let cwd = request.cwd.ok_or("缺少工作目录")?;
            if !binary.is_absolute() || !cwd.is_absolute() {
                return Err("执行路径必须为绝对路径".into());
            }
            let timeout = request.timeout_seconds.unwrap_or(300).clamp(1, 300);
            let spec = ProcessSpec {
                binary,
                args: request.args.unwrap_or_default(),
                cwd,
                input: request.input.unwrap_or_default().into_bytes(),
            };
            let directory = execution_directory(path, &request.attempt_id);
            std::fs::create_dir_all(&directory).map_err(|_| "无法保存执行意图")?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "系统时间无效")?
                .as_nanos();
            let mut receipt = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(directory.join(format!("{}-{stamp}.log", std::process::id())))
                .map_err(|_| "无法登记执行意图")?;
            receipt
                .write_all(b"started\n")
                .and_then(|_| receipt.sync_all())
                .map_err(|_| "执行意图未持久保存")?;
            let result =
                task_process::run_with_start(spec, Duration::from_secs(timeout), |identity| {
                    let Some(path) = request.identity_path.as_ref() else {
                        return false;
                    };
                    let Ok(mut file) = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path)
                    else {
                        return false;
                    };
                    serde_json::to_writer(&mut file, &identity).is_ok()
                        && file.flush().is_ok()
                        && file.sync_all().is_ok()
                });
            if result.is_ok()
                || matches!(
                    result,
                    Err(task_process::ProcessError::NonZero
                        | task_process::ProcessError::Timeout
                        | task_process::ProcessError::OutputLimit
                        | task_process::ProcessError::Descendants
                        | task_process::ProcessError::StopRequested
                        | task_process::ProcessError::StopRequestUnrecorded)
                )
            {
                receipt
                    .write_all(b"stopped\n")
                    .and_then(|_| receipt.sync_all())
                    .map_err(|_| "停止证据未持久保存")?;
            }
            match result {
                Ok(output) => Ok(json!({"ok":true,"output":String::from_utf8_lossy(&output)})),
                Err(error) => Ok(json!({"ok":false,"reason":format!("{error:?}")})),
            }
        }
        _ => Err("未知原生操作".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn req(action: &str, project: &Path) -> Request {
        serde_json::from_value(json!({"action":action,"taskId":"task","attemptId":"attempt","projectPath":project,"state":"awaiting_review"})).unwrap()
    }
    #[test]
    fn remote_worker_and_desktop_share_the_same_occupancy() {
        let root = std::env::temp_dir().join(format!(
            "agentterm-dispatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("ledger.log");
        operate(&path, req("claim", &root)).unwrap();
        let ledger = Ledger::at(&path).unwrap();
        assert!(ledger.claim("desktop", "project", "desktop-owner").is_err());
        operate(&path, req("claim", &root)).unwrap();
        assert!(operate(&path, req("release", &root)).is_err());
        operate(&path, req("finish", &root)).unwrap();
        operate(&path, req("release", &root)).unwrap();
        assert!(ledger.claim("desktop", "project", "desktop-owner").is_ok());
        assert!(operate(&path, req("claim", &root)).is_err());
    }
    #[test]
    fn missing_stop_evidence_blocks_abandonment_without_losing_task() {
        let root = std::env::temp_dir().join(format!(
            "agentterm-dispatch-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("ledger.log");
        operate(&path, req("claim", &root)).unwrap();
        let mut failed = req("finish", &root);
        failed.state = Some("failed".into());
        operate(&path, failed).unwrap();
        let evidence = execution_directory(&path, "attempt");
        std::fs::create_dir_all(&evidence).unwrap();
        let receipt = evidence.join("1.log");
        std::fs::write(&receipt, "started\n").unwrap();
        assert!(operate(&path, req("abandon", &root)).is_err());
        assert!(
            Ledger::at(&path)
                .unwrap()
                .snapshot()
                .unwrap()
                .active
                .is_some()
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(receipt)
            .unwrap()
            .write_all(b"stopped\n")
            .unwrap();
        operate(&path, req("abandon", &root)).unwrap();
        operate(&path, req("abandon", &root)).unwrap();
        assert!(
            Ledger::at(&path)
                .unwrap()
                .snapshot()
                .unwrap()
                .active
                .is_none()
        );
    }
}
