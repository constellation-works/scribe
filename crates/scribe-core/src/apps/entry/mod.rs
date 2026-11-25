//! The `entry` app: notes written with agent provenance, and the decision
//! and open-item records (`scribe entry`, 2_design.md §12, §18).
//!
//! `list` and `show` read only; every other command is an audited write,
//! checked before the store lock where it can be and again by the store
//! under it. The instruction check (`--session`/`--instruction`) is here,
//! below `proposal settle`, which runs it too.

mod add;
mod answer;
mod close;
mod erase;
mod forget;
mod instruction;
mod list;
mod pickup;
mod retire;
pub mod rules;
mod scope;
mod show;
#[cfg(test)]
mod tests;
mod update;
mod verbs;
mod verify;
mod written;

pub use add::{AddInput, RecordFields};
pub use erase::erase_session_documents;
pub use instruction::Instruction;
pub use list::{ListInput, Listed};
pub use pickup::PickupInput;
#[cfg(any(test, feature = "test-support"))]
pub use pickup::PickupVerbs;
pub use rules::message::{keeps, typed_turn};
pub use rules::{
    SettleRefusal, Settled, check_content, check_handoff, forget, required_message, required_reason,
};
pub use scope::{handoffs_for_session, of_members};
pub use show::Shown;
#[cfg(any(test, feature = "test-support"))]
pub use verbs::EntryVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use verify::verify_quotes_in_sessions;
pub use written::{Actions, EntryWritten};

pub(crate) use instruction::{check as check_instruction, note as note_instruction};

use crate::composition::Context;
use crate::runtime::audit::{Audit, AuditMeta, AuditSubject, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::runtime::errors::{ReadError, WriteError};
use crate::runtime::{Ctx, PortError};
use crate::{AuthorName, Cite, Entry, EntryId, EntryPatch, Provider, Related, SessionId, Store};

/// An `entry` command.
pub enum Command<'i> {
    /// `entry list`.
    List(Box<ListInput>),
    /// `entry show <id>`.
    Show(String),
    /// `entry add`.
    Add(Box<AddInput>),
    /// `entry pickup`.
    Pickup(PickupInput<'i>),
    /// `entry update`.
    Update {
        /// The entry: an id or unique prefix.
        id: String,
        /// What changes.
        patch: Box<EntryPatch>,
        /// The agent updating it.
        author: AuthorName,
        /// The provider whose transcripts resolve session references.
        provider: Provider,
    },
    /// `entry retire`.
    Retire {
        /// The entry: an id or unique prefix.
        id: String,
        /// Why the note no longer applies.
        reason: String,
        /// The agent retiring it.
        author: AuthorName,
        /// The instruction it is run on.
        instruction: Instruction<'i>,
    },
    /// `entry forget`.
    Forget {
        /// The entry: an id or unique prefix.
        id: String,
        /// The agent forgetting it.
        author: AuthorName,
        /// Whether `--confirm` was given.
        confirm: bool,
        /// The instruction it is run on.
        instruction: Instruction<'i>,
    },
    /// `entry answer`.
    Answer {
        /// The open item: an id or unique prefix.
        id: String,
        /// The agent recording the answer.
        author: AuthorName,
        /// The user's answer, verbatim.
        quote: String,
        /// The sessions the user typed it in.
        related: Vec<Related>,
        /// The provider whose transcripts hold them.
        provider: Provider,
    },
    /// `entry close`.
    Close {
        /// The open item: an id or unique prefix.
        id: String,
        /// The decision that settles it.
        decision: Option<EntryId>,
        /// What was done, when no decision came of it.
        note: Option<String>,
        /// The asker closing it.
        author: AuthorName,
    },
}

impl Command<'_> {
    /// What the audit log records for this command, or `None` when it is
    /// not audited: a read writes nothing, not even an audit line (STD-01
    /// R31). Exhaustive, so a new command cannot compile without deciding
    /// (STD-02 R27).
    #[must_use]
    pub fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
            Self::Add(_) => Some(AuditMeta {
                command: "entry add",
                stepwise: false,
            }),
            Self::Update { .. } => Some(AuditMeta {
                command: "entry update",
                stepwise: false,
            }),
            Self::Retire { .. } => Some(AuditMeta {
                command: "entry retire",
                stepwise: false,
            }),
            Self::Forget { .. } => Some(AuditMeta {
                command: "entry forget",
                stepwise: false,
            }),
            Self::Pickup(_) => Some(AuditMeta {
                command: "entry pickup",
                stepwise: false,
            }),
            Self::Answer { .. } => Some(AuditMeta {
                command: "entry answer",
                stepwise: false,
            }),
            Self::Close { .. } => Some(AuditMeta {
                command: "entry close",
                stepwise: false,
            }),
        }
    }

    /// The record and agent this command names, resolved before it runs so
    /// a refusal is attributable too. Exhaustive like [`Command::audit`].
    #[must_use]
    pub fn subject(&self, ctx: &Context) -> AuditSubject {
        let of = |id: &str, author: &AuthorName| AuditSubject {
            target: resolved_id(ctx, id),
            author: Some(author.as_str().to_owned()),
        };
        match self {
            Self::List(_) | Self::Show(_) => AuditSubject::default(),
            Self::Add(input) => AuditSubject {
                target: None,
                author: Some(input.author.as_str().to_owned()),
            },
            Self::Pickup(input) => of(input.id, &input.author),
            Self::Update { id, author, .. }
            | Self::Retire { id, author, .. }
            | Self::Forget { id, author, .. }
            | Self::Answer { id, author, .. }
            | Self::Close { id, author, .. } => of(id, author),
        }
    }
}

