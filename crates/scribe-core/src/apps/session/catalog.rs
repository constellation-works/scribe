//! `scribe session catalog` (2_design.md §19.3.3): switch one past session
//! on with `cataloged_by`. Its windows run as the `scribe-session-catalog`
//! Orbit job, which begins with the same switch write.

use super::pass::{CatalogSelection, PassPayload, PassReport};
use crate::apps::tag::ConfigReads;
use crate::control::{self, CatalogGate};
use crate::runtime::resolve::refuse_forgotten;
use crate::runtime::{Ctx, PortError, Unreadable};
use crate::{
    Author, AuthorName, Project, Provider, SessionControl, SessionFilter, SessionId,
    check_agent_author,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// What `session catalog` runs.
pub struct CatalogInput<'i> {
    /// The session, or a prefix of its id.
    pub id: &'i SessionId,
    /// The agent cataloging it.
    pub author: &'i AuthorName,
    /// The project of the first record (`--project`); else attributed.
    pub project: Option<Project>,
    /// The provider whose transcript it is.
    pub provider: Provider,
}

/// The `scribe-session-catalog` job's `begin` input.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogBeginInput {
    /// The session, or a prefix of its id.
    pub session: SessionId,
    /// The agent cataloging it.
    pub author: AuthorName,
    /// The project of the first record; else attributed.
    #[serde(default)]
    pub project: Option<Project>,
    /// The provider whose transcript it is; the host's default when absent.
    #[serde(default)]
    pub provider: Option<Provider>,
}

/// What `begin` hands the job's window loop.
#[derive(Debug, Serialize)]
pub struct CatalogBegun {
    /// The session's full id.
    pub session: SessionId,
    /// The same id as the pass selection's one session.
    pub sessions: Vec<SessionId>,
    /// Whether this call switched it on.
    pub wrote_switch: bool,
    /// The pass selection's catalog mode.
    pub catalog: CatalogSelection,
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

/// Run `session catalog`: begin the catalog, and name the job that runs
/// its windows.
///
/// # Errors
///
/// [`CatalogError`].
pub fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: CatalogInput<'_>,
) -> Result<PassReport, CatalogError<E>> {
    let CatalogInput {
        id,
        author,
        project,
        provider,
    } = input;
    check_agent_author(author)?;
    let begun = begin(ctx, provider, id, Author::agent(author.clone()))?;
    let mut job_input = json!({"session": begun.id, "author": author});
    if let Some(project) = &project {
        job_input["project"] = json!(project);
    }
    Ok(PassReport {
        passes: vec![PassPayload {
            session: begun.id.to_string(),
            result: "waiting",
            error_code: None,
            passes: None,
            dropped_terms: 0,
            windows: 0,
            segments_sealed: 0,
            segments: None,
            author: None,
            gave_up: false,
            error: None,
            warnings: Vec::new(),
            memory_written: 0,
            memory_promoted: 0,
            memory_proposed: 0,
            memory_pending: 0,
            memory_refused: std::collections::BTreeMap::new(),
        }],
        remaining: 1,
        notices: vec![format!(
            "the catalog switch is on; run orbit job run scribe-session-catalog --input '{job_input}' to write its windows"
        )],
        catalog: true,
        switch_written: begun.wrote_switch,
        persisted_targets: Vec::new(),
        memory_targets: Vec::new(),
    })
}

/// The job's `begin`: the same switch write, idempotent when it resumes a
/// catalog switch.
///
/// # Errors
///
/// [`CatalogError`].
pub fn begin_job<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: CatalogBeginInput,
    default_provider: Provider,
) -> Result<CatalogBegun, CatalogError<E>> {
    check_agent_author(&input.author)?;
    let provider = input.provider.unwrap_or(default_provider);
    let begun = begin(
        ctx,
        provider,
        &input.session,
        Author::agent(input.author.clone()),
    )?;
    Ok(CatalogBegun {
        session: begun.id.clone(),
        sessions: vec![begun.id],
        wrote_switch: begun.wrote_switch,
        catalog: CatalogSelection {
            author: input.author,
            project: input.project,
        },
    })
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
    let Some(dir) = ctx.transcripts.dir(provider) else {
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
        .dir(provider)
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
    let found = crate::apps::session::list_sessions(
        &ctx.store.read(),
        &SessionFilter {
            include_inactive: true,
            ..Default::default()
        },
        None,
    )?;
    let parsed = found.listing.items.iter().any(|record| &record.id == id);
    let skipped = found.skipped.iter().any(|skipped| {
        skipped.path.file_stem().and_then(|stem| stem.to_str()) == Some(id.as_str())
    });
    Ok(parsed || skipped)
}

/// `session catalog`'s errors, each the variant it was before the flow moved.
impl From<CatalogError<crate::runtime::errors::CommandError>>
    for crate::runtime::errors::CommandError
{
    fn from(err: CatalogError<crate::runtime::errors::CommandError>) -> Self {
        match err {
            CatalogError::Core(e) => Self::Core(e),
            CatalogError::Port(e) => e,
            CatalogError::TranscriptNotFound { id, dir } => Self::TranscriptNotFound { id, dir },
            CatalogError::TranscriptRefused { id, reason } => {
                Self::TranscriptRefused { id, reason }
            }
            CatalogError::Read { what, source } => Self::Read { what, source },
            CatalogError::SessionCataloged(id) => Self::SessionCataloged(id),
            CatalogError::Usage(message) => Self::Usage(message),
        }
    }
}
