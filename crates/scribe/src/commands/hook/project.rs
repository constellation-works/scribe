//! The project a session's cwd belongs to (docs/design/scribe/2_design.md
//! §3): the repository name from the git remote, else the cwd's basename.
//!
//! Read from git's own files, without running git: walk up from the cwd to
//! the first `.git`, follow a worktree's `gitdir:` file to the common
//! directory, and read the `origin` remote's url from its `config` (the
//! first remote when there is no `origin`). `.../constellation-works/scribe.git`
//! and `git@host:owner/scribe` are both `scribe`.

use scribe_core::Project;
use std::path::{Path, PathBuf};

/// The project for `cwd`.
pub(in crate::commands) fn of(cwd: &Path) -> Project {
    remote_name(cwd).map_or_else(
        || Project::sanitized(&basename(cwd)),
        |name| Project::sanitized(&name),
    )
}

fn basename(cwd: &Path) -> String {
    cwd.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn remote_name(cwd: &Path) -> Option<String> {
    let config = std::fs::read_to_string(common_dir(cwd)?.join("config")).ok()?;
    repo_name(&remote_url(&config)?)
}

/// The git directory shared by every worktree of the repository holding
/// `cwd`.
fn common_dir(cwd: &Path) -> Option<PathBuf> {
    let dot_git = cwd
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|p| p.exists())?;
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    // A worktree: `.git` is a file `gitdir: <dir>`, and `<dir>/commondir`
    // names the shared directory relative to it.
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let base = dot_git.parent()?;
    let gitdir = base.join(gitdir);
    match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(common) => Some(gitdir.join(common.trim())),
        Err(_) => Some(gitdir),
    }
}

/// The `origin` remote's url in a git config, else the first remote's.
pub(super) fn remote_url(config: &str) -> Option<String> {
    let mut section: Option<String> = None;
    let mut first: Option<String> = None;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line
                .strip_prefix("[remote \"")
                .and_then(|rest| rest.strip_suffix("\"]"))
                .map(str::to_owned);
            continue;
        }
        let Some(remote) = &section else { continue };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "url" {
            continue;
        }
        let url = value.trim().to_owned();
        if remote == "origin" {
            return Some(url);
        }
        first.get_or_insert(url);
    }
    first
}

/// The repository name in a remote url: its last path segment, less `.git`.
pub(super) fn repo_name(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':', '\\']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty()).then(|| name.to_owned())
}
