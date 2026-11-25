//! The `consistency` app: the weekly scan that opens review proposals,
//! finishes approvals cut short and deactivates stale sessions
//! (`scribe consistency scan`, and its routine; 2_design.md §16).

pub mod rules;
mod scan;
#[cfg(test)]
mod tests;

pub use scan::{COMMAND, Deactivated, DeactivationFailed, ScanError, ScanInput, ScanOutcome};

use crate::composition::Context;
use crate::runtime::audit::{Audit, AuditMeta, AuditSubject, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::runtime::{Ctx, PortError};

/// A `consistency` command.
pub enum Command {
    /// `consistency scan`.
    Scan(ScanInput),
}

impl Command {
    /// What the audit log records for this command, or `None` when it is
    /// not audited: a read writes nothing, not even an audit line (STD-01
    /// R31). Exhaustive, so a new command cannot compile without deciding
    /// (STD-02 R27).
    #[must_use]
    pub fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Scan(_) => Some(AuditMeta {
                command: COMMAND,
                stepwise: true,
            }),
        }
    }

    /// The record and agent this command names: none.
    #[must_use]
    pub fn subject(&self, _ctx: &Context) -> AuditSubject {
        match self {
            Self::Scan(_) => AuditSubject::default(),
        }
    }
}

/// What a `consistency` command returns.
pub enum Outcome {
    /// What the scan did.
    Scanned(Box<ScanOutcome>),
}

/// Why a `consistency` command did not run.
pub type Error<E> = ScanError<E>;

/// Run `command` against `ctx`, inside the audit guard when
/// [`Command::audit`] declares one.
///
/// # Errors
///
/// [`ScanError`]: the config, the redactor, or a store error.
pub fn exec(ctx: &Context, command: Command) -> Result<Outcome, CommandError> {
    let audit = Audit::declared(command.audit(), || command.subject(ctx));
    guarded(ctx, audit, || Ok(run(&ctx.core(), command)?))
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(ctx: &Ctx<'_, E>, command: Command) -> Result<Outcome, Error<E>> {
    match command {
        Command::Scan(input) => Ok(Outcome::Scanned(Box::new(scan::run(ctx, input)?))),
    }
}

/// A scan writes one line per entry re-applied, per proposal opened, closed
/// or covered, per session deactivated, and one failure line per
/// deactivation that failed; none when it changed nothing.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        let Self::Scanned(outcome) = self;
        outcome
            .reapplied
            .applied
            .iter()
            .map(|r| Line::success(Some(r.entry.to_string())))
            .chain(
                outcome
                    .opened
                    .iter()
                    .map(|p| Line::success(Some(p.id.to_string()))),
            )
            .chain(
                outcome
                    .closed
                    .iter()
                    .map(|p| Line::success(Some(p.id.to_string()))),
            )
            .chain(
                outcome
                    .reapplied
                    .covered
                    .iter()
                    .map(|item| Line::success(Some(item.proposal.to_string()))),
            )
            .chain(
                outcome
                    .deactivated
                    .iter()
                    .map(|d| Line::success(Some(format!("session:{}", d.session)))),
            )
            .chain(
                outcome
                    .failed_deactivations
                    .iter()
                    .map(|f| Line::failure(Some(format!("session:{}", f.session)), f.error_code)),
            )
            .collect()
    }
}
