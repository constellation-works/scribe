//! The `group` app: session groups (`scribe group`; 2_design.md §17) —
//! opening, joining, leaving, backfilling and closing a group, reading one
//! or all, and the member turns the hooks capture.
//!
//! The flows share the caller's transcript reads below. The hooks reach the
//! membership verbs and the turn capture through the functions this module
//! exports.

mod backfill;
mod close;
mod join;
mod leave;
mod open;
pub mod rules;
#[cfg(test)]
mod tests;
mod turns;
mod verbs;

pub use backfill::{BackfilledMember, GroupBackfilled};
pub use close::{CLOSE_COMMAND, GroupCloseInput, GroupClosed};
pub use join::{GroupJoinInput, GroupJoined};
pub use open::{GroupOpenInput, GroupOpened};
pub use turns::{claude as claude_turns, codex as codex_turns, past as past_turns};
#[cfg(any(test, feature = "test-support"))]
pub use verbs::{CloseGroupOutcome, GroupVerbs};

use crate::apps::tag::ConfigReads;
use crate::composition::Context;
use crate::group::{GroupAdded, GroupAddition, SeatStart};
use crate::message::NewMessage;
use crate::models::ModelError;
use crate::runtime::audit::HookOutcome;
use crate::runtime::audit::{Audit, AuditMeta, AuditSubject, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::runtime::{Ctx, PortError, Unreadable, resolve};
use crate::{Project, Provider, Redactor, Related, SessionId, Store, transcript};
use Error as GroupError;
use std::path::PathBuf;
use time::OffsetDateTime;
use turns as group_turns;

/// A `group` command.
pub enum Command<'i> {
    /// `group open`.
    Open(GroupOpenInput<'i>),
    /// `group join`.
    Join(GroupJoinInput),
    /// `group backfill`: the group (an id, prefix or slug) and the provider.
    Backfill {
        /// Group id, unique prefix, or project group slug.
        id: String,
        /// The provider whose sessions' transcripts are read.
        provider: Provider,
    },
    /// `group leave`.
    Leave {
        /// The calling session, from the provider's session variable;
        /// `None` refuses the command as a usage error.
        session: Option<SessionId>,
        /// The provider whose session calls.
        provider: Provider,
    },
    /// `group close`.
    Close(GroupCloseInput<'i>),
    /// Read bounded evidence for an authorized closer.
    Evidence {
        /// The calling session.
        session: Option<SessionId>,
        /// The calling provider.
        provider: Provider,
        /// The group id or alias.
        id: String,
        /// Optional verified user consent.
        quote: Option<String>,
    },
    /// `group show`: the group (an id, prefix or slug).
    Show(String),
    /// `group list`.
    List,
}

impl Command<'_> {
    /// What the audit log records for this command, or `None` when it is
    /// not audited: a read writes nothing, not even an audit line (STD-01
    /// R31). Exhaustive, so a new command cannot compile without deciding
    /// (STD-02 R27).
    #[must_use]
    pub fn audit(&self) -> Option<AuditMeta> {
        let command = match self {
            Self::Open(_) => "group open",
            Self::Join(_) => "group join",
            Self::Leave { .. } => "group leave",
            Self::Backfill { .. } => "group backfill",
            Self::Close(_) => CLOSE_COMMAND,
            Self::List | Self::Show(_) | Self::Evidence { .. } => return None,
        };
        Some(AuditMeta {
            command,
            stepwise: matches!(self, Self::Close(_)),
        })
    }

    /// The record and agent this command names, resolved before it runs so
    /// a refusal is attributable too. Exhaustive like [`Command::audit`].
    #[must_use]
    pub fn subject(&self, ctx: &Context) -> AuditSubject {
        match self {
            Self::Join(input) => AuditSubject {
                author: Some(input.author.to_string()),
                target: resolved_id(ctx, &input.id),
            },
            Self::Close(GroupCloseInput { id, .. }) | Self::Backfill { id, .. } => AuditSubject {
                author: None,
                target: resolved_id(ctx, id),
            },
            Self::Open(_)
            | Self::Leave { .. }
            | Self::List
            | Self::Show(_)
            | Self::Evidence { .. } => AuditSubject::default(),
        }
    }
}

/// What a `group` command returns.
pub enum Outcome {
    /// The group as read after an `open`, `join`, `leave` or `close`.
    Written(Box<Written>),
    /// What a `backfill` stored.
    Backfilled(Box<GroupBackfilled>),
    /// Bounded group close evidence.
    Evidence(serde_json::Value),
    /// The group `show` read.
    Shown(Box<crate::group::Group>),
    /// Every group, newest first.
    Listed(Vec<crate::group::Group>),
}

/// The group a write left, as read after it.
pub struct Written {
    /// The group.
    pub group: crate::group::Group,
    /// Whether the command changed the store.
    pub changed: bool,
    /// Notices for the user.
    pub notices: Vec<String>,
}

/// Why a `scribe group` command did not run.
#[derive(Debug)]
pub enum Error<E> {
    /// A store error.
    Core(crate::Error),
    /// A port refused or failed.
    Port(E),
    /// A transcript could not be read.
    Read {
        /// The file.
        what: String,
        /// The read error.
        source: std::io::Error,
    },
    /// A transcript failed its provenance check.
    TranscriptRefused {
        /// The session.
        id: SessionId,
        /// Why.
        reason: String,
    },
    /// A usage error.
    Usage(String),
    /// The model call failed.
    Model(ModelError),
}

impl<E> From<crate::Error> for Error<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

impl<E> From<ModelError> for Error<E> {
    fn from(err: ModelError) -> Self {
        Self::Model(err)
    }
}

impl<E> From<HookError<E>> for Error<E> {
    fn from(err: HookError<E>) -> Self {
        match err {
            HookError::Core(err) => Self::Core(err),
            HookError::Port(err) => Self::Port(err),
            HookError::Read { what, source } => Self::Read { what, source },
            HookError::TranscriptRefused { id, reason } => Self::TranscriptRefused { id, reason },
            HookError::Usage(message) => Self::Usage(message),
        }
    }
}

/// Why a hook use case failed: the error the turn capture returns, and the
/// hooks above it.
#[derive(Debug)]
pub enum HookError<E> {
    /// A store error.
    Core(crate::Error),
    /// A port refused or failed.
    Port(E),
    /// The transcript could not be read.
    Read {
        /// The file.
        what: String,
        /// The read error.
        source: std::io::Error,
    },
    /// The transcript failed its provenance check.
    TranscriptRefused {
        /// The session.
        id: SessionId,
        /// Why.
        reason: String,
    },
    /// A usage error.
    Usage(String),
}

impl<E> From<crate::Error> for HookError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

/// A refusal outcome against `session`.
pub(crate) fn refusal(session: &SessionId, code: &'static str) -> HookOutcome {
    HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: Some(code),
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

/// Run `command` against `ctx`: dispatch only.
/// [`past_turns`] against `ctx` for its provider: the typed turns of a past
/// session whose transcript is at `path`, as `/scribe group add` reads a
/// named session's.
///
/// # Errors
///
/// A refused or unreadable transcript.
pub fn past_turns_of(
    ctx: &Context,
    session: &SessionId,
    path: &std::path::Path,
) -> Result<Option<Vec<NewMessage>>, CommandError> {
    Ok(past_turns(&ctx.core(), ctx.provider(), session, path)?)
}

/// The calling session, or the usage error that names the variable the
/// command should have been run with.
pub(crate) fn calling<E>(
    session: Option<SessionId>,
    provider: Provider,
) -> Result<SessionId, Error<E>> {
    session.ok_or_else(|| {
        Error::Usage(format!(
            "run this command in the calling session with a valid {}",
            crate::composition::caller_env(provider)
        ))
    })
}

pub(crate) fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    command: Command<'_>,
) -> Result<Outcome, Error<E>> {
    let (id, changed, notices) = match command {
        Command::Open(input) => {
            let opened = open::run(ctx, input)?;
            (opened.id, true, opened.notices)
        }
        Command::Join(input) => {
            let joined = join::run(ctx, input)?;
            (joined.id, joined.changed, joined.notices)
        }
        Command::Backfill { id, provider } => {
            return Ok(Outcome::Backfilled(Box::new(backfill::run(
                ctx, &id, provider,
            )?)));
        }
        Command::Leave { session, provider } => {
            let session = calling(session, provider)?;
            (leave::run(ctx, &session)?, true, Vec::new())
        }
        Command::Close(input) => {
            let closed = close::run(ctx, input)?;
            (closed.id, closed.changed, closed.notices)
        }
        Command::Evidence {
            session,
            provider,
            id,
            quote,
        } => {
            return Ok(Outcome::Evidence(close::evidence(
                ctx, session, provider, id, quote,
            )?));
        }
        Command::Show(id) => {
            return Ok(Outcome::Shown(Box::new(
                ctx.store.read().groups().get(&id)?,
            )));
        }
        Command::List => return Ok(Outcome::Listed(ctx.store.read().groups().all()?)),
    };
    Ok(Outcome::Written(Box::new(Written {
        group: ctx.store.read().groups().get(&id)?,
        changed,
        notices,
    })))
}

/// The group `id` (an id, prefix or slug) resolves to, or `None` when it does
/// not: the audit subject's target, which [`Command::subject`] names before the
/// command runs.
#[must_use]
pub fn resolved_id(ctx: &Context, id: &str) -> Option<String> {
    ctx.store().read().groups().get(id).ok().map(|g| g.id)
}

/// Create or join an open project group on a user-typed hook command
/// ([`verbs::GroupVerbs::join_project_group`]).
pub(crate) fn join_project_group(
    store: &Store,
    slug: &str,
    project: &Project,
    session: &SessionId,
    now: OffsetDateTime,
    redactor: &Redactor,
) -> crate::Result<String> {
    verbs::GroupVerbs::join_project_group(store, slug, project, session, now, redactor)
}

/// Add named past sessions to an open project group
/// ([`verbs::GroupVerbs::add_project_group_sessions`]).
pub(crate) fn add_project_group_sessions(
    store: &Store,
    slug: &str,
    project: &Project,
    opener: &SessionId,
    additions: &[GroupAddition],
    now: OffsetDateTime,
    redactor: &Redactor,
) -> crate::Result<GroupAdded> {
    verbs::GroupVerbs::add_project_group_sessions(
        store, slug, project, opener, additions, now, redactor,
    )
}

/// Remove only the caller from its chain ([`verbs::GroupVerbs::leave_group`]).
pub(crate) fn leave_group(store: &Store, session: &SessionId) -> crate::Result<String> {
    verbs::GroupVerbs::leave_group(store, session)
}

/// Move a member atomically ([`verbs::GroupVerbs::move_project_group`]).
pub(crate) fn move_project_group(
    store: &Store,
    session: &SessionId,
    slug: &str,
    project: &Project,
    now: OffsetDateTime,
    redactor: &Redactor,
) -> crate::Result<String> {
    verbs::GroupVerbs::move_project_group(store, session, slug, project, now, redactor)
}

/// Record that a start could not inspect its transcript yet
/// ([`verbs::GroupVerbs::defer_ancestry`]).
pub(crate) fn defer_ancestry(store: &Store, session: &SessionId) -> crate::Result<bool> {
    verbs::GroupVerbs::defer_ancestry(store, session)
}

/// Whether a deferred ancestry check is pending
/// ([`verbs::GroupVerbs::has_deferred_ancestry`]).
pub(crate) fn has_deferred_ancestry(store: &Store, session: &SessionId) -> crate::Result<bool> {
    verbs::GroupVerbs::has_deferred_ancestry(store, session)
}

/// Consume a deferred check once ([`verbs::GroupVerbs::take_deferred_ancestry`]).
pub(crate) fn take_deferred_ancestry(store: &Store, session: &SessionId) -> crate::Result<bool> {
    verbs::GroupVerbs::take_deferred_ancestry(store, session)
}

/// Inherit the predecessor's open seat for a newly cleared session
/// ([`verbs::GroupVerbs::inherit_seat`]).
pub(crate) fn inherit_seat(
    store: &Store,
    child: &SessionId,
    predecessor: Option<&SessionId>,
) -> crate::Result<SeatStart> {
    verbs::GroupVerbs::inherit_seat(store, child, predecessor)
}

/// `session`'s `provider` transcript ([`crate::runtime::Transcripts::files`]).
pub(crate) fn locate<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
) -> Result<Option<PathBuf>, GroupError<E>> {
    Ok(ctx
        .transcripts
        .files(provider)
        .map_err(GroupError::Port)?
        .into_iter()
        .find(|(found, _)| found == session)
        .map(|(_, path)| path))
}

