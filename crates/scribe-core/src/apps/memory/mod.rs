//! The `memory` app: what scribe remembers about the user — the profile,
//! `now` items and milestones (`scribe memory`, 2_design.md §15.4–§15.6, §19.3.5).
//!
//! `show` reads only; `ack`, `forget`, `restore`, `prepare`, `apply` and
//! `refresh` are audited writes (none invokes a model).

mod ack;
mod block;
mod forget;
mod refresh;
mod restore;
pub mod rules;
mod show;
#[cfg(test)]
mod tests;
mod verbs;
mod window;

pub use block::{is_empty, text, warnings};
pub use refresh::{
    ApplyRefreshOutcome, ApplyRefreshRequest, Basis, PreparedProfileInputs, PreparedRefresh,
    ProfileInputs, REFRESH_BUDGET, REFRESH_SESSIONS, REFRESH_SUMMARY_CHARS, REFRESH_VERBATIM_CHARS,
    REFRESH_VERBATIM_SESSIONS, RefreshCursor, RefreshWritten,
};
pub use rules::check_sensitive_drafts;
pub use show::Shown;
#[cfg(any(test, feature = "test-support"))]
pub use verbs::MemoryVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use window::WindowMemoryTestWrites;
pub use window::{WindowMemory, plan_window};

use crate::composition::Context;
use crate::memory::NowItem;
use crate::runtime::audit::{Audit, AuditMeta, AuditSubject, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::runtime::{Ctx, PortError};
use crate::{AuthorName, MemoryShown, Provider, SessionId};

/// A `memory` command.
pub enum Command {
    /// `memory show`: profile revision `revision` when given, else the
    /// item `id` when given, else the block.
    Show {
        /// The profile revision.
        revision: Option<i64>,
        /// The item.
        id: Option<String>,
    },
    /// `memory ack`.
    Ack {
        /// The `now` item.
        id: String,
        /// The session that raised it: an id or unique prefix.
        session: SessionId,
        /// The agent acknowledging it.
        author: AuthorName,
        /// The provider whose transcripts resolve the session.
        provider: Provider,
    },
    /// `memory forget`.
    Forget {
        /// The item.
        id: String,
        /// The agent forgetting it.
        author: AuthorName,
        /// Whether `--confirm` was given.
        confirm: bool,
    },
    /// `memory restore`.
    Restore {
        /// The profile revision.
        revision: i64,
    },
    /// `memory refresh`: runs the deterministic part (replays any open
    /// `memory_journal` and archives legacy format-24 items), succeeds when
    /// nothing is new, and directs the caller to `orbit job run
    /// scribe-memory-refresh` when a model rebuild is needed.
    Refresh,
    /// `memory prepare` / `scribe.memory-refresh-prepare`: replays any open
    /// `memory_journal` and returns bounded inputs and optimistic [`Basis`].
    Prepare,
    /// `memory apply` / `scribe.memory-refresh-apply`: verifies and commits a
    /// supplied `ProfileAnswer` against its prepared [`Basis`].
    Apply {
        /// The provider whose `[models]` credentials apply to redaction.
        provider: Provider,
        /// The prepared basis/cursor/stems and unchecked answer.
        request: Box<ApplyRefreshRequest>,
        /// Whether the plugin's routine tool runs it: its audit lines then
        /// name no subject, as the routine's always have.
        routine: bool,
    },
}

impl Command {
    /// What the audit log records for this command, or `None` when it is
    /// not audited: a read writes nothing, not even an audit line (STD-01
    /// R31). Exhaustive, so a new command cannot compile without deciding
    /// (STD-02 R27).
    #[must_use]
    pub fn audit(&self) -> Option<AuditMeta> {
        let command = match self {
            Self::Show { .. } => return None,
            Self::Ack { .. } => "memory ack",
            Self::Forget { .. } => "memory forget",
            Self::Restore { .. } => "memory restore",
            Self::Refresh | Self::Prepare | Self::Apply { .. } => "memory refresh",
        };
        // `refresh` is stepwise: its prepare commits the journal replay and
        // format-24 archive in their own write, so the `NeedsOrbit` hand-off
        // that follows does not roll them back.
        Some(AuditMeta {
            command,
            stepwise: matches!(self, Self::Refresh),
        })
    }

    /// The record and agent this command names, resolved before it runs so
    /// a refusal is attributable too. Exhaustive like [`Command::audit`].
    #[must_use]
    pub fn subject(&self, _ctx: &Context) -> AuditSubject {
        let of = |id: &str, author: &AuthorName| AuditSubject {
            target: Some(format!("memory:{}", id.trim())),
            author: Some(author.as_str().to_owned()),
        };
        match self {
            Self::Show { .. } | Self::Prepare | Self::Apply { routine: true, .. } => {
                AuditSubject::default()
            }
            Self::Ack { id, author, .. } | Self::Forget { id, author, .. } => of(id, author),
            Self::Restore { .. } | Self::Refresh | Self::Apply { routine: false, .. } => {
                AuditSubject {
                    target: Some(String::from("memory:profile")),
                    author: None,
                }
            }
        }
    }
}

/// What a `memory` command returns.
pub enum Outcome {
    /// What `show` read.
    Show(Box<Shown>),
    /// The item, and whether this changed it.
    Acked(Box<NowItem>, bool),
    /// The item forgotten.
    Forgotten(Box<MemoryShown>),
    /// The profile as it now is, and whether this wrote a revision.
    Restored(Box<MemoryShown>, bool),
    /// Deterministic `memory refresh` found nothing new.
    RefreshedNothingNew,
    /// Prepared refresh inputs and basis.
    Prepared(Box<PreparedRefresh>),
    /// Result of `memory apply`.
    Applied(Box<ApplyRefreshOutcome>),
}

/// A write names its item (`memory:<id>`) or the profile when it changed
/// it, and writes no line when it did not.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        let item = |shown: &MemoryShown| Some(format!("memory:{}", shown_id(shown)));
        match self {
            Self::Show(shown) => match &**shown {
                Shown::Item(shown) => vec![Line::success(item(shown))],
                Shown::Revision(_) | Shown::Block(_) => vec![Line::success(None)],
            },
            Self::Acked(now, changed) => {
                Line::written(Some(format!("memory:{}", now.stem)), *changed)
            }
            Self::Forgotten(shown) => vec![Line::success(item(shown))],
            Self::Restored(shown, written) => Line::written(item(shown), *written),
            Self::RefreshedNothingNew | Self::Prepared(_) => Vec::new(),
            Self::Applied(outcome) => match &**outcome {
                ApplyRefreshOutcome::Written(_) => {
                    Line::written(Some(String::from("memory:profile")), true)
                }
                ApplyRefreshOutcome::Retry { .. } => Vec::new(),
            },
        }
    }
}

