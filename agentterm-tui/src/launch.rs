//! Launching: hand the current terminal over to the real tool.
//!
//! Instead of proxying bytes through our own PTY (which is where the desktop
//! build was blanking out), we `exec` the tool directly in the project
//! directory. The tool then owns the terminal natively — its own UI, its own
//! input handling — and when it exits we are back where we started.

use std::path::Path;

/// Build the launch plan for a tool, or explain why it cannot run.
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub dir: std::path::PathBuf,
}

pub fn plan(program: &Path, dir: &Path) -> Launch {
    Launch {
        program: program.to_string_lossy().into_owned(),
        args: Vec::new(),
        dir: dir.to_path_buf(),
    }
}

#[cfg(unix)]
pub fn exec(program: &str, args: &[String], dir: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;

    // exec replaces this process, so the tool inherits the terminal directly.
    let error = std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .exec();
    // Only reached if exec failed.
    error
}

#[cfg(not(unix))]
pub fn exec(program: &str, args: &[String], dir: &Path) -> std::io::Error {
    // On Windows there is no exec; run to completion instead.
    std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .err()
        .unwrap_or_else(|| std::io::Error::other("launch finished"))
}

#[cfg(test)]
mod tests {
    use super::plan;

    #[test]
    fn plan_carries_program_and_directory() {
        let launch = plan(
            std::path::Path::new("/usr/bin/claude"),
            std::path::Path::new("/work/x"),
        );
        assert_eq!(launch.program, "/usr/bin/claude");
        assert_eq!(launch.dir, std::path::PathBuf::from("/work/x"));
        assert!(launch.args.is_empty());
    }
}
