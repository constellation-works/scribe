//! Provider transcripts on this host: discovery under the transcript roots,
//! the provenance check, and reads through one checked handle
//! (docs/design/scribe/2_design.md §4.1).
//!
//! A hook payload's `transcript_path` is a claim, not authority (STD-05 R1).
//! Before scribe acts on a session it confirms the path is this user's
//! transcript for that session: under a transcripts root, named for the
//! session, a regular file (not a link) owned by this uid and writable by no
//! group or other user. That stops a confused agent or a mistyped command,
//! not a hostile process of the same user (STD-05 R5).
//!
//! The functions take a [`Scope`], the resolved roots and provider, never
//! the caller's context: composition resolves the roots and passes them down.

#[cfg(unix)]
mod attribution;
mod claude;
pub mod codex;
mod parse;
#[cfg(all(test, unix))]
mod tests;

use crate::fs::{self, File, Metadata};
// Transcript bytes read into typed messages (the domain's `transcript`
// until ORB-27921 PR 5-12), at this module's path.
pub use attribution::{attribute, mention_counts};
pub use parse::*;
#[cfg(unix)]
use scribe_store::ClaudeCapturePosition;
use scribe_store::Provider;
use scribe_store::session::SessionId;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Where a provider's transcripts are, and which provider's they are.
pub struct Scope<'a> {
    roots: Vec<&'a Path>,
    provider: Provider,
}

impl<'a> Scope<'a> {
    /// The transcripts under `roots`, read as `provider`'s.
    #[must_use]
    pub fn new(roots: Vec<&'a Path>, provider: Provider) -> Self {
        Self { roots, provider }
    }

    /// The root `path` lies under, if any.
    fn root_of(&self, path: &Path) -> Option<&'a Path> {
        self.roots
            .iter()
            .copied()
            .find(|root| path.starts_with(root))
    }
}

/// Whether a transcript that passed its checks is there yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptState {
    /// The transcript exists and passed.
    Present,
    /// The path is right but there is no file yet.
    Missing,
}

/// Why a transcript could not be read.
#[derive(Debug)]
pub enum Unreadable {
    /// The provenance check refused it; the text is why.
    Refused(String),
    /// The file could not be opened or read.
    Io(std::io::Error),
}

/// Why the transcript tree could not be listed.
#[derive(Debug)]
pub enum DiscoverError {
    /// A directory or one of its entries could not be read.
    Read {
        /// The path that could not be read.
        what: String,
        /// Why.
        source: std::io::Error,
    },
    /// The tree is too large, or names a session twice; the text is why.
    Refused(String),
}

/// One read of a Claude transcript's new records ([`claude_window`]).
#[cfg(unix)]
pub enum ClaudeRead {
    /// The file changed during the read (cut short, truncated or rewritten
    /// in place): read it again.
    Retry,
    /// The new complete records, or `None` when there are none.
    Window(Option<ClaudeWindow>),
}

/// A Claude transcript's new complete records and where they end.
#[cfg(unix)]
pub struct ClaudeWindow {
    /// The records' bytes.
    pub bytes: Vec<u8>,
    /// Whether the read resumed from the saved position.
    pub resumed: bool,
    /// Where the records end, for the next read.
    pub position: ClaudeCapturePosition,
}

/// Whether a Codex read parses the rollout and checks its identity.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rollout {
    Verify,
    Unparsed,
    Header,
}

/// TranscriptState entries found under a root: regular files, and entries named
/// like a transcript whose file type is refused (a link, a device, a
/// directory). A refused entry is present, not absent: it is never read. Every
/// refused entry is kept, sorted by id and path, even when several in one root
/// share a session id; a caller that opens them checks each one.
#[derive(Debug, Default)]
pub struct Discovered {
    /// The regular transcripts, by session id.
    pub files: Vec<(SessionId, PathBuf)>,
    /// The entries named like a transcript whose file type is refused.
    pub refused: Vec<(SessionId, PathBuf)>,
}

/// When `path` was last modified, when the file system says.
#[must_use]
pub fn modified(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// Open `path` checked, read its complete records after `prior`, and ask for
/// a retry when the file changed during the read.
///
/// # Errors
///
/// [`Unreadable`].
#[cfg(unix)]
pub fn claude_window(
    scope: &Scope<'_>,
    path: &Path,
    session: &SessionId,
    prior: Option<&ClaudeCapturePosition>,
) -> Result<Option<ClaudeRead>, Unreadable> {
    let Some((mut file, meta)) = open_checked(scope, path, session)? else {
        return Ok(None);
    };
    let window = match claude::read_window(&mut file, &meta, prior) {
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(Some(ClaudeRead::Retry));
        }
        result => result.map_err(Unreadable::Io)?,
    };
    let after = file.metadata().map_err(Unreadable::Io)?;
    // Retry a truncation or same-size rewrite during the read.
    // Appends past the snapshot's end are for the next hook.
    if after.len() < meta.len()
        || (after.len() == meta.len()
            && after.modified().map_err(Unreadable::Io)?
                != meta.modified().map_err(Unreadable::Io)?)
    {
        return Ok(Some(ClaudeRead::Retry));
    }
    Ok(Some(ClaudeRead::Window(window.map(|window| {
        ClaudeWindow {
            bytes: window.bytes,
            resumed: window.resumed,
            position: window.position,
        }
    }))))
}

