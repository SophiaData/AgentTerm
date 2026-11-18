//! Launch one allowlisted CLI in an independent native terminal window.
//! The frontend only supplies a tool ID and an existing registered project path.

#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

use serde::Serialize;

use crate::projects::ProjectStore;
use crate::tools::{ResolveResult, TOOL_DEFINITIONS, candidate_search_dirs, resolve_binary};

#[derive(Debug, PartialEq, Eq)]
struct LaunchSpec {
    binary: PathBuf,
    directory: PathBuf,
}

fn prepare_launch(
    projects: &ProjectStore,
    tool_id: &str,
    project_path: &str,
    search_dirs: &[PathBuf],
) -> Result<LaunchSpec, String> {
    let definition = TOOL_DEFINITIONS
        .iter()
        .find(|definition| definition.id == tool_id)
        .ok_or_else(|| format!("不支持的工具：{tool_id}"))?;

    // Canonicalize and require membership BEFORE touching any process launcher.
    // The persistent list has no disabled flag: only listed projects are enabled.
    let directory = projects.require_enabled_project(project_path)?;
    let binary = match resolve_binary(definition.binaries, search_dirs) {
        ResolveResult::Found(binary) => binary,
        ResolveResult::NotExecutable(path) => {
            return Err(format!(
                "{} 已找到但没有执行权限：{}",
                definition.name,
                path.display()
            ));
        }
        ResolveResult::Missing => {
            return Err(format!("未安装 {}，无法启动", definition.name));
        }
    };
    let binary =
        std::fs::canonicalize(binary).map_err(|error| format!("无法读取工具路径：{error}"))?;
    Ok(LaunchSpec { binary, directory })
}

#[cfg(target_os = "macos")]
const TERMINAL_APPLESCRIPT: &str = r#"on run argv
    set cliPath to item 1 of argv
    set projectDir to item 2 of argv
    tell application "Terminal"
        do script ("cd " & quoted form of projectDir & " && exec " & quoted form of cliPath)
        activate
    end tell
end run"#;

#[cfg(target_os = "macos")]
fn open_native_terminal(spec: &LaunchSpec) -> Result<(), String> {
    // The AppleScript program is constant. User-controlled paths are separate
    // argv values and AppleScript's quoted form safely escapes shell metacharacters.
    // osascript exits after asking Terminal.app to launch, NOT when the CLI exits.
    let result = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(TERMINAL_APPLESCRIPT)
        .arg(&spec.binary)
        .arg(&spec.directory)
        .status()
        .map_err(|error| format!("无法调用 macOS Terminal.app：{error}"))?;
    if result.success() {
        Ok(())
    } else {
        Err(format!("Terminal.app 启动失败：{result}"))
    }
}

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

#[cfg(windows)]
fn is_batch_shim(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
}

#[cfg(windows)]
fn batch_command(path: &Path) -> Result<String, String> {
    let binary = path.to_str().ok_or("Windows 工具路径不是有效 Unicode")?;
    // cmd.exe expands %VAR% even inside double quotes; reject all shell syntax
    // rather than try to encode attacker-controlled paths into a command line.
    if binary
        .chars()
        .any(|character| ['"', '&', '|', '<', '>', '^', '%', '!', '\r', '\n'].contains(&character))
    {
        return Err("工具路径包含 Windows 命令解释器特殊字符，拒绝启动".to_string());
    }
    Ok(format!("call \"{binary}\""))
}

