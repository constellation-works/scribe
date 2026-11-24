//! The `tag` app: configured and store tags (`scribe tag`).
//!
//! `list` and `show` read only; `add` and `update` write an agent's store
//! tag without changing config.toml.

mod add;
mod list;
mod show;
mod update;
mod verbs;

pub use add::TagAddInput;
pub use show::Shown;
#[cfg(any(test, feature = "test-support"))]
pub use verbs::TagVerbs;

use crate::runtime::errors::WriteError;
use crate::runtime::{Ctx, PortError, Runtime};
use crate::{AuthorName, StoreTag, TagList, TagName, TagPatch};

/// A `tag` command.
pub enum Command {
    /// `tag list`.
    List,
    /// `tag show <tag>`.
    Show(String),
    /// `tag add`.
    Add(TagAddInput),
    /// `tag update`.
    Update {
        /// The store tag.
        name: TagName,
        /// What changes.
        patch: TagPatch,
        /// The agent updating it.
        author: AuthorName,
    },
}

/// What a `tag` command returns.
pub enum Outcome {
    /// The tags.
    List(TagList),
    /// One tag.
    Show(Box<Shown>),
    /// The tag added.
    Added(Box<StoreTag>),
    /// The tag, and whether this changed it.
    Updated(Box<StoreTag>, bool),
}

/// Why a `tag` command did not run.
pub type Error<E> = WriteError<E>;

/// Run `command` against `runtime`.
///
/// # Errors
///
/// [`Error`]: a refusal, the redactor, or a store error.
pub fn exec<E: PortError>(runtime: &Runtime<'_, E>, command: Command) -> Result<Outcome, Error<E>> {
    run(&runtime.ctx(), command)
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(ctx: &Ctx<'_, E>, command: Command) -> Result<Outcome, Error<E>> {
    Ok(match command {
        Command::List => Outcome::List(list::list(ctx)?),
        Command::Show(word) => Outcome::Show(Box::new(show::show(ctx, &word)?)),
        Command::Add(input) => Outcome::Added(Box::new(add::add(ctx, input)?)),
        Command::Update {
            name,
            patch,
            author,
        } => {
            let (tag, changed) = update::update(ctx, &name, patch, author)?;
            Outcome::Updated(Box::new(tag), changed)
        }
    })
}
