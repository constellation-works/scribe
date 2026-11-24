//! The `session` app: session records and the passes that write them
//! (`scribe session`, `scribe transcript`; 2_design.md §6, §11, §15).
//!
//! `list`, `show` and the `transcript` reads read only. `pass`, `catalog`
//! and `rebuild` call a model and write records; `activate` and
//! `deactivate` write one record. The links, the activity switch, a
//! continuation's handoff and its predecessor are here too, for the hooks
//! and the consistency scan.

mod activity;
mod catalog;
mod continuation;
mod list;
mod pass;
mod rebuild;
mod show;
mod transcript;
mod verbs;

pub use activity::{
    ActivateInput, ActivityWritten, DeactivateInput, SessionRef, parse_session_ref,
};
pub use catalog::{Begun, CatalogError, CatalogInput};
pub use list::{ListInput, Listed};
pub use pass::{CATALOG_COMMAND, PASS_COMMAND, PassInput, PassPayload, PassReport, PassRunError};
#[cfg(any(test, feature = "test-support"))]
pub use pass::{ClaimsWritten, PassVerbs, short_reason};
#[cfg(any(test, feature = "test-support"))]
pub use rebuild::RebuildVerbs;
pub use rebuild::{REBUILD_COMMAND, RebuildError, RebuildInput};
pub use show::Shown;
pub use transcript::{ListInput as TranscriptListInput, Listed as TranscriptsListed, Surveyed};
pub use verbs::HandoffSelection;
#[cfg(any(test, feature = "test-support"))]
pub use verbs::SessionVerbs;

pub(crate) use continuation::{ClearedPredecessor, cleared_predecessor, try_cleared_predecessor};

use crate::activity::{ActiveOutcome, SetActive};
use crate::models::{
    CloseEvidence, ModelError, ProfileAnswer, ProfileRequest, SessionAnswer, SessionHeader,
    SessionHeaderRequest, SessionRequest,
};
use crate::runtime::errors::{ReadError, WriteError};
use crate::runtime::{Ctx, PortError, Runtime};
use crate::{Author, Config, Models, Provider, Redactor, SessionId, SessionLink, Store, control};
use std::num::NonZeroU32;
use time::OffsetDateTime;

/// The model adapter a `catalog` or `rebuild` builds for the session's
/// config, once.
pub type ModelsOnce<'i> = Box<dyn FnOnce(&Config) -> Box<dyn Models + 'i> + 'i>;

/// The model adapter a `pass` builds for each session's config.
pub type ModelsEach<'i> = &'i mut dyn FnMut(&Config) -> Box<dyn Models + 'i>;

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
    Pass(PassInput<'i, ModelsEach<'i>>),
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

/// What a `session` or `transcript` command returns.
pub enum Outcome {
    /// What `list` found.
    Listed(Box<Listed>),
    /// What `show` read.
    Shown(Box<Shown>),
    /// What a `pass`, `catalog` or `rebuild` did.
    Passed(Box<PassReport>),
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

/// Run `command` against `runtime`.
///
/// # Errors
///
/// [`Error`]: the flow's own error, as it was before the app moved.
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
    Ok(match command {
        Command::List(input) => Outcome::Listed(Box::new(list::list(ctx, *input)?)),
        Command::Show { id, segment } => Outcome::Shown(Box::new(show::show(ctx, &id, segment)?)),
        Command::Pass(input) => {
            let PassInput {
                sessions,
                resolve,
                models,
                limits,
                host,
            } = input;
            let input = PassInput {
                sessions,
                resolve,
                models: |config: &Config| Adapter(models(config)),
                limits,
                host,
            };
            Outcome::Passed(Box::new(pass::run(ctx, input)?))
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

/// `models` as a flow's `impl FnOnce(&Config) -> M`, for a higher app's
/// model caller (`group close`).
pub(crate) fn adapt_once<'i>(models: ModelsOnce<'i>) -> impl FnOnce(&Config) -> Adapter<'i> + 'i {
    move |config: &Config| Adapter(models(config))
}

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

/// The session `id` (an id or unique prefix) resolves to, or `None` when
/// it does not: the audit line's target. Until migration step 3 moves the
/// audit guard into core, the binary resolves it through this.
#[must_use]
pub fn resolved_id<E>(runtime: &Runtime<'_, E>, id: &SessionId) -> Option<SessionId> {
    runtime.ctx().store.read().sessions().resolve(id).ok()
}

/// The transcript `input` names, resolved against the `provider`
/// transcripts on this host. Reads only. Until migration step 3 the
/// binary resolves `session catalog`'s audit target through this.
///
/// # Errors
///
/// An unknown or ambiguous id.
pub fn resolve_transcript<E: PortError>(
    runtime: &Runtime<'_, E>,
    provider: Provider,
    input: &SessionId,
) -> Result<SessionId, CatalogError<E>> {
    catalog::resolve_transcript(&runtime.ctx(), provider, input)
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
pub fn begin_catalog<E: PortError>(
    runtime: &Runtime<'_, E>,
    provider: Provider,
    input: &SessionId,
    by: Author,
) -> Result<Begun, CatalogError<E>> {
    catalog::begin(&runtime.ctx(), provider, input, by)
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
