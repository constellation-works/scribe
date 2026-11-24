//! The `proposal` app: proposed changes to entries (`scribe proposal`,
//! 2_design.md §11), and the settling verbs `approve`, `reject`, `retire`
//! and `forget` behind them.
//!
//! `list` and `show` read only; `add`, `update` and `settle` are audited
//! agent writes. The consistency scan opens, repairs and closes proposals
//! through the functions below.

mod add;
mod list;
mod settle;
mod update;
mod verbs;

pub use add::ProposalAddInput;
pub use list::ListInput;
pub use settle::ProposalSettleInput;
#[cfg(any(test, feature = "test-support"))]
pub use verbs::ProposalVerbs;

use crate::consistency::{Draft, ReapplyReport, Settle, Settled};
use crate::runtime::errors::{ReadError, WriteError};
use crate::runtime::{Ctx, PortError, Runtime};
use crate::{AuthorName, Cite, Proposal, ProposalFilter, Redactor, Store};

/// A `proposal` command.
pub enum Command<'i> {
    /// `proposal list`.
    List(ListInput<'i>),
    /// `proposal show <id>`.
    Show(String),
    /// `proposal add`.
    Add(ProposalAddInput<'i>),
    /// `proposal update`.
    Update {
        /// The proposal: an id or unique prefix.
        id: String,
        /// The replacement reason.
        reason: String,
        /// The agent updating it.
        author: AuthorName,
    },
    /// `proposal settle`.
    Settle(ProposalSettleInput<'i>),
}

/// What a `proposal` command returns.
pub enum Outcome {
    /// The filter as applied, and the proposals `list` found.
    Listed {
        /// The filter as applied.
        filter: Box<ProposalFilter>,
        /// The proposals.
        proposals: Vec<Proposal>,
    },
    /// The proposal `show` read.
    Shown(Box<Proposal>),
    /// The proposal a write left, and whether it changed.
    Written(Box<Proposal>, bool),
}

/// Why a `proposal` command did not run.
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

/// Run `command` against `runtime`.
///
/// # Errors
///
/// [`Error`]: a refusal, a usage error, the instruction check, the
/// redactor, or a store error.
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
    let (proposal, changed) = match command {
        Command::List(input) => {
            let (filter, proposals) = list::list(ctx, input)?;
            return Ok(Outcome::Listed {
                filter: Box::new(filter),
                proposals,
            });
        }
        Command::Show(id) => {
            return Ok(Outcome::Shown(Box::new(
                verbs::ProposalVerbs::resolve_proposal(ctx.store, &id)?,
            )));
        }
        Command::Add(input) => add::add(ctx, input)?,
        Command::Update { id, reason, author } => update::update(ctx, &id, &reason, author)?,
        Command::Settle(input) => settle::settle(ctx, input)?,
    };
    Ok(Outcome::Written(Box::new(proposal), changed))
}

/// The id of the proposal `input` resolves to, or `None` when it does
/// not: the audit line's target. Until migration step 3 moves the audit
/// guard into core, the binary resolves it through this.
#[must_use]
pub fn resolved_id<E>(runtime: &Runtime<'_, E>, input: &str) -> Option<String> {
    verbs::ProposalVerbs::resolve_proposal(runtime.ctx().store, input)
        .ok()
        .map(|proposal| proposal.id.to_string())
}

/// Apply a settling verb under the store lock, for a `#scribe` line.
///
/// # Errors
///
/// A refusal, or a store error.
pub(crate) fn settle(
    store: &Store,
    action: &Settle,
    cite: &Cite,
    now: time::OffsetDateTime,
) -> crate::Result<Settled> {
    verbs::ProposalVerbs::settle(store, action, cite, now)
}

/// Open one proposal per draft not already settled or gone, under the
/// store lock, for the consistency scan. The proposals opened.
///
/// # Errors
///
/// A store error.
pub(crate) fn open_proposals(
    store: &Store,
    drafts: Vec<Draft>,
    now: time::OffsetDateTime,
    redactor: &Redactor,
) -> crate::Result<Vec<Proposal>> {
    verbs::ProposalVerbs::open_proposals(store, drafts, now, redactor)
}

/// Re-apply every approved proposal whose older entry is still `active`,
/// for the consistency scan.
///
/// # Errors
///
/// A store error.
pub(crate) fn reapply_approved(
    store: &Store,
    now: time::OffsetDateTime,
) -> crate::Result<ReapplyReport> {
    verbs::ProposalVerbs::reapply_approved(store, now)
}

/// Close every open candidate proposal whose tag is gone, for the
/// consistency scan. The proposals closed, in id order.
///
/// # Errors
///
/// A store error.
pub(crate) fn close_retired_candidates(
    store: &Store,
    now: time::OffsetDateTime,
) -> crate::Result<Vec<Proposal>> {
    verbs::ProposalVerbs::close_retired_candidates(store, now)
}
