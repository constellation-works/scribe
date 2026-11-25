//! The `session` app: session records and the passes that write them
//! (`scribe session`, `scribe transcript`; 2_design.md §6, §11, §15).
//!
//! `list`, `show` and the `transcript` reads read only. `pass` settles or
//! prepares and applies Orbit windows. `catalog` and `rebuild` write records; `activate` and
//! `deactivate` write one record. The links, the activity switch, a
//! continuation's handoff and its predecessor are here too, for the hooks
//! and the consistency scan.

mod activity;
mod catalog;
mod continuation;
mod list;
mod pass;
mod rebuild;
pub mod rules;
mod show;
#[cfg(test)]
mod tests;
mod transcript;
mod verbs;
mod view;

pub use activity::{
    ActivateInput, ActivityWritten, DeactivateInput, SessionRef, parse_session_ref,
};
pub use catalog::{Begun, CatalogError, CatalogInput};
pub use list::{ListInput, Listed};
pub use pass::{
    ActivityAnswer, ApplyInput, ApplyOutput, ApplyStatus, CATALOG_COMMAND, FailInput, PASS_COMMAND,
    PASS_START_BY_SECONDS, PassPayload, PassReport, PassRunError, PrepareInput, Prepared,
    SessionPassInput,
};
#[cfg(any(test, feature = "test-support"))]
pub use pass::{ClaimsWritten, PassVerbs, PassWrite, PassWritten, short_reason};
#[cfg(any(test, feature = "test-support"))]
pub use rebuild::RebuildVerbs;
pub use rebuild::{REBUILD_COMMAND, RebuildError, RebuildInput};
pub use show::Shown;
pub use transcript::{ListInput as TranscriptListInput, Listed as TranscriptsListed, Surveyed};
pub use verbs::HandoffSelection;
#[cfg(any(test, feature = "test-support"))]
pub use verbs::SessionVerbs;
pub use view::{SessionList, SessionView, list_sessions, session_view};

pub(crate) use continuation::{ClearedPredecessor, cleared_predecessor, try_cleared_predecessor};

use crate::activity::{ActiveOutcome, SetActive};
use crate::composition::Context;
use crate::models::{
    CloseEvidence, ModelError, ProfileAnswer, ProfileRequest, SessionAnswer, SessionHeader,
    SessionHeaderRequest, SessionRequest,
};
use crate::runtime::audit::{Audit, AuditMeta, AuditSubject, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::runtime::errors::{ReadError, WriteError};
use crate::runtime::{Ctx, PortError};
use crate::{
    Author, AuthorName, Config, Models, Provider, Redactor, SessionId, SessionLink, Store, control,
};
use std::num::NonZeroU32;
use time::OffsetDateTime;

/// The model adapter a `catalog` or `rebuild` builds for the session's
/// config, once.
pub type ModelsOnce<'i> = Box<dyn FnOnce(&Config) -> Box<dyn Models + 'i> + 'i>;

/// A `session` or `transcript` command.
pub enum Command<'i> {
    /// `session list`.
    List(Box<ListInput>),
    /// `session show`: the record, or segment `segment` of its log.
    Show {
        /// The session: an id or unique prefix.
        id: SessionId,
        /// The segment, from 1.
        segment: Option<NonZeroU32>,
    },
    /// `session pass`.
    Pass(SessionPassInput, crate::Label),
    /// Select a pass window or settle a session deterministically.
    PassPrepare(PrepareInput, crate::Label),
    /// Verify a supplied answer and apply it to a prepared pass window.
    PassApply(Box<ApplyInput>, crate::Label),
    /// Record retry backoff for a failed Orbit step.
    PassFail(Box<FailInput>, crate::Label),
    /// `session catalog`.
    Catalog(CatalogInput<'i, ModelsOnce<'i>>),
    /// `session rebuild`.
    Rebuild(RebuildInput<'i, ModelsOnce<'i>>),
    /// `session deactivate`.
    Deactivate(DeactivateInput<'i>),
    /// `session activate`.
    Activate(ActivateInput<'i>),
    /// `transcript list`.
    TranscriptList {
        /// The provider whose transcripts are listed.
        provider: Provider,
        /// What to list.
        input: TranscriptListInput<'i>,
    },
    /// `transcript show`.
    TranscriptShow {
        /// The provider whose transcript it is.
        provider: Provider,
        /// The transcript: an id or unique prefix.
        id: SessionId,
    },
}

/// `session deactivate`'s command path in the audit log.
pub const DEACTIVATE_COMMAND: &str = "session deactivate";

/// `session activate`'s command path in the audit log.
pub const ACTIVATE_COMMAND: &str = "session activate";

impl Command<'_> {
    /// What the audit log records for this command, or `None` when it is
    /// not audited: a read writes nothing, not even an audit line (STD-01
    /// R31). Exhaustive, so a new command cannot compile without deciding
    /// (STD-02 R27).
    #[must_use]
    pub fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_)
            | Self::Show { .. }
            | Self::TranscriptList { .. }
            | Self::TranscriptShow { .. } => None,
            Self::Rebuild(_) => Some(AuditMeta {
                command: REBUILD_COMMAND,
                stepwise: true,
            }),
            Self::Pass(..) | Self::PassPrepare(..) | Self::PassApply(..) | Self::PassFail(..) => {
                Some(AuditMeta {
                    command: PASS_COMMAND,
                    stepwise: false,
                })
            }
            Self::Deactivate(_) => Some(AuditMeta {
                command: DEACTIVATE_COMMAND,
                stepwise: false,
            }),
            Self::Activate(_) => Some(AuditMeta {
                command: ACTIVATE_COMMAND,
                stepwise: false,
            }),
            Self::Catalog(_) => Some(AuditMeta {
                command: CATALOG_COMMAND,
                stepwise: true,
            }),
        }
    }

    /// The record and agent this command names, resolved before it runs so
    /// a refusal is attributable too. Exhaustive like [`Command::audit`].
    #[must_use]
    pub fn subject(&self, ctx: &Context) -> AuditSubject {
        match self {
            Self::List(_)
            | Self::Show { .. }
            | Self::Pass(..)
            | Self::PassPrepare(..)
            | Self::PassApply(..)
            | Self::PassFail(..)
            | Self::TranscriptList { .. }
            | Self::TranscriptShow { .. } => AuditSubject::default(),
            // A prefix is resolved before the line is written; an id that
            // resolves to no record leaves the target null, never the typed id.
            Self::Rebuild(input) => AuditSubject {
                target: resolved_id(ctx, input.id).map(|id| format!("session:{id}")),
                author: Some(input.author.as_str().to_owned()),
            },
            Self::Catalog(input) => AuditSubject {
                target: resolve_transcript(ctx, input.provider, input.id)
                    .ok()
                    .map(|id| format!("session:{id}")),
                author: Some(input.author.as_str().to_owned()),
            },
            Self::Deactivate(input) => activity_subject(ctx, &input.target, &input.author),
            Self::Activate(input) => activity_subject(ctx, &input.target, &input.author),
        }
    }
}

