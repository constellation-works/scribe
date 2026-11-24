//! The `brief` app: the session-start brief (`scribe brief`, and the
//! session-start hook; 2_design.md §18).
//!
//! It reads only.

mod page;
pub mod rules;
#[cfg(test)]
mod tests;

pub use page::{BriefInput, BriefPage};

pub(crate) use page::at_start;

use crate::composition::Context;
use crate::runtime::errors::CommandError;
use crate::runtime::{Ctx, PortError};

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

/// Run `command` against `ctx`, inside the audit guard when `audit` names
/// one.
///
/// # Errors
///
/// A store read error.
pub fn exec(ctx: &Context, command: Command<'_>) -> Result<Outcome, CommandError> {
    Ok(run(&ctx.core(), command)?)
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(ctx: &Ctx<'_, E>, command: Command<'_>) -> Result<Outcome, Error> {
    match command {
        Command::Brief(input) => Ok(Outcome::Page(Box::new(page::run(ctx, &input)?))),
    }
}
