//! `scribe session catalog` (ORB-27707 step 3): switch one past session on
//! with `cataloged_by` and run every window of its transcript.

use crate::cases::pass::{CatalogJob, PassReport, RunFailure, run_catalog};
use crate::control::{self, CatalogGate};
use crate::runtime::resolve::refuse_forgotten;
use crate::runtime::{Ctx, PortError, Unreadable};
use crate::{
    Author, AuthorName, Config, Label, Models, Project, Provider, SessionControl, SessionFilter,
    SessionId, check_agent_author, transcript,
};
use std::path::PathBuf;

/// What `session catalog` runs.
pub struct CatalogInput<'i, F> {
    /// The session, or a prefix of its id.
    pub id: &'i SessionId,
    /// The agent cataloging it.
    pub author: &'i AuthorName,
    /// The project of the first record (`--project`); else attributed.
    pub project: Option<Project>,
    /// The provider whose transcript it is.
    pub provider: Provider,
    /// This host, as records name it.
    pub host: Label,
    /// The `Models` adapter for the session's config.
    pub models: F,
}

/// Why `session catalog` did not run.
#[derive(Debug)]
pub enum CatalogError<E> {
    /// A store error.
    Core(crate::Error),
    /// A port refused or failed.
    Port(E),
    /// No transcript of that session in `dir`.
    TranscriptNotFound {
        /// The session named.
        id: SessionId,
        /// The transcripts directory looked in.
        dir: PathBuf,
    },
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
    /// The session already has a record or a `/scribe on` switch.
    SessionCataloged(SessionId),
    /// A usage error.
    Usage(String),
}

impl<E> From<crate::Error> for CatalogError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

impl<E> From<RunFailure<E>> for CatalogError<E> {
    fn from(failure: RunFailure<E>) -> Self {
        match failure {
            RunFailure::Core(err) => Self::Core(err),
            RunFailure::Port(err) => Self::Port(err),
        }
    }
}

/// A catalog begun: its switch, whether this run wrote it, and the
/// transcript's bytes.
pub struct Begun {
    /// The session's full id.
    pub id: SessionId,
    /// Its catalog switch.
    pub switch: SessionControl,
    /// Whether this run switched it on.
    pub wrote_switch: bool,
    /// The transcript, as checked and read.
    pub bytes: Vec<u8>,
}

/// Run `session catalog`: begin the catalog, then run every window.
///
/// # Errors
///
/// [`CatalogError`]. A failed window is a `failed` payload, not an error.
pub fn run<E: PortError, M: Models>(
    ctx: &Ctx<'_, E>,
    input: CatalogInput<'_, impl FnOnce(&Config) -> M>,
) -> Result<PassReport, CatalogError<E>> {
    let CatalogInput {
        id,
        author,
        project,
        provider,
        host,
        models,
    } = input;
    check_agent_author(author)?;
    let begun = begin(ctx, provider, id, Author::agent(author.clone()))?;
    let config = ctx.store.read().tags().config()?.for_provider(provider);
    let project = project.unwrap_or_else(|| {
        crate::cases::pass::attributed_project(&begun.bytes, &config, |cwd| {
            ctx.transcripts.project_of(cwd)
        })
    });
    let ended = transcript::survey(&begun.bytes)
        .ended
        .is_some_and(|last| transcript::is_day_old(last, ctx.clock.now()));
    let mut report = run_catalog(
        ctx,
        models,
        CatalogJob {
            provider,
            host: &host,
            id: &begun.id,
            switch: &begun.switch,
            project,
            ended,
            author: author.as_str(),
        },
    )?;
    report.switch_written = begun.wrote_switch;
    Ok(report)
}