#[cfg(windows)]
fn open_native_terminal(spec: &LaunchSpec) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let mut command = if is_batch_shim(&spec.binary) {
        // Spawn a known system cmd.exe only for .cmd/.bat shims. The project
        // directory is set by CreateProcess, never interpolated into /K text.
        let root = std::env::var_os("SystemRoot").ok_or("无法定位 Windows 系统目录")?;
        let cmd = PathBuf::from(root).join("System32").join("cmd.exe");
        if !cmd.is_file() {
            return Err("无法找到 Windows 命令解释器".to_string());
        }
        let mut command = Command::new(cmd);
        command
            .args(["/D", "/V:OFF", "/K"])
            .arg(batch_command(&spec.binary)?);
        command
    } else {
        // Do not execute other script extensions (e.g. PowerShell shims) by
        // file association. Only native binaries may bypass cmd.exe.
        let extension = spec
            .binary
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if !extension.eq_ignore_ascii_case("exe") && !extension.eq_ignore_ascii_case("com") {
            return Err("Windows 仅支持 .exe/.com 与 .cmd/.bat 工具".to_string());
        }
        Command::new(&spec.binary)
    };
    command
        .current_dir(&spec.directory)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map_err(|error| format!("无法打开 Windows 原生终端：{error}"))?;
    // Dropping Child does not kill or wait for the independently running CLI.
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn open_native_terminal(_spec: &LaunchSpec) -> Result<(), String> {
    Err("当前平台暂不支持原生终端启动".to_string())
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeLaunchResult {
    pub launched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Called only after the OS has accepted the terminal launch. A failure to
/// remember history must not misreport the already-open CLI as launch failure.
fn record_after_launch(
    projects: &ProjectStore,
    spec: &LaunchSpec,
    tool_id: &str,
) -> NativeLaunchResult {
    let warning = projects
        .remember_last_tool(&spec.directory.display().to_string(), tool_id)
        .err()
        .map(|error| format!("终端已打开，但未能记录最近使用的工具：{error}"));
    NativeLaunchResult {
        launched: true,
        warning,
    }
}

#[tauri::command]
pub fn launch_native_tool(
    projects: tauri::State<'_, ProjectStore>,
    tool_id: String,
    project_path: String,
) -> Result<NativeLaunchResult, String> {
    // Tauri maps snake_case Rust parameters to the requested
    // invoke("launch_native_tool", { toolId, projectPath }) JS object.
    let spec = prepare_launch(&projects, &tool_id, &project_path, &candidate_search_dirs())?;
    open_native_terminal(&spec)?;
    Ok(record_after_launch(&projects, &spec, &tool_id))
}

#[cfg(test)]
mod tests {
    use super::{LaunchSpec, prepare_launch};
    use crate::projects::ProjectStore;
    use std::path::{Path, PathBuf};

    struct TestPaths(PathBuf);

    impl TestPaths {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "agentterm-launcher-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn store(&self) -> ProjectStore {
            ProjectStore::with_path(self.path().join("config/agentterm/projects.json"))
        }

        fn project(&self) -> PathBuf {
            let project = self.path().join("my project");
            std::fs::create_dir_all(&project).unwrap();
            project
        }

        fn tool(&self, name: &str) -> PathBuf {
            let bin_dir = self.path().join("bin");
            std::fs::create_dir_all(&bin_dir).unwrap();
            let binary = bin_dir.join(name);
            std::fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            binary
        }
    }

    impl Drop for TestPaths {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unknown_tool_is_rejected_before_binary_lookup() {
        let tmp = TestPaths::new("tool-id");
        let store = tmp.store();
        let project = tmp.project();
        store.add(&project.display().to_string()).unwrap();
        let error = prepare_launch(
            &store,
            "claude;touch /tmp/invalid",
            &project.display().to_string(),
            &[tmp.path().to_path_buf()],
        )
        .unwrap_err();
        assert!(error.contains("不支持的工具"));
    }

    #[test]
    fn only_registered_canonical_project_can_launch() {
        let tmp = TestPaths::new("projects");
        let store = tmp.store();
        let project = tmp.project();
        let binary = tmp.tool("codex");
        let search_dirs = vec![binary.parent().unwrap().to_path_buf()];

        assert!(
            prepare_launch(
                &store,
                "codex",
                &project.display().to_string(),
                &search_dirs
            )
            .is_err()
        );
        let registered = store.add(&project.display().to_string()).unwrap();
        let spec = prepare_launch(
            &store,
            "codex",
            &project.join("..").join("my project").display().to_string(),
            &search_dirs,
        )
        .unwrap();
        assert_eq!(
            spec,
            LaunchSpec {
                binary: std::fs::canonicalize(binary).unwrap(),
                directory: std::fs::canonicalize(&project).unwrap()
            }
        );
        store.remove(&registered.id).unwrap();
        assert!(
            prepare_launch(
                &store,
                "codex",
                &project.display().to_string(),
                &search_dirs
            )
            .is_err()
        );
    }

    #[test]
    fn missing_and_non_executable_tools_are_rejected() {
        let tmp = TestPaths::new("missing");
        let store = tmp.store();
        let project = tmp.project();
        store.add(&project.display().to_string()).unwrap();

        assert!(
            prepare_launch(
                &store,
                "agy",
                &project.display().to_string(),
                &[tmp.path().to_path_buf()]
            )
            .unwrap_err()
            .contains("未安装")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = tmp.tool("agy");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(
                prepare_launch(
                    &store,
                    "agy",
                    &project.display().to_string(),
                    &[script.parent().unwrap().to_path_buf()]
                )
                .unwrap_err()
                .contains("没有执行权限")
            );
        }
    }

    #[test]
    fn completed_launch_reports_history_error_as_warning_not_failure() {
        let tmp = TestPaths::new("history-warning");
        let store = tmp.store();
        let project = tmp.project();
        let registered = store.add(&project.display().to_string()).unwrap();
        let binary = tmp.tool("codex");
        let spec = prepare_launch(
            &store,
            "codex",
            &project.display().to_string(),
            &[binary.parent().unwrap().to_path_buf()],
        )
        .unwrap();

        let success = super::record_after_launch(&store, &spec, "codex");
        assert!(success.launched);
        assert!(success.warning.is_none());

        // Simulate another client removing the project after the OS launched.
        store.remove(&registered.id).unwrap();
        let partial = super::record_after_launch(&store, &spec, "codex");
        assert!(partial.launched);
        assert!(partial.warning.unwrap().contains("未能记录最近使用的工具"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn applescript_escapes_paths_at_runtime_instead_of_interpolating() {
        assert!(super::TERMINAL_APPLESCRIPT.contains("quoted form of projectDir"));
        assert!(super::TERMINAL_APPLESCRIPT.contains("quoted form of cliPath"));
        assert!(!super::TERMINAL_APPLESCRIPT.contains("do shell script"));
        assert!(super::TERMINAL_APPLESCRIPT.contains("do script"));
    }

    #[cfg(windows)]
    #[test]
    fn cmd_shims_are_supported_without_shell_injection() {
        assert!(super::is_batch_shim(Path::new(r"C:\bin\codex.CMD")));
        assert!(!super::is_batch_shim(Path::new(r"C:\bin\codex.exe")));
        assert!(super::batch_command(Path::new(r"C:\Program Files\codex.cmd")).is_ok());
        for attack in [
            r"C:\bad&run\codex.cmd",
            r"C:\%USERPROFILE%\codex.cmd",
            "C:\\bad\ncmd.cmd",
        ] {
            assert!(super::batch_command(Path::new(attack)).is_err());
        }
    }
}