/// The redactor for the provider's config.
pub(crate) fn redactor<E>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
) -> Result<crate::Redactor, GroupError<E>> {
    let config = ctx.store.read().tags().config()?.for_provider(provider);
    ctx.redaction.redactor(&config).map_err(GroupError::Port)
}

/// The typed turns in an already checked transcript
/// ([`group_turns::typed_turns`]).
pub(crate) fn typed_turns<E>(
    provider: Provider,
    session: &SessionId,
    bytes: &[u8],
) -> Result<Vec<NewMessage>, GroupError<E>> {
    Ok(group_turns::typed_turns::<E>(provider, session, bytes)?)
}

/// Missing or unverifiable evidence leaves the seat occupied. A predecessor
/// can be from either provider in this host's store. Only the predecessor's
/// own transcript is checked: another session's duplicate id elsewhere in the
/// tree is not evidence about it, while a duplicate of the predecessor's own
/// id still refuses.
pub(crate) fn ended<E: PortError>(
    ctx: &Ctx<'_, E>,
    session: &SessionId,
) -> Result<bool, GroupError<E>> {
    let mut found = false;
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        if let Some(path) = ctx
            .transcripts
            .locate_own(provider, session)
            .map_err(GroupError::Port)?
        {
            found = true;
            let old = ctx
                .transcripts
                .read_checked(provider, &path, session)
                .ok()
                .flatten()
                .and_then(|bytes| transcript::survey(&bytes).ended)
                .is_some_and(|last| transcript::is_day_old(last, ctx.clock.now()));
            if !old {
                return Ok(false);
            }
        }
    }
    Ok(found)
}

