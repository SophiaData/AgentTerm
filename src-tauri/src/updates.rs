use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::journal;
use crate::scheduled_tasks::Scheduler;
use crate::session_history::now_ms;
use crate::sessions::{SessionInfo, SessionManager};
use crate::task_commands::TaskLedgerState;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASE_API: &str = "https://api.github.com/repos/SophiaData/AgentTerm/releases?per_page=100";
const MAX_INSTALLER_BYTES: u64 = 209_715_200;
const RECOVERY_SCHEMA: u32 = 1;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub download_url: Option<String>,
    pub notes: Option<String>,
    pub channel: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateActivity {
    pub live_sessions: Vec<SessionInfo>,
    pub dispatch_active: bool,
    pub scheduled_task_id: Option<String>,
    pub has_active_work: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoverableSession {
    pub id: String,
    pub tool_id: String,
    pub tool_name: String,
    pub project_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_session_id: Option<String>,
}

impl From<&SessionInfo> for RecoverableSession {
    fn from(info: &SessionInfo) -> Self {
        Self {
            id: info.id.clone(),
            tool_id: info.tool_id.clone(),
            tool_name: info.tool_name.clone(),
            project_path: info.project_path.clone(),
            title: info.title.clone(),
            cli_session_id: info.cli_session_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateRecoverySnapshot {
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_version: Option<String>,
    pub sessions: Vec<RecoverableSession>,
    pub had_active_dispatch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_scheduled_task_id: Option<String>,
}

impl UpdateRecoverySnapshot {
    fn is_empty(&self) -> bool {
        self.sessions.is_empty()
            && !self.had_active_dispatch
            && self.interrupted_scheduled_task_id.is_none()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryEnvelope {
    schema: u32,
    payload: Option<UpdateRecoverySnapshot>,
    sha256: String,
}

#[derive(Debug, Clone)]
struct PreparedInstaller {
    download_url: String,
    latest_version: Option<String>,
    installer_path: PathBuf,
    sha256: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedUpdateStatus {
    pub download_url: String,
    pub latest_version: Option<String>,
    pub ready: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum InstallUpdateOutcome {
    Installing {
        version: Option<String>,
    },
    Blocked {
        activity: UpdateActivity,
        #[serde(rename = "installerReady")]
        installer_ready: bool,
    },
}

#[derive(Default)]
pub struct UpdateManager {
    prepared: Mutex<Option<PreparedInstaller>>,
    recovery_path: Mutex<Option<PathBuf>>,
}

impl UpdateManager {
    pub fn configure_recovery(&self, path: PathBuf) -> Result<(), String> {
        let mut guard = self
            .recovery_path
            .lock()
            .map_err(|_| "更新恢复状态不可用".to_string())?;
        if guard.is_some() {
            return Err("更新恢复路径已经绑定，不能切换".to_string());
        }
        let parent = path
            .parent()
            .filter(|parent| parent.is_dir())
            .ok_or_else(|| "更新恢复父目录必须已经存在".to_string())?;
        let parent = parent
            .canonicalize()
            .map_err(|_| "无法读取更新恢复目录".to_string())?;
        let name = path
            .file_name()
            .ok_or_else(|| "更新恢复路径无效".to_string())?;
        *guard = Some(parent.join(name));
        Ok(())
    }

    fn recovery_file(&self) -> Result<PathBuf, String> {
        self.recovery_path
            .lock()
            .map_err(|_| "更新恢复状态不可用".to_string())?
            .clone()
            .ok_or_else(|| "更新恢复路径尚未配置".to_string())
    }

    pub fn save_recovery(&self, snapshot: Option<UpdateRecoverySnapshot>) -> Result<(), String> {
        let path = self.recovery_file()?;
        let guard_path = journal::companion(&path, ".lock");
        let data_exists = regular_or_missing(&path)?;
        let guard_exists = regular_or_missing(&guard_path)?;
        if data_exists && !guard_exists {
            return Err("更新恢复文件缺少锁文件；保留原文件，需核实".to_string());
        }
        let _lock = journal::lock_with_retry(&guard_path, true)
            .map_err(|_| "更新恢复文件正被另一操作使用，请重试".to_string())?;
        let payload = snapshot.filter(|item| !item.is_empty());
        let sha256 = recovery_hash(&payload)?;
        let bytes = serde_json::to_vec(&RecoveryEnvelope {
            schema: RECOVERY_SCHEMA,
            payload,
            sha256,
        })
        .map_err(|_| "无法序列化更新恢复状态".to_string())?;
        let archive = journal::replace_and_archive(&path, &bytes)
            .map_err(|_| "无法写入更新恢复状态".to_string())?;
        let _ = std::fs::remove_file(archive);
        Ok(())
    }

    pub fn load_recovery(&self) -> Result<Option<UpdateRecoverySnapshot>, String> {
        let path = self.recovery_file()?;
        let guard_path = journal::companion(&path, ".lock");
        let data_exists = regular_or_missing(&path)?;
        let guard_exists = regular_or_missing(&guard_path)?;
        if !data_exists {
            return Ok(None);
        }
        if !guard_exists {
            return Err("更新恢复文件缺少锁文件；保留原文件，需核实".to_string());
        }
        let _lock = journal::lock_with_retry(&guard_path, false)
            .map_err(|_| "更新恢复文件正被另一操作使用，请重试".to_string())?;
        read_recovery_from_path(&path)
    }

    pub fn consume_recovery_session(
        &self,
        session_id: &str,
    ) -> Result<Option<UpdateRecoverySnapshot>, String> {
        let path = self.recovery_file()?;
        let guard_path = journal::companion(&path, ".lock");
        let data_exists = regular_or_missing(&path)?;
        let guard_exists = regular_or_missing(&guard_path)?;
        if !data_exists {
            return Ok(None);
        }
        if !guard_exists {
            return Err("更新恢复文件缺少锁文件；保留原文件，需核实".to_string());
        }
        let _lock = journal::lock_with_retry(&guard_path, true)
            .map_err(|_| "更新恢复文件正被另一操作使用，请重试".to_string())?;
        let Some(mut snapshot) = read_recovery_from_path(&path)? else {
            return Ok(None);
        };
        snapshot.sessions.retain(|item| item.id != session_id);
        let next = (!snapshot.is_empty()).then_some(snapshot);
        let sha256 = recovery_hash(&next)?;
        let bytes = serde_json::to_vec(&RecoveryEnvelope {
            schema: RECOVERY_SCHEMA,
            payload: next.clone(),
            sha256,
        })
        .map_err(|_| "无法序列化更新恢复状态".to_string())?;
        let archive = journal::replace_and_archive(&path, &bytes)
            .map_err(|_| "无法写入更新恢复状态".to_string())?;
        let _ = std::fs::remove_file(archive);
        Ok(next)
    }

    fn reusable_prepared(&self, download_url: &str) -> Option<PreparedInstaller> {
        let guard = self.prepared.lock().ok()?;
        let candidate = guard.as_ref()?.clone();
        drop(guard);
        if candidate.download_url != download_url {
            return None;
        }
        if verify_prepared_file(&candidate.installer_path, &candidate.sha256).is_ok() {
            Some(candidate)
        } else {
            if let Ok(mut guard) = self.prepared.lock()
                && guard
                    .as_ref()
                    .is_some_and(|item| item.download_url == download_url)
            {
                *guard = None;
            }
            None
        }
    }

    fn store_prepared(&self, prepared: PreparedInstaller) {
        if let Ok(mut guard) = self.prepared.lock() {
            *guard = Some(prepared);
        }
    }

    fn clear_prepared(&self) {
        if let Ok(mut guard) = self.prepared.lock() {
            *guard = None;
        }
    }
}

fn recovery_hash(payload: &Option<UpdateRecoverySnapshot>) -> Result<String, String> {
    let bytes = serde_json::to_vec(payload).map_err(|_| "无法校验更新恢复状态".to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn read_recovery_from_path(path: &Path) -> Result<Option<UpdateRecoverySnapshot>, String> {
    let bytes = journal::read_bytes(path, journal::HARD_READ_LIMIT)
        .map_err(|_| "无法读取更新恢复文件".to_string())?;
    let envelope: RecoveryEnvelope = serde_json::from_slice(&bytes)
        .map_err(|_| "更新恢复文件格式损坏，已保留原文件".to_string())?;
    if envelope.schema != RECOVERY_SCHEMA || envelope.sha256 != recovery_hash(&envelope.payload)? {
        return Err("更新恢复文件校验失败，已保留原文件".to_string());
    }
    Ok(envelope.payload)
}

fn regular_or_missing(path: &Path) -> Result<bool, String> {
    journal::regular_or_missing(path)
        .map_err(|_| "更新恢复文件必须是普通文件，不能是链接".to_string())
}

fn collect_update_activity(
    sessions: &SessionManager,
    tasks: &TaskLedgerState,
    scheduler: &Scheduler,
) -> Result<UpdateActivity, String> {
    let live_sessions = sessions.list_live()?;
    let dispatch_active = tasks.is_dispatch_active();
    let scheduled_task_id = scheduler.running_task()?;
    let has_active_work =
        !live_sessions.is_empty() || dispatch_active || scheduled_task_id.is_some();
    Ok(UpdateActivity {
        live_sessions,
        dispatch_active,
        scheduled_task_id,
        has_active_work,
    })
}

fn build_recovery_snapshot(
    activity: &UpdateActivity,
    target_version: Option<String>,
) -> Option<UpdateRecoverySnapshot> {
    let snapshot = UpdateRecoverySnapshot {
        created_at_ms: now_ms(),
        target_version,
        sessions: activity
            .live_sessions
            .iter()
            .map(RecoverableSession::from)
            .collect(),
        had_active_dispatch: activity.dispatch_active,
        interrupted_scheduled_task_id: activity.scheduled_task_id.clone(),
    };
    (!snapshot.is_empty()).then_some(snapshot)
}

#[derive(Debug, serde::Deserialize)]
struct Release {
    tag_name: String,
    body: Option<String>,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Debug, serde::Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

pub fn check_latest() -> Result<UpdateStatus, String> {
    let releases: Vec<Release> = serde_json::from_slice(&http_get(RELEASE_API)?)
        .map_err(|error| format!("无法读取版本信息：{error}"))?;
    select_update(&releases, CURRENT_VERSION)
}

/// Validate, download and write the installer, but do not start it. Launching
/// is deferred until this process has exited, otherwise the installer would
/// have to kill the running application to replace its executable.
pub fn prepare_installer(url: &str) -> Result<(PathBuf, String), String> {
    if !trusted_release_url(url) {
        return Err("更新地址不在 AgentTerm 官方发布页".into());
    }
    let releases: Vec<Release> = serde_json::from_slice(&http_get(RELEASE_API)?)
        .map_err(|error| format!("无法重新校验更新信息：{error}"))?;
    let digest = installer_digest(&releases, CURRENT_VERSION, url)?;
    let bytes = http_get(url)?;
    let actual_sha256 = verify_digest(&bytes, &digest)?;
    let extension = if cfg!(target_os = "macos") {
        "dmg"
    } else {
        "exe"
    };
    let path = unique_update_path(extension);
    write_new(&path, &bytes)?;
    Ok((path, actual_sha256))
}

/// The digest GitHub publishes for `url`, but only when `url` is the update
/// this build is currently allowed to install.
fn installer_digest(releases: &[Release], current: &str, url: &str) -> Result<String, String> {
    let status = select_update(releases, current)?;
    if !status.update_available || status.download_url.as_deref() != Some(url) {
        return Err("更新地址不是当前允许安装的最新版本".into());
    }
    releases
        .iter()
        .flat_map(|release| &release.assets)
        .find(|asset| asset.browser_download_url == url)
        .and_then(|asset| asset.digest.clone())
        .ok_or_else(|| "更新包缺少 GitHub SHA-256 digest，已拒绝安装".to_string())
}

fn select_update(releases: &[Release], current: &str) -> Result<UpdateStatus, String> {
    let current_version =
        semver::Version::parse(current).map_err(|_| "当前应用版本号无效".to_string())?;
    let mut candidates = releases
        .iter()
        .filter_map(|release| {
            let version = semver::Version::parse(release.tag_name.trim_start_matches('v')).ok()?;
            let asset = platform_asset(&release.assets)?;
            valid_digest(asset.digest.as_deref()?)?;
            Some((release, asset, version))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.2.cmp(&left.2));

    let stable = candidates
        .iter()
        .find(|(release, _, version)| !release.prerelease && version > &current_version);
    let prerelease = candidates.iter().find(|(release, _, version)| {
        release.prerelease
            && current_version.pre != semver::Prerelease::EMPTY
            && version > &current_version
    });
    let selected = stable.or(prerelease);
    let newest_known = candidates.iter().max_by_key(|(_, _, version)| version);
    let (latest_version, download_url, notes, channel, update_available, message) =
        if let Some((release, asset, version)) = selected {
            let channel = if release.prerelease {
                "pre-release"
            } else {
                "stable"
            };
            (
                Some(version.to_string()),
                Some(asset.browser_download_url.clone()),
                release.body.clone(),
                channel.to_string(),
                true,
                format!(
                    "发现{} {}",
                    if release.prerelease {
                        "测试更新"
                    } else {
                        "正式更新"
                    },
                    version
                ),
            )
        } else if let Some((_, _, version)) = newest_known {
            let only_prerelease = candidates.iter().all(|(release, _, _)| release.prerelease);
            (
                Some(version.to_string()),
                None,
                None,
                if only_prerelease {
                    "pre-release"
                } else {
                    "stable"
                }
                .to_string(),
                false,
                if only_prerelease && current_version.pre == semver::Prerelease::EMPTY {
                    "当前没有稳定的正式更新".to_string()
                } else {
                    format!("当前已是 {current}")
                },
            )
        } else {
            (
                None,
                None,
                None,
                "stable".to_string(),
                false,
                format!("当前已是 {current}"),
            )
        };
    Ok(UpdateStatus {
        current_version: current.into(),
        latest_version,
        update_available,
        download_url,
        notes,
        channel,
        message,
    })
}

fn platform_asset(assets: &[Asset]) -> Option<&Asset> {
    let suffix = if cfg!(target_os = "macos") {
        "-macos-arm64.dmg"
    } else {
        "-windows-x64-setup.exe"
    };
    assets.iter().find(|asset| asset.name.ends_with(suffix))
}

fn trusted_release_url(url: &str) -> bool {
    let prefix = "https://github.com/SophiaData/AgentTerm/releases/download/";
    url.starts_with(prefix)
        && (url.ends_with("-windows-x64-setup.exe") || url.ends_with("-macos-arm64.dmg"))
}

/// Body that waits for the launching process to disappear, then runs the
/// installer. Inline via `-Command`, and the pid/path arrive through environment
/// variables so no quoting can break it. Nothing is written to disk.
#[cfg(windows)]
const INSTALLER_WATCH_SCRIPT: &str = "\
$pidToWait=[int]$env:AGENTTERM_INSTALL_PID\n\
$deadline=(Get-Date).AddSeconds(120)\n\
while((Get-Date) -lt $deadline){\n\
  if(-not (Get-Process -Id $pidToWait -ErrorAction SilentlyContinue)){break}\n\
  Start-Sleep -Milliseconds 200\n\
}\n\
if(Get-Process -Id $pidToWait -ErrorAction SilentlyContinue){exit 1}\n\
Start-Process -FilePath $env:AGENTTERM_INSTALL_PATH\n";

/// Start a detached helper that waits for this process to exit and only then
/// runs the installer. If shutdown is blocked the helper gives up, so an
/// installer never races a still-running application.
#[cfg(windows)]
fn spawn_installer_after_exit(installer: &PathBuf, pid: u32) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let powershell = std::env::var_os("SystemRoot")
        .map(|root| {
            PathBuf::from(root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.is_file())
        .ok_or_else(|| "找不到系统 PowerShell，已拒绝自动安装".to_string())?;
    Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
        ])
        .arg(INSTALLER_WATCH_SCRIPT)
        .env("AGENTTERM_INSTALL_PID", pid.to_string())
        .env("AGENTTERM_INSTALL_PATH", installer)
        .creation_flags(0x0800_0000)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法启动安装助手：{error}"))
}

#[cfg(not(windows))]
fn spawn_installer_after_exit(installer: &PathBuf, pid: u32) -> Result<(), String> {
    // Inline script via `sh -c`; no temp file and nothing left behind.
    let script = "for _ in $(seq 1 600); do kill -0 \"$1\" 2>/dev/null || break; \
sleep 0.2; done; kill -0 \"$1\" 2>/dev/null && exit 1; open \"$2\"";
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(pid.to_string())
        .arg(installer)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法启动安装助手：{error}"))
}

fn write_new(path: &PathBuf, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("无法保存更新：{error}"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("无法写入更新：{error}"))
}

fn unique_update_path(extension: &str) -> PathBuf {
    let seed = unique_nonce();
    std::env::temp_dir().join(format!(
        "AgentTerm-update-{}-{seed}.{extension}",
        std::process::id()
    ))
}

fn unique_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default()
}

fn verify_digest(bytes: &[u8], expected: &str) -> Result<String, String> {
    let expected = valid_digest(expected)
        .ok_or_else(|| "GitHub 更新 digest 格式无效，已拒绝安装".to_string())?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err("更新包 SHA-256 校验失败，已拒绝安装".into());
    }
    Ok(actual)
}

fn verify_prepared_file(path: &Path, expected_sha256: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| "已下载的更新包不存在，请重新下载".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("更新包文件无效，已拒绝安装".to_string());
    }
    let bytes = journal::read_bytes(path, MAX_INSTALLER_BYTES)
        .map_err(|_| "无法读取已下载的更新包，请重新下载".to_string())?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err("已下载的更新包校验失败，请重新下载".to_string());
    }
    Ok(())
}

fn valid_digest(value: &str) -> Option<&str> {
    let digest = value.strip_prefix("sha256:")?;
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(digest)
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let mut command = Command::new(curl_program()?);
    command.args([
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--tlsv1.2",
        "--connect-timeout",
        "15",
        "--max-time",
        "300",
        "--max-filesize",
        "209715200",
        url,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command
        .output()
        .map_err(|error| format!("无法访问 GitHub：{error}"))?;
    if !output.status.success() {
        return Err("检查更新失败，请稍后重试".into());
    }
    Ok(output.stdout)
}

fn curl_program() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let path = PathBuf::from(root).join("System32").join("curl.exe");
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    #[cfg(not(windows))]
    for path in ["/usr/bin/curl", "/bin/curl"] {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err("找不到可信的系统 curl，已拒绝更新请求".into())
}

async fn ensure_prepared_installer(
    update_mgr: &UpdateManager,
    download_url: String,
    latest_version: Option<String>,
) -> Result<PreparedInstaller, String> {
    if let Some(mut existing) = update_mgr.reusable_prepared(&download_url) {
        if existing.latest_version.is_none() {
            existing.latest_version = latest_version;
        }
        return Ok(existing);
    }
    let url_for_task = download_url.clone();
    let (installer_path, sha256) =
        tauri::async_runtime::spawn_blocking(move || prepare_installer(&url_for_task))
            .await
            .map_err(|error| format!("更新下载线程异常：{error}"))??;
    let prepared = PreparedInstaller {
        download_url,
        latest_version,
        installer_path,
        sha256,
    };
    update_mgr.store_prepared(prepared.clone());
    Ok(prepared)
}

#[tauri::command]
pub async fn check_for_update() -> Result<UpdateStatus, String> {
    tauri::async_runtime::spawn_blocking(check_latest)
        .await
        .map_err(|error| format!("更新检查线程异常：{error}"))?
}

#[tauri::command(async)]
pub fn get_update_activity(
    sessions: tauri::State<'_, SessionManager>,
    tasks: tauri::State<'_, TaskLedgerState>,
    scheduler: tauri::State<'_, Scheduler>,
) -> Result<UpdateActivity, String> {
    collect_update_activity(&sessions, &tasks, &scheduler)
}

#[tauri::command]
pub async fn download_update(
    update_mgr: tauri::State<'_, UpdateManager>,
    download_url: String,
    latest_version: Option<String>,
) -> Result<PreparedUpdateStatus, String> {
    let prepared = ensure_prepared_installer(&update_mgr, download_url, latest_version).await?;
    Ok(PreparedUpdateStatus {
        download_url: prepared.download_url,
        latest_version: prepared.latest_version,
        ready: true,
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn install_update(
    app: tauri::AppHandle,
    update_mgr: tauri::State<'_, UpdateManager>,
    sessions: tauri::State<'_, SessionManager>,
    tasks: tauri::State<'_, TaskLedgerState>,
    scheduler: tauri::State<'_, Scheduler>,
    download_url: String,
    latest_version: Option<String>,
    allow_stop_active: Option<bool>,
) -> Result<InstallUpdateOutcome, String> {
    let prepared =
        ensure_prepared_installer(&update_mgr, download_url, latest_version.clone()).await?;
    let activity = collect_update_activity(&sessions, &tasks, &scheduler)?;
    if activity.has_active_work && !allow_stop_active.unwrap_or(false) {
        return Ok(InstallUpdateOutcome::Blocked {
            activity,
            installer_ready: true,
        });
    }
    verify_prepared_file(&prepared.installer_path, &prepared.sha256)?;
    let recovery = build_recovery_snapshot(
        &activity,
        prepared.latest_version.clone().or(latest_version),
    );
    update_mgr.save_recovery(recovery)?;
    spawn_installer_after_exit(&prepared.installer_path, std::process::id())?;
    update_mgr.clear_prepared();
    // The detached helper waits for this process to disappear before launching
    // the installer, so a blocked shutdown simply makes the helper give up
    // instead of replacing files under a running application.
    app.exit(0);
    Ok(InstallUpdateOutcome::Installing {
        version: prepared.latest_version,
    })
}

#[tauri::command(async)]
pub fn get_update_recovery(
    update_mgr: tauri::State<'_, UpdateManager>,
) -> Result<Option<UpdateRecoverySnapshot>, String> {
    update_mgr.load_recovery()
}

#[tauri::command(async)]
pub fn consume_update_recovery_session(
    update_mgr: tauri::State<'_, UpdateManager>,
    session_id: String,
) -> Result<Option<UpdateRecoverySnapshot>, String> {
    update_mgr.consume_recovery_session(&session_id)
}

#[tauri::command(async)]
pub fn dismiss_update_recovery(update_mgr: tauri::State<'_, UpdateManager>) -> Result<(), String> {
    update_mgr.save_recovery(None)
}

#[cfg(test)]
mod tests {
    use super::{Asset, Release, verify_digest};
    use sha2::{Digest, Sha256};

    #[test]
    fn accepts_only_official_platform_packages() {
        assert!(super::trusted_release_url(
            "https://github.com/SophiaData/AgentTerm/releases/download/v0.2.0/AgentTerm-windows-x64-setup.exe"
        ));
        assert!(super::trusted_release_url(
            "https://github.com/SophiaData/AgentTerm/releases/download/v0.2.0/AgentTerm-macos-arm64.dmg"
        ));
        assert!(!super::trusted_release_url("https://example.com/setup.exe"));
    }

    #[test]
    fn digest_mismatch_is_rejected() {
        let bytes = b"official installer";
        let digest = format!("sha256:{:x}", Sha256::digest(bytes));
        assert!(verify_digest(bytes, &digest).is_ok());
        assert!(
            verify_digest(
                bytes,
                &format!("sha256:{}", digest[7..].to_ascii_uppercase())
            )
            .is_ok()
        );
        assert!(verify_digest(b"tampered", &digest).is_err());
        assert!(verify_digest(bytes, "sha256:not-hex").is_err());
    }

    fn release(version: &str, prerelease: bool) -> Release {
        let asset_name = if cfg!(target_os = "macos") {
            format!("AgentTerm-{version}-macos-arm64.dmg")
        } else {
            format!("AgentTerm-{version}-windows-x64-setup.exe")
        };
        Release {
            tag_name: format!("v{version}"),
            body: None,
            prerelease,
            assets: vec![Asset {
                name: asset_name,
                browser_download_url: format!(
                    "https://github.com/SophiaData/AgentTerm/releases/download/v{version}/test"
                ),
                digest: Some(format!("sha256:{}", "a".repeat(64))),
            }],
        }
    }

    #[test]
    fn prerelease_only_does_not_offer_itself_to_a_stable_build() {
        let status = super::select_update(
            &[release("0.2.0-rc.5", true), release("0.2.0-rc.3", true)],
            "0.1.0",
        )
        .unwrap();
        assert!(!status.update_available);
        assert_eq!(status.channel, "pre-release");
        assert_eq!(status.message, "当前没有稳定的正式更新");
    }

    #[test]
    fn stable_release_upgrades_a_prerelease() {
        let status = super::select_update(
            &[release("0.2.0", false), release("0.2.0-rc.5", true)],
            "0.2.0-rc.5",
        )
        .unwrap();
        assert!(status.update_available);
        assert_eq!(status.latest_version.as_deref(), Some("0.2.0"));
        assert_eq!(status.channel, "stable");
    }

    #[test]
    fn newer_prerelease_upgrades_an_older_prerelease() {
        let status = super::select_update(
            &[release("0.2.0-rc.6", true), release("0.2.0-rc.5", true)],
            "0.2.0-rc.5",
        )
        .unwrap();
        assert!(status.update_available);
        assert_eq!(status.latest_version.as_deref(), Some("0.2.0-rc.6"));
        assert_eq!(status.channel, "pre-release");
    }

    #[test]
    fn release_without_a_matching_platform_asset_or_digest_is_ignored() {
        let mut wrong_platform = release("0.2.0", false);
        wrong_platform.assets[0].name = "AgentTerm-0.2.0-linux-x64.tar.gz".into();
        let mut missing_digest = release("0.2.0", false);
        missing_digest.assets[0].digest = None;
        let status = super::select_update(
            &[wrong_platform, missing_digest, release("0.2.0-rc.6", true)],
            "0.2.0-rc.5",
        )
        .unwrap();
        assert!(status.update_available);
        assert_eq!(status.latest_version.as_deref(), Some("0.2.0-rc.6"));
        assert_eq!(status.channel, "pre-release");
    }

    #[test]
    fn installer_digest_requires_the_currently_offered_update_url() {
        let releases = vec![release("0.2.0-rc.10", true)];
        let url = releases[0].assets[0].browser_download_url.clone();
        let digest = super::installer_digest(&releases, "0.2.0-rc.9", &url).unwrap();
        assert!(digest.starts_with("sha256:"));
        // Same version is not an update, so nothing may be installed.
        assert!(super::installer_digest(&releases, "0.2.0-rc.10", &url).is_err());
        // A different asset of the same release is never accepted.
        let other =
            "https://github.com/SophiaData/AgentTerm/releases/download/v0.2.0-rc.10/other.exe";
        assert!(super::installer_digest(&releases, "0.2.0-rc.9", other).is_err());
        // An older release is not the offered update either.
        let older = vec![release("0.2.0-rc.8", true)];
        let older_url = older[0].assets[0].browser_download_url.clone();
        assert!(super::installer_digest(&older, "0.2.0-rc.9", &older_url).is_err());
    }

    fn temp_recovery_path(label: &str) -> std::path::PathBuf {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "agentterm-update-recovery-{label}-{}-{}-{}.json",
            std::process::id(),
            super::now_ms(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[test]
    fn recovery_roundtrip_consume_dismiss_and_tamper_protection() {
        let path = temp_recovery_path("roundtrip");
        let manager = super::UpdateManager::default();
        manager.configure_recovery(path.clone()).unwrap();
        assert_eq!(manager.load_recovery().unwrap(), None);

        let activity = super::UpdateActivity {
            live_sessions: vec![
                super::SessionInfo {
                    id: "s-1".into(),
                    tool_id: "claude".into(),
                    tool_name: "Claude Code".into(),
                    project_path: "E:\\AgentTerm".into(),
                    title: Some("修登录".into()),
                    cli_session_id: Some("01234567-89ab-4cde-8123-456789abcdef".into()),
                },
                super::SessionInfo {
                    id: "s-2".into(),
                    tool_id: "codex".into(),
                    tool_name: "Codex".into(),
                    project_path: "E:\\Other".into(),
                    title: None,
                    cli_session_id: None,
                },
            ],
            dispatch_active: true,
            scheduled_task_id: Some("sched-1".into()),
            has_active_work: true,
        };
        let snapshot =
            super::build_recovery_snapshot(&activity, Some("0.2.0-rc.15".into())).unwrap();
        manager.save_recovery(Some(snapshot)).unwrap();

        let reopened = super::UpdateManager::default();
        reopened.configure_recovery(path.clone()).unwrap();
        let loaded = reopened.load_recovery().unwrap().unwrap();
        assert_eq!(loaded.target_version.as_deref(), Some("0.2.0-rc.15"));
        assert_eq!(loaded.sessions.len(), 2);
        assert_eq!(
            loaded.sessions[0].cli_session_id.as_deref(),
            Some("01234567-89ab-4cde-8123-456789abcdef")
        );
        assert!(loaded.had_active_dispatch);
        assert_eq!(
            loaded.interrupted_scheduled_task_id.as_deref(),
            Some("sched-1")
        );

        let after_consume = reopened.consume_recovery_session("s-1").unwrap().unwrap();
        assert_eq!(after_consume.sessions.len(), 1);
        assert_eq!(after_consume.sessions[0].id, "s-2");

        // Tampered file must fail closed without clearing the file.
        let raw = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, raw.replace("sched-1", "sched-2")).unwrap();
        assert!(reopened.load_recovery().is_err());

        // Restoring valid state and dismissing clears payload cleanly.
        reopened.save_recovery(None).unwrap();
        assert_eq!(reopened.load_recovery().unwrap(), None);
    }

    #[test]
    fn prepared_installer_reuse_detects_missing_or_tampered_file() {
        let installer_path = temp_recovery_path("installer").with_extension("exe");
        let content = b"authentic-installer-bytes";
        std::fs::write(&installer_path, content).unwrap();
        let digest = format!("{:x}", Sha256::digest(content));
        assert!(super::verify_prepared_file(&installer_path, &digest).is_ok());

        let manager = super::UpdateManager::default();
        let url = "https://github.com/SophiaData/AgentTerm/releases/download/v0.2.0/AgentTerm-0.2.0-windows-x64-setup.exe";
        manager.store_prepared(super::PreparedInstaller {
            download_url: url.into(),
            latest_version: Some("0.2.0".into()),
            installer_path: installer_path.clone(),
            sha256: digest,
        });
        assert!(manager.reusable_prepared(url).is_some());

        std::fs::write(&installer_path, b"tampered").unwrap();
        assert!(manager.reusable_prepared(url).is_none());
        let _ = std::fs::remove_file(&installer_path);
    }

    #[test]
    fn empty_activity_produces_no_recovery_snapshot() {
        let idle = super::UpdateActivity {
            live_sessions: Vec::new(),
            dispatch_active: false,
            scheduled_task_id: None,
            has_active_work: false,
        };
        assert!(super::build_recovery_snapshot(&idle, Some("0.2.0".into())).is_none());
    }
}
