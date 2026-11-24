//! The `memory` app: what scribe remembers about the user — the profile,
//! `now` items and milestones (`scribe memory`, 2_design.md §15.4–§15.6).
//!
//! `show` reads only; `ack`, `forget`, `restore` and `refresh` are audited
//! writes. `refresh` is the one flow that calls a model.

mod ack;
mod block;
mod forget;
mod refresh;
mod restore;
mod show;
mod verbs;

pub use block::{is_empty, text, warnings};
pub use refresh::REFRESH_BUDGET;
pub use show::Shown;
#[cfg(any(test, feature = "test-support"))]
pub use verbs::MemoryVerbs;

use crate::memory::NowItem;
use crate::models::ModelError;
use crate::runtime::{Ctx, PortError, Runtime};
use crate::{AuthorName, MemoryShown, Models, Provider, RefreshWritten, SessionId};

/// A `memory` command.
pub enum Command<'m> {
    /// `memory show`: profile revision `revision` when given, else the
    /// item `id` when given, else the block.
    Show {
        /// The profile revision.
        revision: Option<i64>,
        /// The item.
        id: Option<String>,
    },
    /// `memory ack`.
    Ack {
        /// The `now` item.
        id: String,
        /// The session that raised it: an id or unique prefix.
        session: SessionId,
        /// The agent acknowledging it.
        author: AuthorName,
        /// The provider whose transcripts resolve the session.
        provider: Provider,
    },
    /// `memory forget`.
    Forget {
        /// The item.
        id: String,
        /// The agent forgetting it.
        author: AuthorName,
        /// Whether `--confirm` was given.
        confirm: bool,
    },
    /// `memory restore`.
    Restore {
        /// The profile revision.
        revision: i64,
    },
    /// `memory refresh` with `models`.
    Refresh {
        /// The provider whose `[models]` table applies.
        provider: Provider,
        /// The model adapter.
        models: &'m dyn Models,
    },
}

/// What a `memory` command returns.
pub enum Outcome {
    /// What `show` read.
    Show(Box<Shown>),
    /// The item, and whether this changed it.
    Acked(Box<NowItem>, bool),
    /// The item forgotten.
    Forgotten(Box<MemoryShown>),
    /// The profile as it now is, and whether this wrote a revision.
    Restored(Box<MemoryShown>, bool),
    /// What a refresh wrote; `None` when nothing was new.
    Refreshed(Option<Box<RefreshWritten>>),
}

/// Why a `scribe memory` write did not run.
#[derive(Debug)]
pub enum Error<E> {
    /// A store error or refusal.
    Core(crate::Error),
    /// A port failed.
    Port(E),
    /// The model call failed.
    Model(ModelError),
    /// `forget` without `--confirm`: the item it would forget.
    ConfirmRequired(Box<MemoryShown>),
}

impl<E> From<crate::Error> for Error<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

impl<E> From<ModelError> for Error<E> {
    fn from(err: ModelError) -> Self {
        Self::Model(err)
    }
}

/// Run `command` against `runtime`.
///
/// # Errors
///
/// [`Error`]: a refusal, the config, the redactor, the model, or the store.
pub fn exec<E: PortError>(
    runtime: &Runtime<'_, E>,
    command: Command<'_>,
) -> Result<Outcome, Error<E>> {
    run(&runtime.ctx(), command)
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    command: Command<'_>,
) -> Result<Outcome, Error<E>> {
    Ok(match command {
        Command::Show { revision, id } => {
            Outcome::Show(Box::new(show::show(ctx, revision, id.as_deref())?))
        }
        Command::Ack {
            id,
            session,
            author,
            provider,
        } => {
            let (item, changed) = ack::ack(ctx, &id, &session, author, provider)?;
            Outcome::Acked(Box::new(item), changed)
        }
        Command::Forget {
            id,
            author,
            confirm,
        } => Outcome::Forgotten(Box::new(forget::forget(ctx, &id, author, confirm)?)),
        Command::Restore { revision } => {
            let (shown, written) = restore::restore(ctx, revision)?;
            Outcome::Restored(Box::new(shown), written)
        }
        Command::Refresh { provider, models } => {
            Outcome::Refreshed(refresh::refresh(ctx, provider, models)?.map(Box::new))
        }
    })
}
