//! What Claude Code sends a hook on stdin, and the transcript provenance
//! check (docs/design/scribe/2_design.md §4.1).
//!
//! The payload is a claim, not authority (STD-05 R1): any process running as
//! Daniel can pipe JSON into `scribe hook`. Before a hook acts on a session,
//! it confirms the payload's `transcript_path` is a Claude Code transcript
//! of this user for that session: under the transcripts directory, named
//! `<session_id>.jsonl`, a regular file (not a link) owned by this uid. That
//! stops a confused agent or a mistyped command, not a hostile process of the
//! same user; the design documents it as an accident guard (STD-05 R5).

use crate::error::CliError;
use scribe_core::SessionId;
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
    /// `prompt_source`, `origin` or `prompt_origin`); a claim, not
    /// authority (STD-05 R1), used only to refuse agent turns.
    #[serde(default)]
    pub(super) source: Option<String>,
    #[serde(default, alias = "promptSource")]
    pub(super) prompt_source: Option<String>,
    #[serde(default)]
    pub(super) origin: Option<String>,
    #[serde(default, alias = "promptOrigin")]
    pub(super) prompt_origin: Option<String>,
}

/// Source values that mean Daniel typed the prompt himself.
const PLAIN_SOURCES: &[&str] = &[
    "typed",
    "user",
    "human",
    "daniel",
    "interactive",
    "keyboard",
    "manual",
    "direct",
    "user-typed",
    "plain",
];

/// Opening tags of agent- or system-written user turns. A prompt carrying
/// one of these anywhere is not Daniel's typing, however a `#scribe` line
/// sits inside it.
const AGENT_WRAPPERS: &[&str] = &[
    "<agent-message",
    "<task-notification",
    "<side-sessions-event",
    "<side-session-event",
    "<side-session",
    "<scheduled-prompt",
    "<scheduled-loop",
    "<scheduled",
    "<loop-prompt",
    "<loop-",
    "<teammate-message",
    "<cross-session",
];

/// Whether this payload's turn was written by an agent or the harness,
/// not plainly typed by Daniel: a non-plain origin field, an agent
/// wrapper anywhere in the prompt, or a prompt opening with a markup tag
/// (a plainly typed prompt does not open that way). Such a turn records
/// nothing, decides nothing and does not switch `/scribe` on or off.
/// This is an accident guard like the transcript check (STD-05 R5): a
/// process running as Daniel can still forge a plain payload.
pub(super) fn agent_originated(payload: &PromptPayload) -> bool {
    let sources = [
        payload.source.as_deref(),
        payload.prompt_source.as_deref(),
        payload.origin.as_deref(),
        payload.prompt_origin.as_deref(),
    ];
    for source in sources.into_iter().flatten() {
        let normalized = source.trim().to_lowercase().replace(['_', ' '], "-");
        if !normalized.is_empty() && !PLAIN_SOURCES.contains(&normalized.as_str()) {
            return true;
        }
    }
    // Underscores and hyphens spell the same wrapper (`<agent_message>`).
    let lower = payload.prompt.to_lowercase().replace('_', "-");
    if AGENT_WRAPPERS.iter().any(|tag| lower.contains(tag)) {
        return true;
    }
    let trimmed = lower.trim_start_matches('\u{feff}').trim_start();
    trimmed
        .strip_prefix('<')
        .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
}