/// The caller's typed turns from the start of its own transcript, for the
/// group it joins. A join does not need the transcript: one that is missing or
/// fails its check joins with no earlier turns, as it always has.
pub(crate) fn earlier_turns<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
) -> Result<Vec<NewMessage>, GroupError<E>> {
    let Some(path) = ctx
        .transcripts
        .locate_own(provider, session)
        .map_err(GroupError::Port)?
    else {
        return Ok(Vec::new());
    };
    match ctx.transcripts.read_checked(provider, &path, session) {
        Ok(Some(bytes)) => typed_turns(provider, session, &bytes),
        Ok(None) => Ok(Vec::new()),
        Err(Unreadable::Refused(reason)) => {
            tracing::warn!(%session, %reason, "group join stores no earlier turns: transcript refused");
            Ok(Vec::new())
        }
        Err(Unreadable::Io(source)) => Err(GroupError::Read {
            what: path.display().to_string(),
            source,
        }),
    }
}

/// The caller's transcript bytes once the provenance check passes. `Ok(None)`
/// is evidence that is missing or refused, which the caller refuses without a
/// model call. A discovery or read I/O failure is an error, so its diagnostic
/// and `io_error` audit code survive instead of reading as missing evidence.
pub(crate) fn checked_transcript<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, GroupError<E>> {
    let Some(path) = locate(ctx, provider, session)? else {
        return Ok(None);
    };
    match ctx.transcripts.read_checked(provider, &path, session) {
        Ok(bytes) => Ok(bytes),
        Err(Unreadable::Refused(_)) => Ok(None),
        Err(Unreadable::Io(source)) => Err(GroupError::Read {
            what: path.display().to_string(),
            source,
        }),
    }
}

