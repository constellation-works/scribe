//! `scribe transcript list|show` (ORB-27707 step 3d): the transcripts on
//! this host with no session record, surveyed without cataloging them.
//! Both read transcripts and the store and write nothing.

use crate::cases::catalog::{self, CatalogError};
use crate::cases::{Ctx, PortError, Unreadable};
use crate::transcript::{self, Survey};
use crate::{Config, Project, Provider, SessionId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One transcript's survey: its project by the pass's rule, and what it
/// holds.
pub struct Surveyed {
    /// The transcript's id.
    pub session: SessionId,
    /// The attributed project.
    pub project: Project,
    /// Mentions of each configured project's paths.
    pub mentions: BTreeMap<Project, u32>,
    /// The transcript's cwd, empty when it names none.
    pub cwd: String,
    /// The survey.
    pub survey: Survey,
}

/// Survey `bytes`, `id`'s transcript, attributing it to a project of
/// `config`.
pub(crate) fn survey<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &SessionId,
    bytes: &[u8],
    config: &Config,
) -> Surveyed {
    let (survey, cwd) = transcript::survey_with_cwd(bytes);
    let cwd = cwd.unwrap_or_default();
    let cwd_project = if cwd.is_empty() {
        Project::sanitized("")
    } else {
        ctx.transcripts.project_of(Path::new(&cwd))
    };
    let mentions = transcript::mention_counts(bytes, &config.projects);
    let project = transcript::attribute(&mentions, &cwd_project);
    Surveyed {
        session: id.clone(),
        project,
        mentions,
        cwd,
        survey,
    }
}

/// `transcript show`: resolve `input` against the transcripts directory,
/// read the transcript through its provenance check, and survey it.
///
/// # Errors
///
/// [`CatalogError::TranscriptNotFound`] when there is no transcripts
/// directory or no such transcript, [`CatalogError::TranscriptRefused`],
/// [`CatalogError::Read`], or a store error.
pub fn show<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    input: &SessionId,
) -> Result<Surveyed, CatalogError<E>> {
    let id = catalog::resolve_transcript(ctx, provider, input)?;
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
    let config = ctx
        .store
        .read()
        .tags()
        .config()
        .map_err(CatalogError::Core)?
        .for_provider(provider);
    Ok(survey(ctx, &id, &bytes, &config))
}
