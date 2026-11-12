//! The tmux popup adapter: runs the embedded fzf script that `scribe tmux`
//! shows (docs/design/scribe/2_design.md §1.2).
//!
//! The script is a POSIX shell text embedded at build time and run with
//! `sh -c`. Its child commands run the executable that was started, named by
//! [`BIN_ENV`], never whichever `scribe` is on `PATH`; it exports itself in
//! [`SCRIPT_ENV`] so fzf's binds and preview can run it again.
//!
//! This is the one process this crate spawns that owns the terminal: fzf
//! and the pager read the tty, so the child inherits stdio and stays in the
//! foreground process group, and it has no budget (a person is driving it).
//! That departs from `models::supervise` (STD-03@2 §R11), recorded in
//! docs/design/scribe/4_decisions.md. Like `models.rs` it lives in the CLI
//! crate, and it imports no surface module (ARCHITECTURE.md).

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus};

/// The popup script, run through `sh`.
const SCRIPT: &str = include_str!("popup.sh");

/// The executable the popup's child commands run.
const BIN_ENV: &str = "SCRIBE_TMUX_BIN";

/// The script text, exported so fzf's binds and preview can run it again.
const SCRIPT_ENV: &str = "SCRIBE_TMUX_SH";

/// The variable naming the data directory the popup's commands read.
const ROOT_ENV: &str = "SCRIBE_ROOT";

/// Run the popup to completion on the terminal the caller owns.
///
/// `exe` is the running `scribe`, `root` its data directory, and `unset` the
/// variables that would reshape the lists the script parses.
pub(crate) fn run(exe: &Path, root: &Path, unset: &[&str]) -> io::Result<ExitStatus> {
    let mut sh = Command::new("sh");
    sh.arg("-c")
        .arg(SCRIPT)
        .arg("scribe-tmux")
        .env(BIN_ENV, exe)
        .env(SCRIPT_ENV, SCRIPT)
        .env(ROOT_ENV, root);
    for name in unset {
        sh.env_remove(OsStr::new(name));
    }
    sh.status()
}

/// Whether an executable `name` is in a `PATH` directory.
pub(crate) fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| is_executable(&dir.join(name)))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}
