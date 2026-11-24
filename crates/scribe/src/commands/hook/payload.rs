//! What Claude Code sends a hook on stdin, and the transcript provenance
//! check (docs/design/scribe/2_design.md §4.1).
//!
//! The payload is a claim, not authority (STD-05 R1): any process running as
//! the user can pipe JSON into `scribe hook`. Before a hook acts on a session,
//! it confirms the payload's `transcript_path` is a Claude Code transcript
//! of this user for that session: under the transcripts directory, named
//! `<session_id>.jsonl`, a regular file (not a link) owned by this uid and
//! writable by no group or other user. That stops a confused agent or a
//! mistyped command, not a hostile process of the same user; the design
//! documents it as an accident guard (STD-05 R5).

use crate::error::CliError;
use scribe_core::SessionId;
/// What the provenance check found, and why [`read_checked`] gave no bytes
/// for a transcript that exists: the core port's types.
pub(in crate::commands) use scribe_core::cases::{TranscriptState as Transcript, Unreadable};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// The largest payload read from stdin. A prompt with pasted logs can be
/// large, but not this large; more is refused rather than buffered without
/// bound.
const MAX_PAYLOAD: u64 = 16 * 1024 * 1024;

/// The fields of the `UserPromptSubmit` payload scribe uses; the rest are
/// ignored.
#[derive(Debug, Deserialize)]
pub(super) struct PromptPayload {
    pub(super) session_id: String,
    pub(super) transcript_path: PathBuf,
    pub(super) cwd: String,
    pub(super) prompt: String,
    /// Origin claim, where the harness provides one (`source`,
    /// `prompt_source`/`promptSource`, `origin`, `prompt_origin`/`promptOrigin`,
    /// `turn_origin`/`turnOrigin`); a claim, not authority (STD-05 R1), used
    /// only to refuse agent turns.
    #[serde(default, deserialize_with = "scribe_core::capture::present_value")]
    pub(super) source: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "promptSource",
        deserialize_with = "scribe_core::capture::present_value"
    )]
    pub(super) prompt_source_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "scribe_core::capture::present_value")]
    pub(super) prompt_source: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "scribe_core::capture::present_value")]
    pub(super) origin: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "promptOrigin",
        deserialize_with = "scribe_core::capture::present_value"
    )]
    pub(super) prompt_origin_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "scribe_core::capture::present_value")]
    pub(super) prompt_origin: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "turnOrigin",
        deserialize_with = "scribe_core::capture::present_value"
    )]
    pub(super) turn_origin_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "scribe_core::capture::present_value")]
    pub(super) turn_origin: Option<serde_json::Value>,
    #[serde(rename = "isMeta", default)]
    pub(super) is_meta: serde_json::Value,
}

/// Whether this payload's turn was written by an agent or the harness,
/// not plainly typed by the user ([`scribe_core::capture::OriginClaims`]
/// over its origin fields, `isMeta`, and prompt). Such a turn records
/// nothing, decides nothing and does not switch `/scribe` on or off.
pub(super) fn agent_originated(payload: &PromptPayload) -> bool {
    if payload.is_meta.as_bool() == Some(true) {
        return true;
    }
    let mut origin = scribe_core::capture::OriginClaims::default();
    origin.push_claim(payload.source.as_ref());
    origin.push_prompt_source(payload.prompt_source_camel.as_ref());
    origin.push_prompt_source(payload.prompt_source.as_ref());
    origin.push_origin(payload.origin.as_ref());
    origin.push_claim(payload.prompt_origin_camel.as_ref());
    origin.push_claim(payload.prompt_origin.as_ref());
    origin.push_turn_origin(payload.turn_origin_camel.as_ref());
    origin.push_turn_origin(payload.turn_origin.as_ref());
    origin.is_agent_originated(&payload.prompt)
}