/// The path-shape half of the provenance check: `path` is absolute, free of
/// `.` and `..`, under `root`, and named for `session`. Reads nothing.
fn check_shape(path: &Path, session: &SessionId, root: Option<&Path>) -> Result<(), String> {
    let root = root.ok_or("no transcripts directory: HOME is not set")?;
    let inside = path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        && path.starts_with(root);
    if !inside {
        return Err(format!(
            "the transcript path is not under {}",
            root.display()
        ));
    }
    let codex_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.starts_with("rollout-") && s.ends_with(&format!("-{session}")));
    let named = path.extension().is_some_and(|e| e == "jsonl")
        && path
            .file_stem()
            .is_some_and(|stem| stem == session.as_str())
        || (path.extension().is_some_and(|e| e == "jsonl") && codex_name);
    if !named {
        return Err(String::from(
            "the transcript is not named for the payload's session_id",
        ));
    }
    Ok(())
}

/// Check `path` is this user's Claude Code transcript for `session`, under
/// `root` (the transcripts directory). The error is why not, for the reply.
///
/// This inspects the path and reads nothing: a caller that goes on to read
/// the transcript uses [`read_checked`], which checks the file it opened
/// rather than the name it was told, so a swap in between cannot redirect
/// the read.
fn check_transcript(
    path: &Path,
    session: &SessionId,
    root: Option<&Path>,
) -> Result<TranscriptState, String> {
    check_shape(path, session, root)?;
    let root = root.ok_or("no transcripts directory: HOME is not set")?;
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(TranscriptState::Missing),
        Err(e) => return Err(format!("cannot inspect the transcript: {e}")),
    };
    if !meta.file_type().is_file() {
        return Err(String::from("the transcript is not a regular file"));
    }
    let real_root =
        fs::canonicalize(root).map_err(|e| format!("cannot resolve transcript root: {e}"))?;
    let real = fs::canonicalize(path).map_err(|e| format!("cannot resolve transcript: {e}"))?;
    if !real.starts_with(&real_root) {
        return Err(String::from("transcript escapes its physical root"));
    }
    private_to_this_user(&meta)?;
    Ok(TranscriptState::Present)
}

/// Whether the transcript is this user's alone to write: owned by this uid,
/// with no group or other write bit. Every provenance decision calls this on
/// the file it checked (path preflight, open handle, handle read), so a
/// writable transcript is refused whichever entry point reached it (STD-05 R9).
#[cfg(unix)]
fn private_to_this_user(meta: &Metadata) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let uid = rustix::process::geteuid().as_raw();
    if meta.uid() != uid {
        return Err(format!(
            "the transcript is owned by uid {}, not this user (uid {uid})",
            meta.uid()
        ));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(String::from(
            "the transcript is writable by another user (a group or other write bit is set)",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_to_this_user(_meta: &Metadata) -> Result<(), String> {
    Ok(())
}

/// Provider-aware check. Codex names rollouts with a timestamp prefix and
/// supplies the session identity in host metadata, which must agree.
pub fn check(
    scope: &Scope<'_>,
    path: &Path,
    session: &SessionId,
) -> Result<TranscriptState, String> {
    let root = scope.root_of(path);
    let found = check_transcript(path, session, root)?;
    let codex_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.starts_with("rollout-"));
    if codex_name != (scope.provider == Provider::Codex) {
        return Err(String::from(
            "transcript provider does not match --provider",
        ));
    }
    if matches!(found, TranscriptState::Present) && scope.provider == Provider::Codex {
        let bytes = read_for(scope, path).map_err(|e| e.to_string())?;
        let rollout = codex::parse(&bytes)?;
        if rollout.session != *session {
            return Err(String::from("Codex session_meta does not match session_id"));
        }
    }
    Ok(found)
}

/// Check `path` is this user's transcript for `session` and read it, through
/// one handle: the file is opened once, every component below the
/// transcripts root without following links (and without blocking on a
/// pipe), and its type, owner and bytes come from that open file. A path
/// swapped for a link or another file after discovery is refused or read as
/// what it now is under the root, never followed out of it.
///
/// `Ok(None)` is a transcript that does not exist yet, like
/// [`TranscriptState::Missing`]; the error is why not, for the reply.
pub fn read_checked(
    scope: &Scope<'_>,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    let Some(bytes) = read_checked_with(scope, path, session, Rollout::Verify)? else {
        return Ok(None);
    };
    if scope.provider == Provider::Codex {
        let rollout = codex::parse(&bytes).map_err(Unreadable::Refused)?;
        if rollout.session != *session {
            return Err(Unreadable::Refused(String::from(
                "Codex session_meta does not match session_id",
            )));
        }
    }
    Ok(Some(bytes))
}

/// [`read_checked`] without its Codex rollout check: the file's path,
/// type, owner, mode and size are checked on the one open handle and its
/// bytes returned unparsed, for a caller that classifies a Codex session
/// itself before parsing it (the instruction check refuses a `codex exec`
/// rollout as not interactive rather than as unreadable).
pub fn read_checked_bytes(
    scope: &Scope<'_>,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    read_checked_with(scope, path, session, Rollout::Unparsed)
}

/// Read only the transcript prelude, with the same path and open-handle checks.
pub fn claude_header(
    scope: &Scope<'_>,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    read_checked_with(scope, path, session, Rollout::Header)
}

fn read_checked_with(
    scope: &Scope<'_>,
    path: &Path,
    session: &SessionId,
    rollout: Rollout,
) -> Result<Option<Vec<u8>>, Unreadable> {
    let Some((file, _)) = open_checked(scope, path, session)? else {
        return Ok(None);
    };
    read_opened(scope, path, file, rollout)
        .map(Some)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidInput {
                Unreadable::Refused(e.to_string())
            } else {
                Unreadable::Io(e)
            }
        })
}

