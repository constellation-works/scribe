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
mod window;

pub use block::{is_empty, text, warnings};
pub use refresh::{
    Basis, ProfileInputs, REFRESH_BUDGET, REFRESH_SESSIONS, REFRESH_SUMMARY_CHARS,
    REFRESH_VERBATIM_CHARS, REFRESH_VERBATIM_SESSIONS, RefreshWritten,
};
pub use show::Shown;
#[cfg(any(test, feature = "test-support"))]
pub use verbs::MemoryVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use window::WindowMemoryTestWrites;
pub use window::{WindowMemory, plan_window};

use crate::composition::Context;
use crate::memory::NowItem;
use crate::models::ModelError;
use crate::runtime::audit::{Audit, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::runtime::{Ctx, PortError};
use crate::{AuthorName, MemoryShown, Models, Provider, SessionId};

/// The model adapter `memory refresh` builds for the config.
pub type ModelsFor<'m> = Box<dyn FnOnce(&crate::Config) -> Box<dyn Models + 'm> + 'm>;

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
    /// `memory refresh` with the adapter `models` builds for the config.
    Refresh {
        /// The provider whose `[models]` table applies.
        provider: Provider,
        /// The model adapter, built for the provider's config.
        models: ModelsFor<'m>,
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
pub fn exec(
    ctx: &Context,
    audit: Option<Audit>,
    command: Command<'_>,
) -> Result<Outcome, CommandError> {
    guarded(ctx, audit, || Ok(run(&ctx.core(), command)?))
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
            // The config is read before the refresh, as `memory refresh` read
            // it to build the adapter before the flow moved into the core.
            let config = ctx.store.read().tags().config()?.for_provider(provider);
            let models = models(&config);
            Outcome::Refreshed(refresh::refresh(ctx, provider, &*models)?.map(Box::new))
        }
    })
}

/// The id `memory show` prints for an item: `profile`, or a now item's or
/// milestone's stem.
fn shown_id(shown: &MemoryShown) -> String {
    match shown {
        MemoryShown::Profile(_) => String::from("profile"),
        MemoryShown::Now(n) => n.stem.to_string(),
        MemoryShown::Milestone(m) => m.stem.to_string(),
    }
}

/// `scribe memory`'s errors, each the variant it was before the flow moved.
impl From<Error<crate::runtime::errors::CommandError>> for crate::runtime::errors::CommandError {
    fn from(err: Error<crate::runtime::errors::CommandError>) -> Self {
        match err {
            Error::Core(e) => Self::Core(e),
            Error::Port(e) => e,
            Error::Model(e) => Self::Model(e),
            Error::ConfirmRequired(shown) => Self::ForgetConfirmRequired(shown_id(&shown)),
        }
    }
}

/// A write names its item (`memory:<id>`) or the profile when it changed
/// it, and writes no line when it did not.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        let item = |shown: &MemoryShown| Some(format!("memory:{}", shown_id(shown)));
        match self {
            Self::Show(shown) => match &**shown {
                Shown::Item(shown) => vec![Line::success(item(shown))],
                Shown::Revision(_) | Shown::Block(_) => vec![Line::success(None)],
            },
            Self::Acked(now, changed) => {
                Line::written(Some(format!("memory:{}", now.stem)), *changed)
            }
            Self::Forgotten(shown) => vec![Line::success(item(shown))],
            Self::Restored(shown, written) => Line::written(item(shown), *written),
            Self::Refreshed(written) => {
                Line::written(Some(String::from("memory:profile")), written.is_some())
            }
        }
    }
}
