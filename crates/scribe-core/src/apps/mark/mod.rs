//! The `mark` app: the `#scribe` markers scribe wrote no entry for
//! (`scribe mark`, 2_design.md §14.5), and recording each marker's outcome
//! for the hooks.

mod list;
mod verbs;

#[cfg(any(test, feature = "test-support"))]
pub use verbs::MarkVerbs;
pub use verbs::{MarkChange, MarkKey, MarkResult};

use crate::annotation::MarkRecord;
use crate::composition::Context;
use crate::runtime::errors::CommandError;
use crate::runtime::{Ctx, PortError};
use crate::{Provider, SessionId, Store};
use time::OffsetDateTime;

/// A `mark` command.
pub enum Command {
    /// `mark list`, of one session when given.
    List {
        /// The session: an id or unique prefix.
        session: Option<SessionId>,
        /// The provider whose transcripts resolve it.
        provider: Provider,
    },
}

/// What a `mark` command returns.
pub enum Outcome {
    /// The session as resolved, and its markers.
    List {
        /// The session as resolved.
        session: Option<SessionId>,
        /// The markers.
        marks: Vec<MarkRecord>,
    },
}

/// Why a `mark` command did not run.
pub type Error = crate::Error;

/// Run `command` against `runtime`.
///
/// # Errors
///
/// An ambiguous or too-short prefix, or a store error.
pub fn exec(ctx: &Context, command: Command) -> Result<Outcome, CommandError> {
    Ok(run(&ctx.core(), command)?)
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(ctx: &Ctx<'_, E>, command: Command) -> Result<Outcome, Error> {
    Ok(match command {
        Command::List { session, provider } => {
            let (session, marks) = list::list(ctx, provider, session.as_ref())?;
            Outcome::List { session, marks }
        }
    })
}

/// Record each marker's outcome in one write (2_design.md §14.5), for the
/// `Stop` hook.
///
/// # Errors
///
/// A forgotten session, or a store error.
pub(crate) fn record_marks(
    store: &Store,
    updates: Vec<(MarkKey, MarkChange)>,
    now: OffsetDateTime,
) -> crate::Result<Vec<MarkResult>> {
    use verbs::MarkVerbs;
    store.record_marks(updates, now)
}
