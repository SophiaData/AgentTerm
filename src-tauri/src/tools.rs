use serde::Serialize;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const VERSION_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_VERSION_OUTPUT: usize = 500;
const MAX_NVM_VERSIONS: usize = 16;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const WINDOWS_NATIVE_EXTENSIONS: &[&str] = &["exe", "com"];
const WINDOWS_BATCH_EXTENSIONS: &[&str] = &["cmd", "bat"];

pub struct ToolDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub binaries: &'static [&'static str],
    pub version_args: &'static [&'static str],
}

pub const TOOL_DEFINITIONS: &[ToolDefinition] = &[
    ToolDefinition {
        id: "claude",
        name: "Claude Code",
        binaries: &["claude"],
        version_args: &["--version"],
    },
    ToolDefinition {
        id: "codex",
        name: "Codex",
        binaries: &["codex"],
        version_args: &["--version"],
    },
    ToolDefinition {
        id: "agy",
        name: "Agy",
        binaries: &["agy"],
        version_args: &["--version"],
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub state: ToolState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolState {
    Installed {
        path: String,
        version: Option<String>,
    },
    Missing,
    NotExecutable {
        reason: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResolveResult {
    Found(PathBuf),
    NotExecutable(PathBuf),
    Missing,
}

pub fn resolve_binary(binaries: &[&str], dirs: &[PathBuf]) -> ResolveResult {
    let mut blocked: Option<PathBuf> = None;

    for dir in dirs {
        for binary in binaries {
            for file_name in candidate_file_names(binary) {
                let candidate = dir.join(file_name);
                if !candidate.is_file() {
                    continue;
                }
                if is_executable(&candidate) {
                    return ResolveResult::Found(candidate);
                }
                if blocked.is_none() {
                    blocked = Some(candidate);
                }
            }
        }
    }

    match blocked {
        Some(path) => ResolveResult::NotExecutable(path),
        None => ResolveResult::Missing,
    }
}

fn candidate_file_names(binary: &str) -> Vec<String> {
    #[cfg(not(windows))]
    {
        vec![binary.to_string()]
    }

    #[cfg(windows)]
    {
        // Never select extensionless POSIX shell shims (e.g. npm `codex`) or `.ps1`
        // files on Windows; accept an explicit `.exe` name, and otherwise prefer
        // native executables over `.cmd`/`.bat` wrappers.
        let mut names = Vec::new();
        if Path::new(binary)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                WINDOWS_NATIVE_EXTENSIONS
                    .iter()
                    .chain(WINDOWS_BATCH_EXTENSIONS.iter())
                    .any(|allowed| ext.eq_ignore_ascii_case(allowed))
            })
        {
            names.push(binary.to_string());
        }
        let mut native = Vec::new();
        let mut batch = Vec::new();
        let mut seen = Vec::new();
        let extensions = env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.COM;.CMD;.BAT".to_string());
        for extension in extensions
            .split(';')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            let lower = extension.trim_start_matches('.').to_ascii_lowercase();
            if seen.contains(&lower) {
                continue;
            }
            if WINDOWS_NATIVE_EXTENSIONS.contains(&lower.as_str()) {
                seen.push(lower.clone());
                native.push(format!("{binary}.{lower}"));
            } else if WINDOWS_BATCH_EXTENSIONS.contains(&lower.as_str()) {
                seen.push(lower.clone());
                batch.push(format!("{binary}.{lower}"));
            }
        }
        for fallback in ["exe", "com", "cmd", "bat"] {
            if !seen.contains(&fallback.to_string()) {
                if WINDOWS_NATIVE_EXTENSIONS.contains(&fallback) {
                    native.push(format!("{binary}.{fallback}"));
                } else {
                    batch.push(format!("{binary}.{fallback}"));
                }
            }
        }

        names.extend(native);
        names.extend(batch);
        names
    }
}

pub fn is_windows_batch(path: &Path) -> bool {
    cfg!(windows)
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                WINDOWS_BATCH_EXTENSIONS
                    .iter()
                    .any(|item| ext.eq_ignore_ascii_case(item))
            })
}

