//! The project a session's cwd belongs to (docs/design/scribe/2_design.md
//! §3): the repository name from the git remote, else the cwd's basename.
//!
//! Read from git's own files, without running git: walk up from the cwd to
//! the first `.git`, follow a worktree's `gitdir:` file to the common
//! directory, and read the `origin` remote's url from its `config` (the
//! first remote when there is no `origin`). `.../constellation-works/scribe.git`
//! and `git@host:owner/scribe` are both `scribe`.
//!
//! The hooks run this synchronously, and the metadata is local and
//! workspace-controlled, so each file is read only when it is a regular file
//! no larger than [`MAX_METADATA_BYTES`] and is not itself a link
//! ([`fs::open_no_follow`]). Anything
//! else gives no name, and the basename answers.

use crate::fs::{self, File};
use scribe_store::record::Project;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The largest Git metadata file read for a project name. A real config or
/// pointer is far smaller; a larger file is refused rather than read.
pub(crate) const MAX_METADATA_BYTES: u64 = 64 * 1024;

/// The project for `cwd`. Never fails: when no remote can be read, the cwd's
/// basename names the project.
pub fn of(cwd: &Path) -> Project {
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
    let config = read_metadata(&common_dir(cwd)?.join("config")).ok()?;
    repo_name(&remote_url(&config)?)
}

/// The text of a Git metadata file: a regular file of at most
/// [`MAX_METADATA_BYTES`] bytes, opened without following a link at its own
/// name and without waiting on a pipe or device. Any other file is an error.
fn read_metadata(path: &Path) -> io::Result<String> {
    let mut file: File = fs::open_no_follow(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the Git metadata file is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_METADATA_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the Git metadata file is larger than the limit",
        ));
    }
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
    let text = read_metadata(&dot_git).ok()?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let base = dot_git.parent()?;
    let gitdir = base.join(gitdir);
    match read_metadata(&gitdir.join("commondir")) {
        Ok(common) => Some(gitdir.join(common.trim())),
        // A missing `commondir` means the worktree's directory is the common
        // one; a present but unsafe one names nothing.
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some(gitdir),
        Err(_) => None,
    }
}

/// The `origin` remote's url in a git config, else the first remote's.
pub(crate) fn remote_url(config: &str) -> Option<String> {
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
pub(crate) fn repo_name(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':', '\\']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty()).then(|| name.to_owned())
}
