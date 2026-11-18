//! Tool discovery: find the AI CLIs on this machine and read their versions.
//!
//! GUI apps launched from Finder do not inherit the shell's PATH, so we look in
//! the usual bin directories (Homebrew, nvm, pnpm, cargo, ...) in addition to
//! `$PATH`. This mirrors what the desktop build learned the hard way.

use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const VERSION_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub id: &'static str,
    pub name: &'static str,
    pub binary: &'static str,
    pub version_args: &'static [&'static str],
    pub path: Option<PathBuf>,
    pub version: Option<String>,
}

pub const TOOL_SPECS: &[(&str, &str, &str, &[&str])] = &[
    ("claude", "Claude Code", "claude", &["--version"]),
    ("codex", "Codex", "codex", &["--version"]),
    ("agy", "Agy", "agy", &["--version"]),
];

pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&path));
    }

    if cfg!(windows) {
        if let Some(appdata) = env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    } else {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
            dirs.push(home.join(".local/bin"));
            dirs.push(home.join(".npm-global/bin"));
            dirs.push(home.join(".bun/bin"));
            dirs.push(home.join(".volta/bin"));
            dirs.push(home.join(".cargo/bin"));
            dirs.push(home.join(".local/share/pnpm"));
            dirs.extend(nvm_bins(&home));
        }
    }

    let mut unique: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if !dir.as_os_str().is_empty() && !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}

fn nvm_bins(home: &std::path::Path) -> Vec<PathBuf> {
    let root = home.join(".nvm/versions/node");
    let mut found: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("bin"))
        .filter(|path| path.is_dir())
        .collect();
    found.sort();
    found.reverse();
    found.truncate(16);
    found
}

fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

pub fn find_binary(binary: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        let candidate = dir.join(binary);
        if is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            for ext in ["exe", "cmd", "bat"] {
                let with_ext = dir.join(format!("{binary}.{ext}"));
                if is_executable(&with_ext) {
                    return Some(with_ext);
                }
            }
        }
    }
    None
}

pub fn parse_version(output: &str) -> Option<String> {
    let is_sep = |c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+');
    for token in output.split(is_sep) {
        let trimmed = token.trim_start_matches('v');
        let core_end = trimmed.find(['-', '+']).unwrap_or(trimmed.len());
        let parts: Vec<&str> = trimmed[..core_end].split('.').collect();
        if parts.len() >= 2
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn read_version(path: &std::path::Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
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
    parse_version(&text)
}

pub fn detect(dirs: &[PathBuf]) -> Vec<Tool> {
    TOOL_SPECS
        .iter()
        .map(|(id, name, binary, version_args)| {
            let path = find_binary(binary, dirs);
            let version = path.as_deref().and_then(|p| read_version(p, version_args));
            Tool {
                id,
                name,
                binary,
                version_args,
                path,
                version,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{find_binary, parse_version};

    #[test]
    fn parses_common_version_output() {
        assert_eq!(parse_version("2.0.14 (Claude Code)"), Some("2.0.14".into()));
        assert_eq!(parse_version("codex-cli 0.20.0"), Some("0.20.0".into()));
        assert_eq!(parse_version("agy v1.5.10"), Some("1.5.10".into()));
        assert_eq!(parse_version("nothing here"), None);
    }

    #[test]
    fn missing_binary_returns_none() {
        let dir = std::env::temp_dir().join("agentterm-tui-none");
        let _ = std::fs::create_dir_all(&dir);
        assert!(find_binary("definitely-not-a-real-binary-xyz", &[dir]).is_none());
    }
}
