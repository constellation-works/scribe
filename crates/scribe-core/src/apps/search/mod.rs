//! The `search` app: one ranked search over the entries, the sessions and
//! the stored messages (`scribe search`; 2_design.md §10).
//!
//! It reads only.

mod query;

pub use query::Searched;

use crate::runtime::errors::ReadError;
use crate::runtime::{Ctx, PortError, Runtime};
use crate::search::SearchQuery;

/// A `search` command: the query, scoped to the sessions of `group` when
/// given (an id, prefix or slug).
pub struct Command<'i> {
    /// The query.
    pub query: &'i SearchQuery,
    /// The group to scope it to.
    pub group: Option<&'i str>,
}

/// What a `search` command returns.
pub enum Outcome {
    /// What the search found.
    Searched(Box<Searched>),
}

/// Why a `search` command did not run.
pub type Error<E> = ReadError<E>;

/// Run `command`: the interim entry point until migration step 3 starts the
/// runtime from an `Env` and wraps the audit guard.
///
/// # Errors
///
/// [`ReadError::Usage`] for a query with no terms or an unknown group, or a
/// store error.
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
    let Command { query, group } = command;
    Ok(Outcome::Searched(Box::new(query::run(ctx, query, group)?)))
}