/// An activity verb's subject: the session a whole-session reference
/// resolves to, and the author. A segment reference names no target.
fn activity_subject(ctx: &Context, id: &SessionRef, author: &AuthorName) -> AuditSubject {
    let target = id
        .segment
        .is_none()
        .then(|| id.id.clone())
        .and_then(|id| resolved_id(ctx, &id))
        .map(|id| format!("session:{id}"));
    AuditSubject {
        target,
        author: Some(author.as_str().to_owned()),
    }
}

/// What a `session` or `transcript` command returns.
pub enum Outcome {
    /// What `list` found.
    Listed(Box<Listed>),
    /// What `show` read.
    Shown(Box<Shown>),
    /// What a `pass`, `catalog` or `rebuild` did.
    Passed(Box<PassReport>),
    /// A prepared pass window or model-free settlement.
    PassPrepared(Box<Prepared>),
    /// A pass apply or retry outcome.
    PassApplied(Box<ApplyOutput>),
    /// Backoff for the active session, if its basis is still current.
    PassFailed(Option<Box<PassPayload>>),
    /// The record an `activate` or `deactivate` left.
    Activity(Box<ActivityWritten>),
    /// The uncataloged transcripts; `None` when there is no transcripts
    /// directory.
    Transcripts(Option<Box<TranscriptsListed>>),
    /// One transcript.
    Transcript(Box<Surveyed>),
}