/// What an `entry` command returns.
pub enum Outcome {
    /// What `list` found.
    Listed(Box<Listed>),
    /// What `show` read.
    Shown(Box<Shown>),
    /// What a write did.
    Written(Box<EntryWritten>),
}

/// Why an `entry` command did not run.
#[derive(Debug)]
pub enum Error<E> {
    /// A read's error (`list`, `show`).
    Read(ReadError<E>),
    /// A write's error.
    Write(WriteError<E>),
}

impl<E> From<ReadError<E>> for Error<E> {
    fn from(err: ReadError<E>) -> Self {
        Self::Read(err)
    }
}

impl<E> From<WriteError<E>> for Error<E> {
    fn from(err: WriteError<E>) -> Self {
        Self::Write(err)
    }
}

impl<E> From<crate::Error> for Error<E> {
    fn from(err: crate::Error) -> Self {
        Self::Read(ReadError::Core(err))
    }
}

/// Run `command` against `ctx`, inside the audit guard when
/// [`Command::audit`] declares one.
///
/// # Errors
///
/// [`Error`]: a refusal, a usage error, a transcript read, the redactor,
/// or a store error.
pub fn exec(ctx: &Context, command: Command<'_>) -> Result<Outcome, CommandError> {
    let audit = Audit::declared(command.audit(), || command.subject(ctx));
    guarded(ctx, audit, || Ok(run(&ctx.core(), command)?))
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    command: Command<'_>,
) -> Result<Outcome, Error<E>> {
    let written = match command {
        Command::List(input) => return Ok(Outcome::Listed(Box::new(list::list(ctx, *input)?))),
        Command::Show(id) => return Ok(Outcome::Shown(Box::new(show::show(ctx, &id)?))),
        Command::Add(input) => add::add(ctx, *input)?,
        Command::Pickup(input) => pickup::pickup(ctx, input)?,
        Command::Update {
            id,
            patch,
            author,
            provider,
        } => update::update(ctx, &id, *patch, author, provider)?,
        Command::Retire {
            id,
            reason,
            author,
            instruction,
        } => retire::retire(ctx, &id, &reason, author, &instruction)?,
        Command::Forget {
            id,
            author,
            confirm,
            instruction,
        } => forget::forget(ctx, &id, &author, confirm, &instruction)?,
        Command::Answer {
            id,
            author,
            quote,
            related,
            provider,
        } => answer::answer(ctx, &id, &author, &quote, &related, provider)?,
        Command::Close {
            id,
            decision,
            note,
            author,
        } => close::close(ctx, &id, decision.as_ref(), note.as_deref(), author)?,
    };
    Ok(Outcome::Written(Box::new(written)))
}

/// The id of the entry `input` resolves to, or `None` when it does not: the
/// audit subject's target, which [`Command::subject`] names before the command
/// runs.
#[must_use]
pub fn resolved_id(ctx: &Context, input: &str) -> Option<String> {
    resolve_entry(ctx.store(), input)
        .ok()
        .map(|entry| entry.id.to_string())
}

/// The entry `input` (an id or unique prefix) names.
///
/// # Errors
///
/// An unknown or ambiguous id, or a store error.
pub(crate) fn resolve_entry(store: &Store, input: &str) -> crate::Result<Entry> {
    verbs::EntryVerbs::resolve_entry(store, input)
}

/// Whether any entry from `session` still waits for its cite's message
/// uuid, for the `Stop` hook. Reads only.
///
/// # Errors
///
/// A store error.
pub(crate) fn has_unfilled_cites(store: &Store, session: &SessionId) -> crate::Result<bool> {
    verbs::EntryVerbs::has_unfilled_cites(store, session)
}

/// Under the store lock, fill the cite's message uuid of every entry from
/// `session` that has none yet with what `find` returns, for the `Stop`
/// hook. The ids filled, oldest first.
///
/// # Errors
///
/// A store error.
pub(crate) fn fill_cite_messages(
    store: &Store,
    session: &SessionId,
    now: time::OffsetDateTime,
    find: impl Fn(&Cite) -> Option<String>,
) -> crate::Result<Vec<EntryId>> {
    verbs::EntryVerbs::fill_cite_messages(store, session, now, find)
}

/// `scribe entry`'s errors, each the variant it was before the app moved.
impl From<Error<crate::runtime::errors::CommandError>> for crate::runtime::errors::CommandError {
    fn from(err: Error<crate::runtime::errors::CommandError>) -> Self {
        match err {
            Error::Read(e) => e.into(),
            Error::Write(e) => e.into(),
        }
    }
}

/// A write names its entry when it changed it, and writes no line when it
/// did not.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        match self {
            Self::Written(written) => {
                Line::written(Some(written.entry.id.to_string()), written.changed)
            }
            Self::Shown(shown) => vec![Line::success(Some(shown.entry.id.to_string()))],
            Self::Listed(_) => vec![Line::success(None)],
        }
    }
}