/// Why a `scribe memory` write did not run.
#[derive(Debug)]
pub enum Error<E> {
    /// A store error or refusal.
    Core(crate::Error),
    /// A port failed.
    Port(E),
    /// `forget` without `--confirm`: the item it would forget.
    ConfirmRequired(Box<MemoryShown>),
    /// `memory refresh` found new material that needs an Orbit job or
    /// interactive `prepare`/`apply`.
    NeedsOrbit {
        /// New sessions since the cursor.
        sessions: usize,
        /// Whether the user edited `memory/profile.md`.
        corrections: bool,
        /// Passed `now` items to settle.
        passed: usize,
    },
}

impl<E> From<crate::Error> for Error<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

/// `scribe memory`'s errors, each the variant it was before the flow moved.
impl From<Error<crate::runtime::errors::CommandError>> for crate::runtime::errors::CommandError {
    fn from(err: Error<crate::runtime::errors::CommandError>) -> Self {
        match err {
            Error::Core(e) => Self::Core(e),
            Error::Port(e) => e,
            Error::ConfirmRequired(shown) => Self::ForgetConfirmRequired(shown_id(&shown)),
            Error::NeedsOrbit {
                sessions,
                corrections,
                passed,
            } => {
                let mut pending = Vec::new();
                if sessions > 0 {
                    pending.push(format!("{sessions} session(s)"));
                }
                if corrections {
                    pending.push(String::from("profile edits"));
                }
                if passed > 0 {
                    pending.push(format!("{passed} passed now item(s)"));
                }
                Self::Usage(format!(
                    "memory refresh needs a model run ({}); run `orbit job run scribe-memory-refresh`, or use `scribe memory prepare --json` and `scribe memory apply`",
                    pending.join(", ")
                ))
            }
        }
    }
}

/// Run `command` against `ctx`, inside the audit guard when
/// [`Command::audit`] declares one.
///
/// # Errors
///
/// [`Error`]: a refusal, the config, the redactor, or the store.
pub fn exec(ctx: &Context, command: Command) -> Result<Outcome, CommandError> {
    let audit = Audit::declared(command.audit(), || command.subject(ctx));
    guarded(ctx, audit, || Ok(run(&ctx.core(), command)?))
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(ctx: &Ctx<'_, E>, command: Command) -> Result<Outcome, Error<E>> {
    Ok(match command {
        Command::Show { revision, id } => {
            Outcome::Show(Box::new(show::show(ctx, revision, id.as_deref())?))
        }
        Command::Ack {
            id,
            session,
            author,
            provider,
        } => {
            let (item, changed) = ack::ack(ctx, &id, &session, author, provider)?;
            Outcome::Acked(Box::new(item), changed)
        }
        Command::Forget {
            id,
            author,
            confirm,
        } => Outcome::Forgotten(Box::new(forget::forget(ctx, &id, author, confirm)?)),
        Command::Restore { revision } => {
            let (shown, written) = restore::restore(ctx, revision)?;
            Outcome::Restored(Box::new(shown), written)
        }
        Command::Refresh => {
            refresh::refresh_deterministic(ctx)?;
            Outcome::RefreshedNothingNew
        }
        Command::Prepare => Outcome::Prepared(Box::new(refresh::prepare(ctx)?)),
        Command::Apply {
            provider, request, ..
        } => Outcome::Applied(Box::new(refresh::apply(ctx, provider, &request)?)),
    })
}

/// The id `memory show` prints for an item: `profile`, or a now item's or
/// milestone's stem.
fn shown_id(shown: &MemoryShown) -> String {
    match shown {
        MemoryShown::Profile(_) => String::from("profile"),
        MemoryShown::Now(n) => n.stem.to_string(),
        MemoryShown::Milestone(m) => m.stem.to_string(),
    }
}
