//! The `consistency` app: the weekly scan that opens review proposals,
//! finishes approvals cut short and deactivates stale sessions
//! (`scribe consistency scan`, and its routine; 2_design.md §16).

mod scan;

pub use scan::{COMMAND, Deactivated, DeactivationFailed, ScanError, ScanInput, ScanOutcome};

use crate::runtime::{Ctx, PortError, Runtime};

/// A `consistency` command.
pub enum Command {
    /// `consistency scan`.
    Scan(ScanInput),
}

/// What a `consistency` command returns.
pub enum Outcome {
    /// What the scan did.
    Scanned(Box<ScanOutcome>),
}

/// Why a `consistency` command did not run.
pub type Error<E> = ScanError<E>;

/// Run `command`: the interim entry point until migration step 3 starts the
/// runtime from an `Env` and wraps the audit guard.
///
/// # Errors
///
/// [`ScanError`]: the config, the redactor, or a store error.
pub fn exec<E: PortError>(runtime: &Runtime<'_, E>, command: Command) -> Result<Outcome, Error<E>> {
    run(&runtime.ctx(), command)
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(ctx: &Ctx<'_, E>, command: Command) -> Result<Outcome, Error<E>> {
    match command {
        Command::Scan(input) => Ok(Outcome::Scanned(Box::new(scan::run(ctx, input)?))),
    }
}