pub fn windows_cmd_executable() -> PathBuf {
    env::var_os("ComSpec")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cmd.exe"))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                WINDOWS_NATIVE_EXTENSIONS
                    .iter()
                    .chain(WINDOWS_BATCH_EXTENSIONS.iter())
                    .any(|allowed| ext.eq_ignore_ascii_case(allowed))
            })
}

pub fn candidate_search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(path) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&path));
    }
    dirs.extend(platform_extra_dirs());

    deduplicate(dirs)
}

fn platform_extra_dirs() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = Vec::new();

    if cfg!(windows) {
        if let Some(app_data) = env::var_os("APPDATA") {
            dirs.push(PathBuf::from(app_data).join("npm"));
        }
        if let Some(local_data) = env::var_os("LOCALAPPDATA") {
            dirs.extend(windows_local_bin_dirs(Path::new(&local_data)));
        }
        if let Some(home) = &home {
            dirs.push(home.join(".bun").join("bin"));
        }
    } else {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        if let Some(home) = &home {
            dirs.push(home.join(".local/bin"));
            dirs.push(home.join(".npm-global/bin"));
            dirs.push(home.join(".bun/bin"));
            dirs.push(home.join(".volta/bin"));
            dirs.push(home.join(".cargo/bin"));
            dirs.push(home.join(".local/share/pnpm"));
            dirs.extend(nvm_bin_dirs(home));
        }
    }

    dirs
}

fn windows_local_bin_dirs(local_data: &Path) -> [PathBuf; 2] {
    [local_data.join("pnpm"), local_data.join("agy").join("bin")]
}

fn nvm_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let root = home.join(".nvm/versions/node");
    let mut found: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("bin"))
        .filter(|path| path.is_dir())
        .take(MAX_NVM_VERSIONS)
        .collect();

    found.sort();
    found.reverse();
    found
}

fn deduplicate(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique: Vec<PathBuf> = Vec::new();

    for dir in dirs {
        if dir.as_os_str().is_empty() || unique.contains(&dir) {
            continue;
        }
        unique.push(dir);
    }

    unique
}

pub fn parse_version(output: &str) -> Option<String> {
    let separators = |character: char| {
        !(character.is_ascii_alphanumeric()
            || character == '.'
            || character == '-'
            || character == '+')
    };

    for token in output.split(separators) {
        let trimmed = token.trim_start_matches('v');
        if trimmed.is_empty() {
            continue;
        }
        if let Some(version) = normalize_version(trimmed) {
            return Some(version);
        }
    }

    None
}

fn normalize_version(token: &str) -> Option<String> {
    let core_end = token.find(['-', '+']).unwrap_or(token.len());
    let core = &token[..core_end];
    let parts: Vec<&str> = core.split('.').collect();

    if parts.len() < 2 {
        return None;
    }
    if !parts
        .iter()
        .all(|part| !part.is_empty() && part.chars().all(|character| character.is_ascii_digit()))
    {
        return None;
    }

    Some(token.to_string())
}

fn read_version(path: &Path, args: &[&str]) -> Option<String> {
    let mut command = if is_windows_batch(path) {
        let mut wrapped = Command::new(windows_cmd_executable());
        wrapped.arg("/D").arg("/C").arg(path).args(args);
        wrapped
    } else {
        let mut direct = Command::new(path);
        direct.args(args);
        direct
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                return None;
            }
        }
    }

    let output = child.wait_with_output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();

    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&output.stderr).into_owned();
    }

    let truncated: String = text.chars().take(MAX_VERSION_OUTPUT).collect();
    parse_version(&truncated)
}

pub fn detect_tool(definition: &ToolDefinition, dirs: &[PathBuf]) -> ToolStatus {
    let state = match resolve_binary(definition.binaries, dirs) {
        ResolveResult::Found(path) => {
            let version = read_version(&path, definition.version_args);
            ToolState::Installed {
                path: path.display().to_string(),
                version,
            }
        }
        ResolveResult::NotExecutable(path) => ToolState::NotExecutable {
            reason: format!("已找到 {}，但没有执行权限", path.display()),
        },
        ResolveResult::Missing => ToolState::Missing,
    };

    ToolStatus {
        id: definition.id,
        name: definition.name,
        state,
    }
}

