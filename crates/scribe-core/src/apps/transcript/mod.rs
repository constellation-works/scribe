//! The `transcript` app: `scribe transcript list` and `transcript show`
//! (2_design.md §5.8), the provider transcripts on this host that have no
//! session record and no switch, read and surveyed without cataloging them.
//! Transcript inspection is its own subject (STD-02@8 §R45; ORB-28770): it
//! can read a transcript before any session record exists, so it does not
//! belong to the `session` app's records. Both commands are reads.
//!
//! The app calls the `session` app only through its public exports (the
//! cataloged-session listing and the transcript-id lookup), and the tools
//! for discovery, reading and parsing. It writes nothing.

mod read;

pub use read::{ListInput, Listed, Surveyed};

use crate::apps::session::CatalogError;
use crate::composition::Context;
use crate::runtime::audit::{Audit, AuditMeta, AuditSubject, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::runtime::{Ctx, PortError};
use crate::{Provider, SessionId};

/// A `transcript` command.
pub enum Command<'i> {
    /// `transcript list`.
    List {
        /// The provider whose transcripts are listed.
        provider: Provider,
        /// What to list.
        input: ListInput<'i>,
    },
    /// `transcript show`.
    Show {
        /// The provider whose transcript it is.
        provider: Provider,
        /// The transcript: an id or unique prefix.
        id: SessionId,
    },
}

impl Command<'_> {
    /// What the audit log records for this command, or `None` when it is
    /// not audited: both commands are reads, which write nothing, not even
    /// an audit line (STD-01 R31). Exhaustive, so a new command cannot
    /// compile without deciding (STD-02 R27).
    #[must_use]
    pub fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List { .. } | Self::Show { .. } => None,
        }
    }

    /// The record and agent this command names: none, since neither reads
    /// one.
    #[must_use]
    pub fn subject(&self, _ctx: &Context) -> AuditSubject {
        AuditSubject::default()
    }
}

/// What a `transcript` command returns.
pub enum Outcome {
    /// The uncataloged transcripts; `None` when there is no transcripts
    /// directory.
    Transcripts(Option<Box<Listed>>),
    /// One transcript.
    Transcript(Box<Surveyed>),
}

/// Reads: never audited (STD-01 R31).
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        match self {
            Self::Transcripts(_) | Self::Transcript(_) => Vec::new(),
        }
    }
}

/// Why a `transcript` command did not run: a catalog-style read error.
pub type Error<E> = CatalogError<E>;

/// Run `command` inside the audit guard when [`Command::audit`] declares
/// one. Both commands are reads, so no line is written.
///
/// # Errors
///
/// A transcripts directory that cannot be listed, an unknown or ambiguous
/// id, a transcript refused by its provenance check or unreadable, or a
/// store error.
pub fn exec(ctx: &Context, command: Command<'_>) -> Result<Outcome, CommandError> {
    let audit = Audit::declared(command.audit(), || command.subject(ctx));
    guarded(ctx, audit, || Ok(run(&ctx.core(), command)?))
}

/// Run `command` against `ctx`: dispatch only.
fn run<E: PortError>(ctx: &Ctx<'_, E>, command: Command<'_>) -> Result<Outcome, Error<E>> {
    Ok(match command {
        Command::List { provider, input } => {
            Outcome::Transcripts(read::list(ctx, provider, &input)?.map(Box::new))
        }
        Command::Show { provider, id } => {
            Outcome::Transcript(Box::new(read::show(ctx, provider, &id)?))
        }
    })
}
