//! The `tag` app: configured and store tags (`scribe tag`).
//!
//! `list` and `show` read only; `add` and `update` write an agent's store
//! tag without changing config.toml.

mod add;
mod config;
mod list;
pub mod rules;
mod show;
#[cfg(test)]
mod tests;
mod update;
mod verbs;

pub use add::TagAddInput;
pub use config::ConfigReads;
pub use list::{TagList, list_tags};
pub use rules::check_agent_author;
pub use show::Shown;
#[cfg(any(test, feature = "test-support"))]
pub use verbs::TagVerbs;

use crate::composition::Context;
use crate::runtime::audit::{Audit, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::runtime::errors::WriteError;
use crate::runtime::{Ctx, PortError};
use crate::{AuthorName, StoreTag, TagName, TagPatch};

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
pub fn exec(
    ctx: &Context,
    audit: Option<Audit>,
    command: Command,
) -> Result<Outcome, CommandError> {
    guarded(ctx, audit, || Ok(run(&ctx.core(), command)?))
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

/// A write names its tag when it changed it, and writes no line when it did
/// not.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        match self {
            Self::Added(tag) => vec![Line::success(Some(tag.name.to_string()))],
            Self::Updated(tag, changed) => Line::written(Some(tag.name.to_string()), *changed),
            // Reads: never audited (STD-01 R31).
            Self::List(_) | Self::Show(_) => vec![Line::success(None)],
        }
    }
}