pub fn detect_all(dirs: &[PathBuf]) -> Vec<ToolStatus> {
    TOOL_DEFINITIONS
        .iter()
        .map(|definition| detect_tool(definition, dirs))
        .collect()
}

#[tauri::command]
pub fn detect_tools() -> Vec<ToolStatus> {
    detect_all(&candidate_search_dirs())
}

#[cfg(test)]
mod tests {
    use super::{
        ResolveResult, TOOL_DEFINITIONS, ToolState, detect_all, detect_tool, parse_version,
        resolve_binary,
    };
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let unique = format!(
                "agentterm-tools-{label}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_nanos())
                    .unwrap_or_default()
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("temp directory should be created");
            Self(path)
        }

        fn path(&self) -> PathBuf {
            self.0.clone()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn write_script(dir: &TempDir, name: &str, body: &str, executable: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.path().join(name);
        std::fs::write(&path, body).expect("script should be written");
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("permissions should be set");
        path
    }

    #[test]
    fn registry_contains_the_three_supported_tools() {
        let ids: Vec<&str> = TOOL_DEFINITIONS
            .iter()
            .map(|definition| definition.id)
            .collect();
        assert_eq!(ids, vec!["claude", "codex", "agy"]);
    }

    #[test]
    fn parse_version_handles_common_cli_output() {
        assert_eq!(
            parse_version("2.0.14 (Claude Code)"),
            Some("2.0.14".to_string())
        );
        assert_eq!(
            parse_version("codex-cli 0.20.0"),
            Some("0.20.0".to_string())
        );
        assert_eq!(parse_version("agy v1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(
            parse_version("0.1.0-beta.2\n"),
            Some("0.1.0-beta.2".to_string())
        );
    }

    #[test]
    fn parse_version_returns_none_without_a_version_number() {
        assert_eq!(parse_version("unknown"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("build 2024"), None);
    }

    #[test]
    fn missing_binary_is_reported_as_missing() {
        let dir = TempDir::new("missing");

        assert_eq!(
            resolve_binary(&["claude"], &[dir.path()]),
            ResolveResult::Missing
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_binary_is_found_and_version_is_read() {
        let dir = TempDir::new("installed");
        let script = write_script(&dir, "codex", "#!/bin/sh\necho 'codex-cli 0.20.0'\n", true);

        assert_eq!(
            resolve_binary(&["codex"], &[dir.path()]),
            ResolveResult::Found(script.clone())
        );

        let definition = TOOL_DEFINITIONS
            .iter()
            .find(|item| item.id == "codex")
            .expect("codex definition should exist");
        let status = detect_tool(definition, &[dir.path()]);

        assert_eq!(
            status.state,
            ToolState::Installed {
                path: script.display().to_string(),
                version: Some("0.20.0".to_string()),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_binary_is_reported_without_running_it() {
        let dir = TempDir::new("blocked");
        let script = write_script(&dir, "agy", "#!/bin/sh\necho '1.0.0'\n", false);

        assert_eq!(
            resolve_binary(&["agy"], &[dir.path()]),
            ResolveResult::NotExecutable(script.clone())
        );

        let definition = TOOL_DEFINITIONS
            .iter()
            .find(|item| item.id == "agy")
            .expect("agy definition should exist");
        let status = detect_tool(definition, &[dir.path()]);

        match status.state {
            ToolState::NotExecutable { reason } => {
                assert!(reason.contains("没有执行权限"));
                assert!(reason.contains("agy"));
            }
            other => panic!("expected not executable, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn hang_version_command_does_not_block_detection() {
        let dir = TempDir::new("slow");
        write_script(&dir, "claude", "#!/bin/sh\nsleep 30\n", true);

        let definition = TOOL_DEFINITIONS
            .iter()
            .find(|item| item.id == "claude")
            .expect("claude definition should exist");
        let started = std::time::Instant::now();
        let status = detect_tool(definition, &[dir.path()]);

        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        match status.state {
            ToolState::Installed { version, .. } => assert_eq!(version, None),
            other => panic!("expected installed without version, got {other:?}"),
        }
    }

    #[test]
    fn detect_all_reports_every_registered_tool() {
        let dir = TempDir::new("all");
        let statuses = detect_all(&[dir.path()]);

        assert_eq!(statuses.len(), TOOL_DEFINITIONS.len());
        assert!(
            statuses
                .iter()
                .all(|status| status.state == ToolState::Missing)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_finds_agy_in_default_directory_without_a_path_entry() {
        let dir = TempDir::new("agy-default");
        let local_data = dir.path().join("Local App Data");
        let agy_dir = local_data.join("agy").join("bin");
        let dirs = super::windows_local_bin_dirs(&local_data);
        let agy = TOOL_DEFINITIONS
            .iter()
            .find(|tool| tool.id == "agy")
            .unwrap();
        assert_eq!(detect_tool(agy, &dirs).state, ToolState::Missing);

        std::fs::create_dir_all(&agy_dir).unwrap();
        let binary = agy_dir.join("agy.cmd");
        std::fs::write(&binary, "@echo off\r\necho 1.5.15\r\n").unwrap();
        assert_eq!(
            detect_tool(agy, &dirs).state,
            ToolState::Installed {
                path: binary.display().to_string(),
                version: Some("1.5.15".into()),
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_explicit_path_keeps_priority_over_default_agy_installation() {
        let dir = TempDir::new("agy-path-priority");
        let custom = dir.path().join("custom");
        let local_data = dir.path().join("local");
        let agy_dir = local_data.join("agy").join("bin");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::create_dir_all(&agy_dir).unwrap();
        let custom_binary = custom.join("agy.cmd");
        std::fs::write(&custom_binary, "@echo off\r\necho 2.0.0\r\n").unwrap();
        std::fs::write(agy_dir.join("agy.cmd"), "@echo off\r\necho 1.5.15\r\n").unwrap();
        let mut dirs = vec![custom];
        dirs.extend(super::windows_local_bin_dirs(&local_data));
        let agy = TOOL_DEFINITIONS
            .iter()
            .find(|tool| tool.id == "agy")
            .unwrap();
        assert_eq!(
            detect_tool(agy, &dirs).state,
            ToolState::Installed {
                path: custom_binary.display().to_string(),
                version: Some("2.0.0".into()),
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_skips_extensionless_and_ps1_shims_and_prefers_exe_over_cmd() {
        let dir = TempDir::new("win-shim");
        std::fs::write(dir.path().join("codex"), "#!/bin/sh\necho 0.0.1\n").unwrap();
        std::fs::write(dir.path().join("codex.ps1"), "Write-Output 0.0.1\n").unwrap();
        assert_eq!(
            resolve_binary(&["codex"], &[dir.path()]),
            ResolveResult::Missing
        );

        let cmd_path = dir.path().join("codex.cmd");
        std::fs::write(&cmd_path, "@echo off\r\necho codex-cli 0.169.0\r\n").unwrap();
        assert_eq!(
            resolve_binary(&["codex"], &[dir.path()]),
            ResolveResult::Found(cmd_path.clone())
        );
        let codex_def = TOOL_DEFINITIONS
            .iter()
            .find(|item| item.id == "codex")
            .expect("codex definition should exist");
        let status = detect_tool(codex_def, &[dir.path()]);
        assert_eq!(
            status.state,
            ToolState::Installed {
                path: cmd_path.display().to_string(),
                version: Some("0.169.0".to_string()),
            }
        );

        let exe_path = dir.path().join("codex.exe");
        std::fs::write(&exe_path, b"MZ").unwrap();
        assert_eq!(
            resolve_binary(&["codex"], &[dir.path()]),
            ResolveResult::Found(exe_path)
        );
    }
}
