//! `scribe mark retry` (ORB-27707 step 3b): open one session's given-up
//! `#scribe` markers, then catalog its transcript's unsettled markers as
//! its `Stop` does ([`crate::cases::annotation`]), with no per-run bound.

use crate::annotation::MarkRecord;
use crate::cases::annotation::{self, AnnotationInput, Cataloged};
use crate::cases::hook::HookError;
use crate::cases::mark::MarkVerbs;
use crate::cases::session::SessionVerbs;
use crate::cases::{Ctx, PortError, TranscriptState};
use crate::{Config, Label, Models, Provider, SessionId};

/// What `mark retry` runs.
pub struct MarkRetryInput<'i, F> {
    /// The session, already resolved.
    pub session: &'i SessionId,
    /// The provider whose transcript it is.
    pub provider: Provider,
    /// This host, as records name it.
    pub host: Label,
    /// Builds the `Models` adapter for the config, only when a call is due.
    pub models: F,
}

/// What a retry did: the markers it opened or recorded, and the echoes.
#[derive(Debug)]
pub struct MarkRetried {
    /// The session's markers written at or after the run's start.
    pub touched: Vec<MarkRecord>,
    /// The catalog's echoes, for the user.
    pub echoes: Vec<String>,
}

/// Why `mark retry` did not finish.
#[derive(Debug)]
pub enum MarkRetryError<E> {
    /// A store error.
    Core(crate::Error),
    /// A port refused or failed.
    Port(E),
    /// The session has no transcript on this host.
    TranscriptMissing(SessionId),
    /// The transcript failed its provenance check.
    TranscriptRefused {
        /// The session.
        id: SessionId,
        /// Why.
        reason: String,
    },
    /// The transcript could not be read.
    Read {
        /// The file.
        what: String,
        /// The read error.
        source: std::io::Error,
    },
    /// Markers still not cataloged: the model failure's message.
    StillFailed(String),
    /// A usage error.
    Usage(String),
}

impl<E> From<crate::Error> for MarkRetryError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

impl<E> From<HookError<E>> for MarkRetryError<E> {
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

/// Open the session's given-up markers, then catalog its transcript's
/// unsettled markers as `Stop` does. The markers this run opened or
/// recorded (written at or after its start) are returned, with the echoes.
///
/// # Errors
///
/// [`MarkRetryError`]; [`MarkRetryError::StillFailed`] when a marker is
/// still not cataloged.
pub fn run<E: PortError, M: Models>(
    ctx: &Ctx<'_, E>,
    input: MarkRetryInput<'_, impl FnOnce(&Config) -> M>,
) -> Result<MarkRetried, MarkRetryError<E>> {
    let MarkRetryInput {
        session,
        provider,
        host,
        models,
    } = input;
    let session = session.clone();
    ctx.store.refuse_forgotten(&session)?;
    let path = ctx
        .transcripts
        .files(provider)
        .map_err(MarkRetryError::Port)?
        .into_iter()
        .find(|(found, _)| *found == session)
        .map(|(_, path)| path)
        .ok_or_else(|| MarkRetryError::TranscriptMissing(session.clone()))?;
    match ctx.transcripts.check(provider, &path, &session) {
        Ok(TranscriptState::Present) => {}
        Ok(TranscriptState::Missing) => return Err(MarkRetryError::TranscriptMissing(session)),
        Err(reason) => {
            return Err(MarkRetryError::TranscriptRefused {
                id: session,
                reason,
            });
        }
    }
    let now = ctx.clock.now();
    ctx.store.reopen_marks(&session, now)?;
    // No hook payload names a cwd here: the transcript's own stands in for
    // a line that does not carry one.
    let cwd = ctx
        .transcripts
        .read(provider, &path)
        .ok()
        .and_then(|bytes| crate::transcript::cwd(&bytes));
    let cataloged: Cataloged = annotation::run(
        ctx,
        AnnotationInput {
            session: &session,
            transcript_path: &path,
            cwd,
            provider,
            host,
            turns: usize::MAX,
            models,
        },
    )?;
    let touched: Vec<_> = ctx
        .store
        .read()
        .marks()
        .of_session(&session)?
        .into_iter()
        .filter(|m| m.at >= now)
        .collect();
    let failed: Vec<&String> = cataloged
        .echoes
        .iter()
        .filter(|echo| echo.starts_with("scribe: could not catalog"))
        .collect();
    if !failed.is_empty() {
        return Err(MarkRetryError::StillFailed(format!(
            "{} #scribe marker(s) of {session} still not cataloged — {}",
            failed.len(),
            failed
                .iter()
                .map(|echo| {
                    echo.trim_start_matches("scribe: could not catalog #scribe in your message — ")
                        .trim_start_matches("the model failed: ")
                })
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    Ok(MarkRetried {
        touched,
        echoes: cataloged.echoes,
    })
}
