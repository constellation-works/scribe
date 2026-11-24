//! What the `scribe group` use cases share (ORB-27707 step 3c): their error,
//! and the caller's transcript reads.

use crate::cases::group_turns;
use crate::cases::hook::HookError;
use crate::message::NewMessage;
use crate::models::ModelError;
use crate::runtime::{Ctx, PortError, Unreadable, resolve};
use crate::{Provider, Related, SessionId, transcript};
use std::path::PathBuf;

/// Why a `scribe group` command did not run.
#[derive(Debug)]
pub enum GroupError<E> {
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

impl<E> From<crate::Error> for GroupError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

impl<E> From<ModelError> for GroupError<E> {
    fn from(err: ModelError) -> Self {
        Self::Model(err)
    }
}

impl<E> From<HookError<E>> for GroupError<E> {
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
