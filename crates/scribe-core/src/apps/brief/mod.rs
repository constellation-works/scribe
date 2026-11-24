//! The `brief` app: the session-start brief (`scribe brief`, and the
//! session-start hook; 2_design.md §18).
//!
//! It reads only.

mod page;

pub use page::{BriefInput, BriefPage};

pub(crate) use page::at_start;

use crate::runtime::{Ctx, PortError, Runtime};

/// A `brief` command.
pub enum Command<'i> {
    /// `brief`: the page for the project and session count given.
    Brief(BriefInput<'i>),
}

/// What a `brief` command returns.
pub enum Outcome {
    /// The brief and what it was built from.
    Page(Box<BriefPage>),
}

/// Why a `brief` command did not run: a store read error.
pub type Error = crate::Error;

/// Run `command`: the interim entry point until migration step 3 starts the
/// runtime from an `Env` and wraps the audit guard.
///
/// # Errors
///
/// A store read error.
pub fn exec<E: PortError>(
    runtime: &Runtime<'_, E>,
    command: Command<'_>,
) -> Result<Outcome, Error> {
    run(&runtime.ctx(), command)
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(ctx: &Ctx<'_, E>, command: Command<'_>) -> Result<Outcome, Error> {
    match command {
        Command::Brief(input) => Ok(Outcome::Page(Box::new(page::run(ctx, &input)?))),
    }
}
