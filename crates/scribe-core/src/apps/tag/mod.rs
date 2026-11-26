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
pub(crate) use config::redactor;
pub use list::{TagList, list_tags};
pub use rules::check_agent_author;
pub use show::Shown;
#[cfg(any(test, feature = "test-support"))]
pub use verbs::TagVerbs;

use crate::composition::Context;
use crate::runtime::audit::{Audit, AuditMeta, AuditSubject, Audited, Line, guarded};
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

impl Command {
    /// What the audit log records for this command, or `None` when it is
    /// not audited: a read writes nothing, not even an audit line (STD-01
    /// R31). Exhaustive, so a new command cannot compile without deciding
    /// (STD-02 R27).
    #[must_use]
    pub fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List | Self::Show(_) => None,
            Self::Add(_) => Some(AuditMeta {
                command: "tag add",
                stepwise: false,
            }),
            Self::Update { .. } => Some(AuditMeta {
                command: "tag update",
                stepwise: false,
            }),
        }
    }

    /// The record and agent this command names: none, since the tag
    /// commands name theirs in their audit lines.
    #[must_use]
    pub fn subject(&self, _ctx: &Context) -> AuditSubject {
        AuditSubject::default()
    }
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

/// Run `command` against `ctx`, inside the audit guard when
/// [`Command::audit`] declares one.
///
/// # Errors
///
/// [`Error`]: a refusal, the redactor, or a store error.
pub fn exec(ctx: &Context, command: Command) -> Result<Outcome, CommandError> {
    let audit = Audit::declared(command.audit(), || command.subject(ctx));
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