/// Open and check one file before any read or seek.
pub fn open_checked(
    scope: &Scope<'_>,
    path: &Path,
    session: &SessionId,
) -> Result<Option<(File, Metadata)>, Unreadable> {
    let refused = |why: &str| Unreadable::Refused(String::from(why));
    let root = scope.root_of(path);
    check_shape(path, session, root).map_err(Unreadable::Refused)?;
    let codex_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.starts_with("rollout-"));
    if codex_name != (scope.provider == Provider::Codex) {
        return Err(refused("transcript provider does not match --provider"));
    }
    let file = match open_transcript(scope, path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if fs::is_link_refusal(&e) => {
            return Err(refused("the transcript is not a regular file"));
        }
        Err(e) => return Err(Unreadable::Io(e)),
    };
    let meta = file.metadata().map_err(Unreadable::Io)?;
    if !meta.is_file() {
        return Err(refused("the transcript is not a regular file"));
    }
    private_to_this_user(&meta).map_err(Unreadable::Refused)?;
    Ok(Some((file, meta)))
}

/// Read a transcript [`check`] passed, through the same handle discipline as
/// [`read_checked`] (opened without following links, validated on the open
/// file). Prefer [`read_checked`] when the caller has no other use for the
/// separate check.
pub fn read_for(scope: &Scope<'_>, path: &Path) -> std::io::Result<Vec<u8>> {
    let file = open_transcript(scope, path)?;
    read_opened(scope, path, file, Rollout::Verify)
}

fn invalid(why: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, why)
}

/// Open `path` beneath the transcripts root it lies in, never following a
/// link.
fn open_transcript(scope: &Scope<'_>, path: &Path) -> std::io::Result<File> {
    let root = scope.root_of(path).ok_or_else(|| {
        invalid(String::from(
            "the transcript is outside the configured transcript roots",
        ))
    })?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid(String::from("the transcript escapes its configured root")))?;
    let parts: Vec<_> = relative.components().collect();
    if parts.is_empty() || parts.iter().any(|p| !matches!(p, Component::Normal(_))) {
        return Err(invalid(String::from("invalid transcript path")));
    }
    fs::open_beneath(root, &parts)
}

/// Validate the open `file` and read its bytes. The owner and write-bit check
/// runs on the handle before any byte is read, for every provider. A Codex
/// rollout is also bounded and must name the session its own metadata does.
fn read_opened(
    scope: &Scope<'_>,
    path: &Path,
    mut file: File,
    rollout: Rollout,
) -> std::io::Result<Vec<u8>> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(invalid(String::from(
            "the transcript is not a regular file",
        )));
    }
    private_to_this_user(&meta).map_err(invalid)?;
    if rollout == Rollout::Header {
        let mut bytes = Vec::new();
        file.take(64 * 1024).read_to_end(&mut bytes)?;
        return Ok(bytes);
    }
    if scope.provider != Provider::Codex {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        return Ok(bytes);
    }
    const MAX: u64 = 64 * 1024 * 1024;
    let mut bytes = Vec::new();
    (&mut file).take(MAX + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX {
        return Err(invalid(String::from("Codex transcript exceeds 64 MiB")));
    }
    if rollout == Rollout::Unparsed {
        return Ok(bytes);
    }
    let rollout = codex::parse(&bytes).map_err(invalid)?;
    let named = path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| {
        s.starts_with("rollout-") && s.ends_with(&format!("-{}", rollout.session))
    });
    if !named {
        return Err(invalid(String::from(
            "Codex transcript name and host session identity disagree",
        )));
    }
    Ok(bytes)
}