/// Resolve `session:<id>` references in `related` ([`resolve::relateds`]).
pub(crate) fn resolve_relateds<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    related: &[Related],
) -> Result<Vec<Related>, GroupError<E>> {
    Ok(resolve::relateds(ctx, provider, related)?)
}

/// The hooks' errors (`hook stop`, the marker catalog, group turn capture,
/// the prompt hook and `/scribe group add`), each the variant it was before
/// the flow moved (ORB-27707 step 3b).
impl From<HookError<crate::runtime::errors::CommandError>>
    for crate::runtime::errors::CommandError
{
    fn from(err: HookError<crate::runtime::errors::CommandError>) -> Self {
        match err {
            HookError::Core(e) => Self::Core(e),
            HookError::Port(e) => e,
            HookError::Read { what, source } => Self::Read { what, source },
            HookError::TranscriptRefused { id, reason } => Self::TranscriptRefused { id, reason },
            HookError::Usage(message) => Self::Usage(message),
        }
    }
}

/// `scribe group`'s errors, each the variant it was before the flow moved.
impl From<Error<crate::runtime::errors::CommandError>> for crate::runtime::errors::CommandError {
    fn from(err: Error<crate::runtime::errors::CommandError>) -> Self {
        match err {
            Error::Core(e) => Self::Core(e),
            Error::Port(e) => e,
            Error::Read { what, source } => Self::Read { what, source },
            Error::TranscriptRefused { id, reason } => Self::TranscriptRefused { id, reason },
            Error::Usage(message) => Self::Usage(message),
            Error::Model(e) => Self::Model(e),
        }
    }
}

/// A write names its group when it changed it; a backfill names it when it
/// stored a turn.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        match self {
            Self::Written(written) => {
                Line::written(Some(written.group.id.clone()), written.changed)
            }
            Self::Backfilled(report) => {
                Line::written(Some(report.group.clone()), report.stored != 0)
            }
            Self::Shown(group) => vec![Line::success(Some(group.id.clone()))],
            Self::Listed(_) | Self::Evidence(_) => vec![Line::success(None)],
        }
    }
}