/// Why a `session` or `transcript` command did not run.
#[derive(Debug)]
pub enum Error<E> {
    /// A read's error (`list`, `show`).
    Read(ReadError<E>),
    /// A record write's error (`activate`, `deactivate`).
    Write(WriteError<E>),
    /// A `pass`'s error.
    Pass(PassRunError<E>),
    /// A `catalog`'s or a transcript read's error.
    Catalog(CatalogError<E>),
    /// A `rebuild`'s error.
    Rebuild(RebuildError<E>),
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

impl<E> From<PassRunError<E>> for Error<E> {
    fn from(err: PassRunError<E>) -> Self {
        Self::Pass(err)
    }
}

impl<E> From<CatalogError<E>> for Error<E> {
    fn from(err: CatalogError<E>) -> Self {
        Self::Catalog(err)
    }
}

impl<E> From<RebuildError<E>> for Error<E> {
    fn from(err: RebuildError<E>) -> Self {
        Self::Rebuild(err)
    }
}

/// Run `command` against `ctx`, inside the audit guard when
/// [`Command::audit`] declares one.
///
/// # Errors
///
/// [`Error`]: the flow's own error, as it was before the app moved.
pub fn exec(ctx: &Context, command: Command<'_>) -> Result<Outcome, CommandError> {
    let audit = Audit::declared(command.audit(), || command.subject(ctx));
    guarded(ctx, audit, || Ok(run(&ctx.core(), command)?))
}

/// Run `command` against `ctx` with no audit line, as the binary's catalog
/// and pass tests drive it.
///
/// # Errors
///
/// As [`exec`].
#[cfg(any(test, feature = "test-support"))]
pub fn exec_unaudited(ctx: &Context, command: Command<'_>) -> Result<Outcome, CommandError> {
    guarded(ctx, None, || Ok(run(&ctx.core(), command)?))
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    command: Command<'_>,
) -> Result<Outcome, Error<E>> {
    Ok(match command {
        Command::List(input) => Outcome::Listed(Box::new(list::list(ctx, *input)?)),
        Command::Show { id, segment } => Outcome::Shown(Box::new(show::show(ctx, &id, segment)?)),
        Command::Pass(input, host) => {
            Outcome::Passed(Box::new(pass::settle_only(ctx, &host, input)?))
        }
        Command::PassPrepare(input, host) => {
            Outcome::PassPrepared(Box::new(pass::prepare(ctx, &host, input)?))
        }
        Command::PassApply(input, host) => {
            Outcome::PassApplied(Box::new(pass::apply(ctx, &host, *input)?))
        }
        Command::PassFail(input, host) => {
            Outcome::PassFailed(pass::fail(ctx, &host, *input)?.map(Box::new))
        }
        Command::Catalog(input) => {
            let CatalogInput {
                id,
                author,
                project,
                provider,
                host,
                models,
            } = input;
            let input = CatalogInput {
                id,
                author,
                project,
                provider,
                host,
                models: |config: &Config| Adapter(models(config)),
            };
            Outcome::Passed(Box::new(catalog::run(ctx, input)?))
        }
        Command::Rebuild(input) => {
            let RebuildInput {
                id,
                author,
                confirm,
                models,
            } = input;
            let input = RebuildInput {
                id,
                author,
                confirm,
                models: |config: &Config| Adapter(models(config)),
            };
            Outcome::Passed(Box::new(rebuild::run(ctx, input)?))
        }
        Command::Deactivate(input) => {
            Outcome::Activity(Box::new(activity::deactivate(ctx, input)?))
        }
        Command::Activate(input) => Outcome::Activity(Box::new(activity::activate(ctx, input)?)),
        Command::TranscriptList { provider, input } => {
            Outcome::Transcripts(transcript::list(ctx, provider, &input)?.map(Box::new))
        }
        Command::TranscriptShow { provider, id } => {
            Outcome::Transcript(Box::new(transcript::show(ctx, provider, &id)?))
        }
    })
}

/// The boxed adapter a command carries, as the flows' `M: Models`. Every
/// method forwards, so an adapter's own answers (a test's fake) are kept.
pub(crate) struct Adapter<'i>(Box<dyn Models + 'i>);

impl Models for Adapter<'_> {
    fn session(&self, request: &SessionRequest) -> Result<SessionAnswer, ModelError> {
        self.0.session(request)
    }

    fn session_header(&self, request: &SessionHeaderRequest) -> Result<SessionHeader, ModelError> {
        self.0.session_header(request)
    }

    fn group_summary(&self, model: &str, evidence: &CloseEvidence) -> Result<String, ModelError> {
        self.0.group_summary(model, evidence)
    }

    fn profile(&self, request: &ProfileRequest) -> Result<ProfileAnswer, ModelError> {
        self.0.profile(request)
    }
}

/// The session `id` (an id or unique prefix) resolves to, or `None` when it
/// does not: the audit subject's target, which [`Command::subject`] names
/// before the command runs.
#[must_use]
pub fn resolved_id(ctx: &Context, id: &SessionId) -> Option<SessionId> {
    ctx.store().read().sessions().resolve(id).ok()
}

/// The transcript `input` names, resolved against the `provider`
/// transcripts on this host. Reads only: `session catalog`'s audit
/// target, which [`Command::subject`] names before the command runs.
///
/// # Errors
///
/// An unknown or ambiguous id.
pub fn resolve_transcript(
    ctx: &Context,
    provider: Provider,
    input: &SessionId,
) -> Result<SessionId, CatalogError<CommandError>> {
    catalog::resolve_transcript(&ctx.core(), provider, input)
}

/// Begin a catalog of `input` by `by`: its switch, whether this run wrote
/// it, and the bytes. `/scribe catalog <id>` runs this.
///
/// # Errors
///
/// [`CatalogError`]: a refusal, the transcript, or the store.
pub(crate) fn begin<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    input: &SessionId,
    by: Author,
) -> Result<Begun, CatalogError<E>> {
    catalog::begin(ctx, provider, input, by)
}

