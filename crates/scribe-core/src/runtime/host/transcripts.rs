//! The transcript reads over a [`Context`]'s roots and provider: the
//! provenance check, the checked reads and discovery of
//! `scribe_tools::transcript`, scoped to the context, with a listing failure
//! as a [`CommandError`].

use crate::SessionId;
use crate::composition::Context;
use crate::runtime::errors::CommandError;
#[cfg(unix)]
use scribe_tools::transcript::ClaudeRead;
use scribe_tools::transcript::{self, DiscoverError, Discovered, Scope};
/// What the provenance check found, and why [`read_checked`] gave no bytes
/// for a transcript that exists: the core port's types.
pub(crate) use scribe_tools::transcript::{TranscriptState as Transcript, Unreadable};
use std::path::{Path, PathBuf};

/// The transcript roots and provider `ctx` reads as.
fn scope(ctx: &Context) -> Scope<'_> {
    Scope::new(ctx.transcript_roots(), ctx.provider())
}

/// A listing failure as the host error the CLI has always reported for it.
fn listing(err: DiscoverError) -> CommandError {
    match err {
        DiscoverError::Read { what, source } => CommandError::Read { what, source },
        DiscoverError::Refused(why) => CommandError::Usage(why),
    }
}

/// Provider-aware check ([`transcript::check`]).
pub(crate) fn check(ctx: &Context, path: &Path, session: &SessionId) -> Result<Transcript, String> {
    transcript::check(&scope(ctx), path, session)
}

/// Check and read through one handle ([`transcript::read_checked`]).
pub(crate) fn read_checked(
    ctx: &Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    transcript::read_checked(&scope(ctx), path, session)
}

/// [`read_checked`] without its Codex rollout check
/// ([`transcript::read_checked_bytes`]).
pub(crate) fn read_checked_bytes(
    ctx: &Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    transcript::read_checked_bytes(&scope(ctx), path, session)
}

/// Read only the transcript prelude ([`transcript::claude_header`]).
pub(crate) fn claude_header(
    ctx: &Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    transcript::claude_header(&scope(ctx), path, session)
}

/// Open and check one file before any read or seek
/// ([`transcript::open_checked`]), for the read tests.
#[cfg(test)]
pub(crate) fn open_checked(
    ctx: &Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<(scribe_tools::fs::File, scribe_tools::fs::Metadata)>, Unreadable> {
    transcript::open_checked(&scope(ctx), path, session)
}

/// Read a transcript [`check`] passed ([`transcript::read_for`]).
pub(crate) fn read_for(ctx: &Context, path: &Path) -> std::io::Result<Vec<u8>> {
    transcript::read_for(&scope(ctx), path)
}

/// The active provider's live and archived rollouts
/// ([`transcript::files_for`]).
pub(crate) fn files_for(ctx: &Context) -> Result<Vec<(SessionId, PathBuf)>, CommandError> {
    transcript::files_for(&scope(ctx)).map_err(listing)
}

/// [`files_for`], with the refused entries of every root
/// ([`transcript::discovered_for`]).
pub(crate) fn discovered_for(ctx: &Context) -> Result<Discovered, CommandError> {
    transcript::discovered_for(&scope(ctx)).map_err(listing)
}

/// The transcript of `id` alone ([`transcript::locate_own`]).
pub(crate) fn locate_own(ctx: &Context, id: &SessionId) -> Result<Option<PathBuf>, CommandError> {
    transcript::locate_own(&scope(ctx), id).map_err(listing)
}

/// The transcripts port's Claude window ([`transcript::claude_window`]).
#[cfg(unix)]
pub(crate) fn claude_window(
    ctx: &Context,
    path: &Path,
    session: &SessionId,
    prior: Option<&crate::ClaudeCapturePosition>,
) -> Result<Option<ClaudeRead>, Unreadable> {
    transcript::claude_window(&scope(ctx), path, session, prior)
}