/// The fields of the `SessionStart` payload scribe uses.
#[derive(Debug, Deserialize)]
pub(super) struct SessionStartPayload {
    pub(super) session_id: String,
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

/// The fields of the `Stop` payload scribe uses.
#[derive(Debug, Deserialize)]
pub(super) struct StopPayload {
    pub(super) session_id: String,
    pub(super) transcript_path: PathBuf,
    /// Fallback for transcript lines from clients that omit their cwd.
    #[serde(default)]
    pub(super) cwd: Option<String>,
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

/// What the provenance check found.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::commands) enum Transcript {
    /// The transcript exists and passed.
    Present,
    /// The path is right but there is no file yet (a session's first prompt
    /// can arrive before Claude Code writes its transcript).
    Missing,
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
    owned_by_this_user(&meta)?;
    Ok(Transcript::Present)
}

#[cfg(unix)]
fn owned_by_this_user(meta: &std::fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let uid = rustix::process::geteuid().as_raw();
    if meta.uid() == uid {
        Ok(())
    } else {
        Err(format!(
            "the transcript is owned by uid {}, not this user (uid {uid})",
            meta.uid()
        ))
    }
}

#[cfg(not(unix))]
fn owned_by_this_user(_meta: &std::fs::Metadata) -> Result<(), String> {
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
    owned_by_this_user(&meta).map_err(Unreadable::Refused)?;
    let bytes = read_opened(ctx, path, file).map_err(|e| {
        if e.kind() == std::io::ErrorKind::InvalidInput {
            Unreadable::Refused(e.to_string())
        } else {
            Unreadable::Io(e)
        }
    })?;
    if ctx.provider() == scribe_core::Provider::Codex {
        let rollout = scribe_core::codex::parse(&bytes).map_err(Unreadable::Refused)?;
        if rollout.session != *session {
            return Err(refused("Codex session_meta does not match session_id"));
        }
    }
    Ok(Some(bytes))
}

/// Why [`read_checked`] gave no bytes for a transcript that exists.
#[derive(Debug)]
pub(in crate::commands) enum Unreadable {
    /// The provenance check refused it; the text is why, for the reply.
    Refused(String),
    /// The file could not be opened or read.
    Io(std::io::Error),
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
    read_opened(ctx, path, file)
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

/// Validate the open `file` and read its bytes. A Codex rollout is also
/// bounded, writable by no one else, and must name the session its own
/// metadata does.
fn read_opened(
    ctx: &crate::app::Context,
    path: &Path,
    mut file: std::fs::File,
) -> std::io::Result<Vec<u8>> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(invalid(String::from(
            "the transcript is not a regular file",
        )));
    }
    owned_by_this_user(&meta).map_err(invalid)?;
    if ctx.provider() != scribe_core::Provider::Codex {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        return Ok(bytes);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o022 != 0 {
            return Err(invalid(String::from(
                "Codex transcript is writable by another user",
            )));
        }
    }
    const MAX: u64 = 64 * 1024 * 1024;
    let mut bytes = Vec::new();
    (&mut file).take(MAX + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX {
        return Err(invalid(String::from("Codex transcript exceeds 64 MiB")));
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

/// Discover only the provider's expected layout; never follow directory links.
pub(in crate::commands) fn files(
    dir: &Path,
    provider: scribe_core::Provider,
) -> Result<Vec<(SessionId, PathBuf)>, CliError> {
    let mut pending = vec![(dir.to_path_buf(), 0usize)];
    let mut found = std::collections::BTreeMap::new();
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
            if kind.is_dir() && depth < max_depth {
                pending.push((entry.path(), depth + 1));
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(stem) = name.to_str().and_then(|s| s.strip_suffix(".jsonl")) else {
                continue;
            };
            let raw = if provider == scribe_core::Provider::Codex {
                if !stem.starts_with("rollout-") || stem.len() < 36 {
                    continue;
                }
                let Some(id) = stem.get(stem.len() - 36..) else {
                    continue;
                };
                id
            } else {
                stem
            };
            let Ok(id) = raw.parse::<SessionId>() else {
                continue;
            };
            if found.insert(id, entry.path()).is_some() {
                return Err(CliError::Usage(String::from(
                    "ambiguous duplicate transcript session id",
                )));
            }
        }
    }
    Ok(found.into_iter().collect())
}

/// Discover the active provider's live and archived rollouts without duplicates.
pub(in crate::commands) fn files_for(
    ctx: &crate::app::Context,
) -> Result<Vec<(SessionId, PathBuf)>, CliError> {
    let mut all = std::collections::BTreeMap::new();
    for root in ctx.transcript_roots() {
        for (id, path) in files(root, ctx.provider())? {
            if all.insert(id, path).is_some() {
                return Err(CliError::Usage(String::from(
                    "session exists in both active and archived transcript roots",
                )));
            }
        }
    }
    Ok(all.into_iter().collect())
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