/// Resolve `input` to a transcript on this host: an exact file name, else a
/// session id the store resolves among them.
///
/// # Errors
///
/// `TranscriptNotFound` when there is no transcripts directory or no such
/// transcript; a store or port error.
pub fn resolve_transcript<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    input: &SessionId,
) -> Result<SessionId, CatalogError<E>> {
    let Some(dir) = ctx.transcripts.dir() else {
        return Err(CatalogError::TranscriptNotFound {
            id: input.clone(),
            dir: PathBuf::from("(no transcripts directory)"),
        });
    };
    let files = ctx
        .transcripts
        .files(provider)
        .map_err(CatalogError::Port)?;
    let ids: Vec<String> = files.into_iter().map(|(id, _)| id.to_string()).collect();
    // An exact local filename already identifies the transcript. Keep this
    // direct read independent of catalog metadata such as group rosters; only
    // aliases need store resolution.
    let resolved = if ids.iter().any(|id| id == input.as_str()) {
        input.clone()
    } else {
        ctx.store
            .read()
            .sessions()
            .resolve_or_typed_with(input, &ids)?
    };
    if ids.iter().any(|id| id == resolved.as_str()) {
        Ok(resolved)
    } else {
        Err(CatalogError::TranscriptNotFound {
            id: input.clone(),
            dir,
        })
    }
}

/// Begin a catalog of `input` by `by`: resolve and read its transcript,
/// then write its catalog switch, or resume the one a failed catalog left.
///
/// # Errors
///
/// [`CatalogError`].
pub fn begin<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    input: &SessionId,
    by: Author,
) -> Result<Begun, CatalogError<E>> {
    let id = resolve_transcript(ctx, provider, input)?;
    refuse_forgotten(ctx.store, &id)?;
    let dir = ctx
        .transcripts
        .dir()
        .ok_or_else(|| CatalogError::TranscriptNotFound {
            id: id.clone(),
            dir: PathBuf::from("(no transcripts directory)"),
        })?;
    let path = ctx
        .transcripts
        .files(provider)
        .map_err(CatalogError::Port)?
        .into_iter()
        .find(|(found, _)| *found == id)
        .map(|(_, path)| path)
        .ok_or_else(|| CatalogError::TranscriptNotFound {
            id: id.clone(),
            dir: dir.clone(),
        })?;
    let bytes = match ctx.transcripts.read_checked(provider, &path, &id) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Err(CatalogError::TranscriptNotFound { id, dir }),
        Err(Unreadable::Refused(reason)) => {
            return Err(CatalogError::TranscriptRefused { id, reason });
        }
        Err(Unreadable::Io(source)) => {
            return Err(CatalogError::Read {
                what: path.display().to_string(),
                source,
            });
        }
    };
    let store = ctx.store;
    let config = store.read().tags().config()?.for_provider(provider);
    let _redactor = ctx
        .redaction
        .redactor(&config)
        .map_err(CatalogError::Port)?;
    let (switch, wrote_switch) = store.write(|tx| {
        tx.sessions()
            .change_control(|state| -> Result<_, CatalogError<E>> {
                let has_record = recorded(ctx, &id)?;
                match control::catalog_gate(state, &id, has_record) {
                    CatalogGate::Refused => Err(CatalogError::SessionCataloged(id.clone())),
                    CatalogGate::Resume(switch) => {
                        if switch.provider != provider {
                            return Err(CatalogError::Usage(String::from(
                                "catalog switch belongs to another provider",
                            )));
                        }
                        Ok((switch, false))
                    }
                    CatalogGate::Start => {
                        control::catalog_on(state, &id, by, ctx.clock.now());
                        let switch = state.sessions.get_mut(&id).ok_or_else(|| {
                            CatalogError::Core(crate::Error::SessionNotFound {
                                id: id.clone(),
                                path: store.root().join("control.json"),
                            })
                        })?;
                        switch.provider = provider;
                        Ok((switch.clone(), true))
                    }
                }
            })
    })??;
    Ok(Begun {
        id,
        switch,
        wrote_switch,
        bytes,
    })
}

/// Whether `id` has a session record, readable or not.
fn recorded<E>(ctx: &Ctx<'_, E>, id: &SessionId) -> Result<bool, crate::Error> {
    let found = ctx.store.read().sessions().list(&SessionFilter {
        include_inactive: true,
        ..Default::default()
    })?;
    let parsed = found.listing.items.iter().any(|record| &record.id == id);
    let skipped = found.skipped.iter().any(|skipped| {
        skipped.path.file_stem().and_then(|stem| stem.to_str()) == Some(id.as_str())
    });
    Ok(parsed || skipped)
}
