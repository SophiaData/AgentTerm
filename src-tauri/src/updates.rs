use serde::Serialize;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASE_API: &str = "https://api.github.com/repos/SophiaData/AgentTerm/releases/latest";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub download_url: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct Release {
    tag_name: String,
    body: Option<String>,
    assets: Vec<Asset>,
}

#[derive(Debug, serde::Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub fn check_latest() -> Result<UpdateStatus, String> {
    let release: Release = serde_json::from_slice(&http_get(RELEASE_API)?)
        .map_err(|error| format!("无法读取版本信息：{error}"))?;
    let latest = release.tag_name.trim_start_matches('v').to_string();
    let download_url = release
        .assets
        .iter()
        .find(|asset| asset.name.ends_with("-windows-x64-setup.exe"))
        .map(|asset| asset.browser_download_url.clone());
    Ok(UpdateStatus {
        current_version: CURRENT_VERSION.into(),
        update_available: newer(&latest, CURRENT_VERSION) && download_url.is_some(),
        latest_version: Some(latest),
        download_url,
        notes: release.body,
    })
}

pub fn download_and_launch(url: &str) -> Result<(), String> {
    if !url.starts_with("https://github.com/SophiaData/AgentTerm/releases/download/")
        || !url.ends_with("-windows-x64-setup.exe")
    {
        return Err("更新地址不在 AgentTerm 官方发布页".into());
    }
    let bytes = http_get(url)?;
    let path = std::env::temp_dir().join(format!("AgentTerm-update-{CURRENT_VERSION}.exe"));
    write_new(&path, &bytes)?;
    Command::new(&path)
        .spawn()
        .map_err(|error| format!("无法启动安装程序：{error}"))?;
    Ok(())
}

fn write_new(path: &PathBuf, bytes: &[u8]) -> Result<(), String> {
    let mut file = File::create(path).map_err(|error| format!("无法保存更新：{error}"))?;
    file.write_all(bytes)
        .map_err(|error| format!("无法写入更新：{error}"))
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let mut command = Command::new("curl");
    command.args(["--fail", "--location", "--silent", "--show-error", url]);
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

fn newer(latest: &str, current: &str) -> bool {
    fn parts(value: &str) -> Vec<u32> {
        value
            .split(['.', '-'])
            .filter_map(|part| part.parse().ok())
            .collect()
    }
    let left = parts(latest);
    let right = parts(current);
    left != right && left.iter().ne(right.iter()) && left > right
}

#[tauri::command]
pub fn check_for_update() -> Result<UpdateStatus, String> {
    check_latest()
}

#[tauri::command]
pub fn install_update(download_url: String) -> Result<(), String> {
    download_and_launch(&download_url)
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn compares_release_numbers() {
        assert!(newer("0.2.0-rc.4", "0.2.0-rc.3"));
        assert!(!newer("0.2.0-rc.3", "0.2.0-rc.3"));
        assert!(!newer("0.2.0-rc.2", "0.2.0-rc.3"));
    }
}
