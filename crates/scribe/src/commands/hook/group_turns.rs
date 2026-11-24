//! Capture a member's typed turns from host-checked transcript data:
//! the arguments, then the group app's turn capture. The Claude
//! transcript reader stays here, as the transcripts port's adapter.

use super::payload;
use crate::{app::Context, error::CliError};
use scribe_core::apps::group;
use scribe_core::{SessionId, message};
use std::path::Path;

#[cfg(unix)]
mod claude;

#[cfg(all(test, unix))]
mod tests;

/// The transcripts port's Claude window: open `path` checked, read its
/// complete records after `prior`, and ask for a retry when the file changed
/// during the read.
#[cfg(unix)]
pub(in crate::commands) fn claude_window(
    ctx: &Context,
    path: &Path,
    session: &SessionId,
    prior: Option<&scribe_core::ClaudeCapturePosition>,
) -> Result<Option<scribe_core::runtime::ClaudeRead>, payload::Unreadable> {
    use payload::Unreadable;
    use scribe_core::runtime::{ClaudeRead, ClaudeWindow};

    let Some((mut file, meta)) = payload::open_checked(ctx, path, session)? else {
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

/// The typed turns of a past session whose transcript is at `path`
/// ([`group::past_turns`]).
pub(super) fn past(
    ctx: &Context,
    session: &SessionId,
    path: &Path,
) -> Result<Option<Vec<message::NewMessage>>, CliError> {
    Ok(group::past_turns(
        &ctx.core(),
        ctx.provider(),
        session,
        path,
    )?)
}