/// The session id a directory entry is named for, if its name is a transcript's.
fn named_session(name: &std::ffi::OsStr, provider: Provider) -> Option<SessionId> {
    let stem = name.to_str()?.strip_suffix(".jsonl")?;
    let raw = if provider == Provider::Codex {
        if !stem.starts_with("rollout-") || stem.len() < 36 {
            return None;
        }
        stem.get(stem.len() - 36..)?
    } else {
        stem
    };
    raw.parse::<SessionId>().ok()
}

/// Discover only the provider's expected layout; never follow directory links.
/// Also reports entries named like a transcript that are not regular files, so
/// a caller can tell a refused entry from an absent one. With `only`, another
/// session's entries are neither kept nor compared, so a duplicate id
/// elsewhere in the tree is not an error.
fn discover(
    dir: &Path,
    provider: Provider,
    only: Option<&SessionId>,
) -> Result<Discovered, DiscoverError> {
    let mut pending = vec![(dir.to_path_buf(), 0usize)];
    let mut found = std::collections::BTreeMap::new();
    let mut refused = std::collections::BTreeSet::new();
    let mut count = 0usize;
    while let Some((parent, depth)) = pending.pop() {
        let entries = match fs::read_dir(&parent) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(DiscoverError::Read {
                    what: parent.display().to_string(),
                    source,
                });
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| DiscoverError::Read {
                what: parent.display().to_string(),
                source,
            })?;
            count += 1;
            if count > 100_000 {
                return Err(DiscoverError::Refused(String::from(
                    "transcript discovery exceeds 100000 files/directories",
                )));
            }
            let kind = entry.file_type().map_err(|source| DiscoverError::Read {
                what: entry.path().display().to_string(),
                source,
            })?;
            let max_depth = if provider == Provider::Codex { 3 } else { 1 };
            let id = named_session(&entry.file_name(), provider);
            if kind.is_dir() && depth < max_depth {
                pending.push((entry.path(), depth + 1));
            }
            let Some(id) = id else {
                continue;
            };
            if only.is_some_and(|only| only != &id) {
                continue;
            }
            if !kind.is_file() {
                refused.insert((id, entry.path()));
                continue;
            }
            if found.insert(id, entry.path()).is_some() {
                return Err(DiscoverError::Refused(String::from(
                    "ambiguous duplicate transcript session id",
                )));
            }
        }
    }
    Ok(Discovered {
        files: found.into_iter().collect(),
        refused: refused.into_iter().collect(),
    })
}

/// Discover the active provider's live and archived rollouts without duplicates.
pub fn files_for(scope: &Scope<'_>) -> Result<Vec<(SessionId, PathBuf)>, DiscoverError> {
    Ok(discovered_for(scope)?.files)
}

/// [`files_for`], with the refused entries of every root.
pub fn discovered_for(scope: &Scope<'_>) -> Result<Discovered, DiscoverError> {
    discovered_only(scope, None)
}

fn discovered_only(
    scope: &Scope<'_>,
    only: Option<&SessionId>,
) -> Result<Discovered, DiscoverError> {
    let mut all = std::collections::BTreeMap::new();
    let mut refused = Vec::new();
    for root in scope.roots.iter().copied() {
        let found = discover(root, scope.provider, only)?;
        for (id, path) in found.files {
            if all.insert(id, path).is_some() {
                return Err(DiscoverError::Refused(String::from(
                    "session exists in both active and archived transcript roots",
                )));
            }
        }
        refused.extend(found.refused);
    }
    Ok(Discovered {
        files: all.into_iter().collect(),
        refused,
    })
}

/// The transcript of `id` among [`files_for`], for a caller that needs only
/// its own transcript. A duplicate id
/// belonging to another session elsewhere in the tree is not an error here;
/// a duplicate of `id` itself still is.
pub fn locate_own(scope: &Scope<'_>, id: &SessionId) -> Result<Option<PathBuf>, DiscoverError> {
    Ok(discovered_only(scope, Some(id))?
        .files
        .into_iter()
        .next()
        .map(|(_, path)| path))
}
