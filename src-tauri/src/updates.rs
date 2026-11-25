use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASE_API: &str = "https://api.github.com/repos/SophiaData/AgentTerm/releases?per_page=100";

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
pub fn prepare_installer(url: &str) -> Result<PathBuf, String> {
    if !trusted_release_url(url) {
        return Err("更新地址不在 AgentTerm 官方发布页".into());
    }
    let releases: Vec<Release> = serde_json::from_slice(&http_get(RELEASE_API)?)
        .map_err(|error| format!("无法重新校验更新信息：{error}"))?;
    let digest = installer_digest(&releases, CURRENT_VERSION, url)?;
    let bytes = http_get(url)?;
    verify_digest(&bytes, &digest)?;
    let extension = if cfg!(target_os = "macos") {
        "dmg"
    } else {
        "exe"
    };
    let path = unique_update_path(extension);
    write_new(&path, &bytes)?;
    Ok(path)
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

/// Start a detached helper that waits for this process to exit and only then
/// runs the installer. If shutdown is blocked the helper gives up, so an
/// installer never races a still-running application.
#[cfg(windows)]
fn spawn_installer_after_exit(installer: &PathBuf, pid: u32) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let script =
        std::env::temp_dir().join(format!("agentterm-install-{pid}-{}.ps1", unique_nonce()));
    let body = "param([int]$TargetPid,[string]$Installer)\n\
$deadline = (Get-Date).AddSeconds(120)\n\
while ((Get-Date) -lt $deadline) {\n\
  if (-not (Get-Process -Id $TargetPid -ErrorAction SilentlyContinue)) { break }\n\
  Start-Sleep -Milliseconds 200\n\
}\n\
if (Get-Process -Id $TargetPid -ErrorAction SilentlyContinue) { exit 1 }\n\
Start-Process -FilePath $Installer\n\
exit 0\n";
    fs::write(&script, body).map_err(|error| format!("无法准备安装脚本：{error}"))?;
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(&script)
        .arg("-TargetPid")
        .arg(pid.to_string())
        .arg("-Installer")
        .arg(installer)
        .creation_flags(0x0800_0000);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法启动安装助手：{error}"))
}

#[cfg(not(windows))]
fn spawn_installer_after_exit(installer: &PathBuf, pid: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let script =
        std::env::temp_dir().join(format!("agentterm-install-{pid}-{}.sh", unique_nonce()));
    let body = "#!/bin/sh\n\
for _ in $(seq 1 600); do\n\
  kill -0 \"$1\" 2>/dev/null || break\n\
  sleep 0.2\n\
done\n\
kill -0 \"$1\" 2>/dev/null && exit 1\n\
open \"$2\"\n";
    fs::write(&script, body).map_err(|error| format!("无法准备安装脚本：{error}"))?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("无法保护安装脚本：{error}"))?;
    Command::new("/bin/sh")
        .arg(&script)
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

fn verify_digest(bytes: &[u8], expected: &str) -> Result<(), String> {
    let expected = valid_digest(expected)
        .ok_or_else(|| "GitHub 更新 digest 格式无效，已拒绝安装".to_string())?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err("更新包 SHA-256 校验失败，已拒绝安装".into());
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

#[tauri::command]
pub async fn check_for_update() -> Result<UpdateStatus, String> {
    tauri::async_runtime::spawn_blocking(check_latest)
        .await
        .map_err(|error| format!("更新检查线程异常：{error}"))?
}

#[tauri::command]
pub async fn install_update(app: tauri::AppHandle, download_url: String) -> Result<(), String> {
    let installer = tauri::async_runtime::spawn_blocking(move || prepare_installer(&download_url))
        .await
        .map_err(|error| format!("更新安装线程异常：{error}"))??;
    spawn_installer_after_exit(&installer, std::process::id())?;
    // The detached helper waits for this process to disappear before launching
    // the installer, so a blocked shutdown simply makes the helper give up
    // instead of replacing files under a running application.
    app.exit(0);
    Ok(())
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
}
