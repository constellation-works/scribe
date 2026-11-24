//! `/scribe group add <slug> <session-id>...`: add past sessions to a project
//! group on a typed line (docs/design/scribe/2_design.md §4.4): the
//! arguments, then [`scribe_core::cases::group_add::run`].
//!
//! The caller has already checked that the prompt is a typed user turn and
//! that its transcript passes the provenance check, exactly as for
//! `/scribe group <slug>`; this module acts only on a line that passed both.
//! It opens the group when absent and adds each named session, whether or not
//! the session was ever recorded. A session whose transcript is on disk has its
//! typed turns stored, chosen as a member's own turns are chosen
//! ([`super::group_turns::past`]); one without a transcript gets membership
//! only. The reply says which, per session. A session already in another
//! group refuses the whole line. The session the line is typed in is added
//! only when it names itself.
//!
//! The reply is a refusal or a success, never an error exit, and the prompt is
//! blocked as a `/scribe` line always is. Audit outcomes carry session ids and
//! error codes only.

use super::group_turns;
#[cfg(test)]
use super::payload::PromptPayload;
use crate::app::Context;
use crate::error::CliError;
#[cfg(test)]
use crate::output::Output;
use scribe_core::SessionId;
#[cfg(test)]
use scribe_core::cases::group_add::{self as case, GroupAddInput};
use scribe_core::message::NewMessage;
use std::path::Path;

/// Apply a typed `group add` line.
#[cfg(test)]
pub(super) fn run(
    slug: &str,
    named: &[SessionId],
    payload: &PromptPayload,
    session: &SessionId,
    ctx: &Context,
) -> Result<Output, CliError> {
    run_with_transcript_reader(slug, named, payload, session, ctx, read_past)
}

/// A named session's past transcript, as `group add` reads it.
pub(super) fn read_past(
    ctx: &Context,
    id: &SessionId,
    path: &Path,
) -> Result<Option<Vec<NewMessage>>, CliError> {
    #[cfg(test)]
    super::tests::group_add::before_past_read(path);
    group_turns::past(ctx, id, path)
}

/// [`run`] with `read_past` reading each named session's past transcript.
#[cfg(test)]
pub(super) fn run_with_transcript_reader(
    slug: &str,
    named: &[SessionId],
    payload: &PromptPayload,
    session: &SessionId,
    ctx: &Context,
    mut read_past: impl FnMut(&Context, &SessionId, &Path) -> Result<Option<Vec<NewMessage>>, CliError>,
) -> Result<Output, CliError> {
    let reply = case::run(
        &ctx.core(),
        &GroupAddInput {
            slug,
            named,
            session,
            cwd: &payload.cwd,
            provider: ctx.provider(),
        },
        |provider, id, path| read_past(&ctx.with_provider(provider), id, path),
    )?;
    Ok(Output::Hook(reply))
}