/// The fields of the `SessionStart` payload scribe uses.
#[derive(Debug, Deserialize)]
pub(super) struct SessionStartPayload {
    pub(super) session_id: String,
    pub(super) transcript_path: Option<PathBuf>,
    pub(super) source: Option<String>,
    /// Working directory, when the harness sends one. The continuation
    /// reminder matches a recorded session in this directory's project.
    /// Absent, the hook still starts the session and prints no reminder.
    #[serde(default)]
    pub(super) cwd: Option<String>,
}

/// The fields of the `SessionEnd` payload scribe uses.
#[derive(Debug, Deserialize)]
pub(super) struct SessionEndPayload {
    pub(super) session_id: String,
}

/// The fields of the `Stop` payload scribe uses. `scribe mark retry` builds
/// one for a session's transcript to catalog its markers as `Stop` would.
#[derive(Debug, Deserialize)]
pub(in crate::commands) struct StopPayload {
    pub(in crate::commands) session_id: String,
    pub(in crate::commands) transcript_path: PathBuf,
    /// Fallback for transcript lines from clients that omit their cwd.
    #[serde(default)]
    pub(in crate::commands) cwd: Option<String>,
}

/// Read the whole of stdin, up to [`MAX_PAYLOAD`].
pub(super) fn read_stdin() -> Result<Vec<u8>, CliError> {
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_PAYLOAD + 1)
        .read_to_end(&mut input)
        .map_err(|source| CliError::Read {
            what: String::from("stdin"),
            source,
        })?;
    if u64::try_from(input.len()).unwrap_or(u64::MAX) > MAX_PAYLOAD {
        return Err(CliError::HookPayload(format!(
            "stdin is larger than {} MiB",
            MAX_PAYLOAD / 1024 / 1024
        )));
    }
    Ok(input)
}

/// Parse a payload.
pub(super) fn decode<T: DeserializeOwned>(input: &[u8]) -> Result<T, CliError> {
    serde_json::from_slice(input).map_err(|e| CliError::HookPayload(e.to_string()))
}

/// The payload's session id, validated.
pub(super) fn session(id: &str) -> Result<SessionId, CliError> {
    id.parse()
        .map_err(|e: scribe_core::SessionError| CliError::HookPayload(e.to_string()))
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
pub(in crate::commands) fn check_transcript(
    path: &Path,
    session: &SessionId,
    root: Option<&Path>,
) -> Result<Transcript, String> {
    check_shape(path, session, root)?;
    let root = root.ok_or("no transcripts directory: HOME is not set")?;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Transcript::Missing),
        Err(e) => return Err(format!("cannot inspect the transcript: {e}")),
    };
    if !meta.file_type().is_file() {
        return Err(String::from("the transcript is not a regular file"));
    }
    let real_root =
        std::fs::canonicalize(root).map_err(|e| format!("cannot resolve transcript root: {e}"))?;
    let real =
        std::fs::canonicalize(path).map_err(|e| format!("cannot resolve transcript: {e}"))?;
    if !real.starts_with(&real_root) {
        return Err(String::from("transcript escapes its physical root"));
    }
    private_to_this_user(&meta)?;
    Ok(Transcript::Present)
}