/// [`begin`] for the binary's tests.
///
/// # Errors
///
/// As [`begin`].
#[cfg(any(test, feature = "test-support"))]
pub fn begin_catalog(
    ctx: &Context,
    provider: Provider,
    input: &SessionId,
    by: Author,
) -> Result<Begun, CommandError> {
    Ok(catalog::begin(&ctx.core(), provider, input, by)?)
}

/// Link `session` to another, under the store lock, stamping `provider`.
///
/// # Errors
///
/// A refused link, or a store error.
pub(crate) fn link_session_for_provider(
    store: &Store,
    session: &SessionId,
    link: &SessionLink,
    on: bool,
    now: OffsetDateTime,
    provider: Provider,
) -> crate::Result<control::Linked> {
    verbs::SessionVerbs::link_session_for_provider(store, session, link, on, now, provider)
}

/// Change many sessions' `active` or `keep_active` in one write, for the
/// consistency scan.
///
/// # Errors
///
/// A store error.
pub(crate) fn set_active_many(
    store: &Store,
    requests: &[(SessionId, SetActive)],
    now: OffsetDateTime,
    redactor: &Redactor,
) -> crate::Result<Vec<crate::Result<ActiveOutcome>>> {
    verbs::SessionVerbs::set_active_many(store, requests, now, redactor)
}

/// Continue the handoff `selection` names into `child`: its body and
/// whether this changed anything, for the hooks.
///
/// # Errors
///
/// A refusal, or a store error.
pub(crate) fn continue_handoff(
    store: &Store,
    child: &SessionId,
    selection: HandoffSelection<'_>,
    now: OffsetDateTime,
) -> crate::Result<Option<(String, bool)>> {
    verbs::SessionVerbs::continue_handoff(store, child, selection, now)
}

/// Set one metadata field of a session's record, for `/scribe session`.
///
/// # Errors
///
/// A refused field or value, or a store error.
pub(crate) fn update_session_metadata(
    store: &Store,
    id: &SessionId,
    field: &str,
    value: &str,
    redactor: &Redactor,
) -> crate::Result<()> {
    verbs::SessionVerbs::update_session_metadata(store, id, field, value, redactor)
}

/// `scribe session` and `scribe transcript`'s errors, each the variant it was
/// before the app moved.
impl From<Error<crate::runtime::errors::CommandError>> for crate::runtime::errors::CommandError {
    fn from(err: Error<crate::runtime::errors::CommandError>) -> Self {
        match err {
            Error::Read(e) => e.into(),
            Error::Write(e) => e.into(),
            Error::Pass(e) => e.into(),
            Error::Catalog(e) => e.into(),
            Error::Rebuild(e) => e.into(),
        }
    }
}

/// A pass, catalog or rebuild writes one line per session passed (see
/// [`PassReport`]'s lines); an activity write names its session when it
/// changed it.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        match self {
            Self::Passed(report) => pass_lines(report),
            Self::PassPrepared(prepared) => prepared.settled.iter().filter_map(pass_line).collect(),
            Self::PassApplied(applied) => applied
                .pass
                .as_ref()
                .and_then(pass_line)
                .into_iter()
                .chain(
                    applied
                        .persisted_targets
                        .iter()
                        .chain(&applied.memory_targets)
                        .map(|id| Line::success(Some(id.clone()))),
                )
                .collect(),
            Self::PassFailed(failure) => {
                failure.as_deref().and_then(pass_line).into_iter().collect()
            }
            Self::Activity(written) => Line::written(
                Some(format!("session:{}", written.view.record.id)),
                written.changed,
            ),
            // Reads: never audited (STD-01 R31).
            Self::Listed(_) | Self::Shown(_) | Self::Transcripts(_) | Self::Transcript(_) => {
                vec![Line::success(None)]
            }
        }
    }
}

fn pass_line(pass: &PassPayload) -> Option<Line> {
    if pass.result == "waiting" {
        return None;
    }
    let target = Some(format!("session:{}", pass.session));
    Some(match pass.error_code {
        Some(code) => Line::failure(target, code),
        None => Line::success(target),
    })
}

/// One line per settled or failed pass and per persisted target.
fn pass_lines(report: &PassReport) -> Vec<Line> {
    let mut lines: Vec<Line> = report.passes.iter().filter_map(pass_line).collect();
    let failed = lines
        .iter()
        .any(|line| line.status == crate::AuditStatus::Failure);
    if report.switch_written && !failed {
        let target = report
            .passes
            .first()
            .map(|p| format!("session:{}", p.session));
        lines.insert(0, Line::success(target));
    }
    for id in report
        .persisted_targets
        .iter()
        .chain(&report.memory_targets)
    {
        lines.push(Line::success(Some(id.clone())));
    }
    lines
}
