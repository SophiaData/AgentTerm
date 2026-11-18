//! Launching: run the chosen tool in the current terminal, then come back.
//!
//! We deliberately do NOT `exec` (replace our own process). Exec is what made
//! macOS lose the way back: once exec'd, AgentTerm's process is gone, so when
//! the tool exits there is no list to return to. Instead we spawn the tool as a
//! child sharing our terminal and `wait` for it. When it exits — normally OR by
//! error — we still own the process and can redraw the list.
//!
//! The return value distinguishes "the tool ran and finished" (fine, go back to
//! the list) from "we could not start it at all" (show a real error). Windows
//! previously reported a normal exit as a failure; this type fixes that.

use std::path::Path;

/// Build the launch plan for a tool.
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

/// What happened when we tried to run the tool.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The tool started and exited (with any status). Go back to the list.
    Finished { code: Option<i32> },
    /// The tool could not be started at all. The string is a human reason.
    FailedToStart(String),
}

/// Run the tool in `dir`, waiting for it to exit, then return an outcome.
///
/// The child inherits our stdin/stdout/stderr, so the terminal is the tool's for
/// the duration of the call. On return the caller re-enters the TUI.
pub fn run(program: &str, args: &[String], dir: &Path) -> Outcome {
    let mut command = std::process::Command::new(program);
    command.args(args).current_dir(dir);

    match command.status() {
        Ok(status) => Outcome::Finished {
            code: status.code(),
        },
        Err(error) => Outcome::FailedToStart(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Outcome, plan, run};

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

    #[cfg(unix)]
    #[test]
    fn normal_exit_is_finished_not_failed() {
        let outcome = run("/usr/bin/true", &[], std::path::Path::new("/tmp"));
        assert_eq!(outcome, Outcome::Finished { code: Some(0) });
    }

    #[cfg(unix)]
    #[test]
    fn nonzero_exit_is_still_finished() {
        // Exit status does not mean "couldn't start"; we still return to the list.
        let outcome = run(
            "/bin/sh",
            &["-c".into(), "exit 3".into()],
            std::path::Path::new("/tmp"),
        );
        assert_eq!(outcome, Outcome::Finished { code: Some(3) });
    }

    #[test]
    fn missing_program_reports_failed_to_start() {
        let outcome = run(
            "/definitely/not/a/real/program-xyz",
            &[],
            std::path::Path::new("/tmp"),
        );
        assert!(matches!(outcome, Outcome::FailedToStart(_)));
    }
}