/// Whether the transcript is this user's alone to write: owned by this uid,
/// with no group or other write bit. Every provenance decision calls this on
/// the file it checked (path preflight, open handle, handle read), so a
/// writable transcript is refused whichever entry point reached it (STD-05 R9).
#[cfg(unix)]
fn private_to_this_user(meta: &std::fs::Metadata) -> Result<(), String> {
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
fn private_to_this_user(_meta: &std::fs::Metadata) -> Result<(), String> {
    Ok(())
}

/// Provider-aware check. Codex names rollouts with a timestamp prefix and
/// supplies the session identity in host metadata, which must agree.
pub(in crate::commands) fn check(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
) -> Result<Transcript, String> {
    let root = ctx
        .transcript_roots()
        .into_iter()
        .find(|root| path.starts_with(root));
    let found = check_transcript(path, session, root)?;
    let codex_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.starts_with("rollout-"));
    if codex_name != (ctx.provider() == scribe_core::Provider::Codex) {
        return Err(String::from(
            "transcript provider does not match --provider",
        ));
    }
    if matches!(found, Transcript::Present) && ctx.provider() == scribe_core::Provider::Codex {
        let bytes = read_for(ctx, path).map_err(|e| e.to_string())?;
        let rollout = scribe_core::codex::parse(&bytes)?;
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
/// [`Transcript::Missing`]; the error is why not, for the reply.
pub(in crate::commands) fn read_checked(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    let Some(bytes) = read_checked_with(ctx, path, session, Rollout::Verify)? else {
        return Ok(None);
    };
    if ctx.provider() == scribe_core::Provider::Codex {
        let rollout = scribe_core::codex::parse(&bytes).map_err(Unreadable::Refused)?;
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
pub(in crate::commands) fn read_checked_bytes(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    read_checked_with(ctx, path, session, Rollout::Unparsed)
}

/// Read only the transcript prelude, with the same path and open-handle checks.
pub(super) fn claude_header(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    read_checked_with(ctx, path, session, Rollout::Header)
}

/// Whether a Codex read parses the rollout and checks its identity.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rollout {
    Verify,
    Unparsed,
    Header,
}

fn read_checked_with(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
    rollout: Rollout,
) -> Result<Option<Vec<u8>>, Unreadable> {
    let Some((file, _)) = open_checked(ctx, path, session)? else {
        return Ok(None);
    };
    read_opened(ctx, path, file, rollout)
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
pub(super) fn open_checked(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<(std::fs::File, std::fs::Metadata)>, Unreadable> {
    let refused = |why: &str| Unreadable::Refused(String::from(why));
    let root = ctx
        .transcript_roots()
        .into_iter()
        .find(|root| path.starts_with(root));
    check_shape(path, session, root).map_err(Unreadable::Refused)?;
    let codex_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.starts_with("rollout-"));
    if codex_name != (ctx.provider() == scribe_core::Provider::Codex) {
        return Err(refused("transcript provider does not match --provider"));
    }
    let file = match open_transcript(ctx, path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if is_link_refusal(&e) => {
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
pub(in crate::commands) fn read_for(
    ctx: &crate::app::Context,
    path: &Path,
) -> std::io::Result<Vec<u8>> {
    let file = open_transcript(ctx, path)?;
    read_opened(ctx, path, file, Rollout::Verify)
}

fn invalid(why: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, why)
}

/// Open `path` beneath the transcripts root it lies in, never following a
/// link.
fn open_transcript(ctx: &crate::app::Context, path: &Path) -> std::io::Result<std::fs::File> {
    let root = ctx
        .transcript_roots()
        .into_iter()
        .find(|root| path.starts_with(root))
        .ok_or_else(|| {
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
    open_beneath(root, &parts)
}

/// Whether `e` is the refusal to open a link where a file or directory was
/// expected (`O_NOFOLLOW` on a link, or a directory component that is one).
fn is_link_refusal(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        use rustix::io::Errno;
        matches!(Errno::from_io_error(e), Some(Errno::LOOP | Errno::NOTDIR))
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

/// Validate the open `file` and read its bytes. The owner and write-bit check
/// runs on the handle before any byte is read, for every provider. A Codex
/// rollout is also bounded and must name the session its own metadata does.
fn read_opened(
    ctx: &crate::app::Context,
    path: &Path,
    mut file: std::fs::File,
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
    if ctx.provider() != scribe_core::Provider::Codex {
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
    let rollout = scribe_core::codex::parse(&bytes).map_err(invalid)?;
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

#[cfg(unix)]
fn open_beneath(root: &Path, parts: &[Component<'_>]) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, open, openat};
    let root = std::fs::canonicalize(root)?;
    let mut dir = open(
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for (n, part) in parts.iter().enumerate() {
        let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        if n + 1 != parts.len() {
            flags |= OFlags::DIRECTORY;
        } else {
            // A pipe swapped in for the file must not block the open; the
            // type check on the handle refuses it.
            flags |= OFlags::NONBLOCK;
        }
        dir = openat(&dir, part.as_os_str(), flags, Mode::empty())?;
    }
    Ok(std::fs::File::from(dir))
}

#[cfg(not(unix))]
fn open_beneath(root: &Path, parts: &[Component<'_>]) -> std::io::Result<std::fs::File> {
    let mut path = root.to_path_buf();
    path.extend(parts.iter().map(|part| part.as_os_str()));
    std::fs::File::open(path)
}

/// Transcript entries found under a root: regular files, and entries named
/// like a transcript whose file type is refused (a link, a device, a
/// directory). A refused entry is present, not absent: it is never read. Every
/// refused entry is kept, sorted by id and path, even when several in one root
/// share a session id; a caller that opens them checks each one.
#[derive(Default)]
pub(in crate::commands) struct Discovered {
    pub(in crate::commands) files: Vec<(SessionId, PathBuf)>,
    pub(in crate::commands) refused: Vec<(SessionId, PathBuf)>,
}

/// The session id a directory entry is named for, if its name is a transcript's.
fn named_session(name: &std::ffi::OsStr, provider: scribe_core::Provider) -> Option<SessionId> {
    let stem = name.to_str()?.strip_suffix(".jsonl")?;
    let raw = if provider == scribe_core::Provider::Codex {
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
    provider: scribe_core::Provider,
    only: Option<&SessionId>,
) -> Result<Discovered, CliError> {
    let mut pending = vec![(dir.to_path_buf(), 0usize)];
    let mut found = std::collections::BTreeMap::new();
    let mut refused = std::collections::BTreeSet::new();
    let mut count = 0usize;
    while let Some((parent, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&parent) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(CliError::Read {
                    what: parent.display().to_string(),
                    source,
                });
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| CliError::Read {
                what: parent.display().to_string(),
                source,
            })?;
            count += 1;
            if count > 100_000 {
                return Err(CliError::Usage(String::from(
                    "transcript discovery exceeds 100000 files/directories",
                )));
            }
            let kind = entry.file_type().map_err(|source| CliError::Read {
                what: entry.path().display().to_string(),
                source,
            })?;
            let max_depth = if provider == scribe_core::Provider::Codex {
                3
            } else {
                1
            };
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
                return Err(CliError::Usage(String::from(
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
pub(in crate::commands) fn files_for(
    ctx: &crate::app::Context,
) -> Result<Vec<(SessionId, PathBuf)>, CliError> {
    Ok(discovered_for(ctx)?.files)
}

/// [`files_for`], with the refused entries of every root.
pub(in crate::commands) fn discovered_for(
    ctx: &crate::app::Context,
) -> Result<Discovered, CliError> {
    discovered_only(ctx, None)
}

fn discovered_only(
    ctx: &crate::app::Context,
    only: Option<&SessionId>,
) -> Result<Discovered, CliError> {
    let mut all = std::collections::BTreeMap::new();
    let mut refused = Vec::new();
    for root in ctx.transcript_roots() {
        let found = discover(root, ctx.provider(), only)?;
        for (id, path) in found.files {
            if all.insert(id, path).is_some() {
                return Err(CliError::Usage(String::from(
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

/// Resolve a provider transcript, preserving discovery errors.
pub(in crate::commands) fn locate(
    ctx: &crate::app::Context,
    id: &SessionId,
) -> Result<Option<PathBuf>, CliError> {
    Ok(files_for(ctx)?
        .into_iter()
        .find(|(found, _)| found == id)
        .map(|(_, path)| path))
}

/// [`locate`] for a caller that needs only its own transcript. A duplicate id
/// belonging to another session elsewhere in the tree is not an error here;
/// a duplicate of `id` itself still is.
pub(in crate::commands) fn locate_own(
    ctx: &crate::app::Context,
    id: &SessionId,
) -> Result<Option<PathBuf>, CliError> {
    Ok(discovered_only(ctx, Some(id))?
        .files
        .into_iter()
        .next()
        .map(|(_, path)| path))
}
