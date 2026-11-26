//! `scribe transcript list|show` (ORB-27707 step 3d): the transcripts on
//! this host with no session record, surveyed without cataloging them.
//! Both read transcripts and the store and write nothing. Discovery and
//! parsing are the tools' (`scribe-tools`); the cataloged-session and
//! transcript-id lookups are the `session` app's public entry points.

use crate::apps::session::{CatalogError, list_sessions, resolve_transcript};
use crate::apps::tag::ConfigReads;
use crate::runtime::{Ctx, PortError, Unreadable};
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
    let id = resolve_transcript(ctx, provider, input)?;
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
    let config = ctx
        .store
        .read()
        .tags()
        .config()
        .map_err(CatalogError::Core)?
        .for_provider(provider);
    Ok(survey(ctx, &id, &bytes, &config))
}

/// What `transcript list` reads.
pub struct ListInput<'i> {
    /// `--since`: the first day a transcript's span may overlap.
    pub since: time::Date,
    /// The checked `--since`/`--until` range.
    pub dates: crate::DateRange,
    /// `--project`, any of them; empty takes any.
    pub projects: &'i [Project],
    /// `--min-turns`.
    pub min_turns: u32,
    /// `--limit`.
    pub limit: Option<std::num::NonZeroUsize>,
}

/// What `transcript list` found.
pub struct Listed {
    /// The transcripts kept, newest first, cut to the limit.
    pub rows: Vec<Surveyed>,
    /// How many were kept before the cut.
    pub total: usize,
    /// The transcripts that could not be read: the path and the error.
    pub warnings: Vec<(String, String)>,
}

/// `transcript list`: the `provider` transcripts with no session record and
/// no switch, surveyed and filtered. `None` when there is no transcripts
/// directory.
///
/// # Errors
///
/// When the transcripts tree cannot be listed, or a store error.
pub fn list<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    input: &ListInput<'_>,
) -> Result<Option<Listed>, CatalogError<E>> {
    if ctx.transcripts.dir(provider).is_none() {
        return Ok(None);
    }
    let config = ctx
        .store
        .read()
        .tags()
        .config()
        .map_err(CatalogError::Core)?
        .for_provider(provider);
    let cataloged = cataloged(ctx).map_err(CatalogError::Core)?;
    let since_start = since_start(input.since);
    let mut warnings = Vec::new();
    let mut rows = Vec::new();
    for (id, path) in ctx
        .transcripts
        .files(provider)
        .map_err(CatalogError::Port)?
    {
        if cataloged.contains(id.as_str()) {
            continue;
        }
        // Skip without opening: a file last written before `--since`
        // started cannot be read cheaply, and the acceptance rule is to
        // skip it even if its message timestamps fall in range.
        if let Some(modified) = ctx.transcripts.modified(&path)
            && modified < since_start
        {
            continue;
        }
        let bytes = match ctx.transcripts.read_checked(provider, &path, &id) {
            Ok(Some(bytes)) => bytes,
            Ok(None) | Err(Unreadable::Refused(_)) => continue,
            Err(Unreadable::Io(err)) => {
                warnings.push((path.display().to_string(), err.to_string()));
                continue;
            }
        };
        let row = survey(ctx, &id, &bytes, &config);
        if row.survey.user_turns < input.min_turns {
            continue;
        }
        if !input.projects.is_empty()
            && !input
                .projects
                .iter()
                .any(|p| p.as_str() == row.project.as_str())
        {
            continue;
        }
        let (Some(started), Some(ended)) = (row.survey.started, row.survey.ended) else {
            continue;
        };
        if !input.dates.overlaps(started, ended) {
            continue;
        }
        rows.push(row);
    }
    rows.sort_by(|a, b| {
        b.survey
            .started
            .cmp(&a.survey.started)
            .then_with(|| a.session.as_str().cmp(b.session.as_str()))
    });
    let total = rows.len();
    if let Some(limit) = input.limit {
        rows.truncate(limit.get());
    }
    Ok(Some(Listed {
        rows,
        total,
        warnings,
    }))
}

/// The start of the `--since` day as a system time (midnight UTC).
fn since_start(since: time::Date) -> std::time::SystemTime {
    let start = since.midnight().assume_utc().unix_timestamp();
    if start >= 0 {
        std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(u64::try_from(start).unwrap_or(0))
    } else {
        std::time::SystemTime::UNIX_EPOCH
            - std::time::Duration::from_secs(u64::try_from(-start).unwrap_or(0))
    }
}

/// Session ids that already have a record (including a file that did not
/// parse) or a switch. Those transcripts are not candidates.
fn cataloged<E>(ctx: &Ctx<'_, E>) -> crate::Result<std::collections::BTreeSet<String>> {
    let found = list_sessions(
        &ctx.store.read(),
        &crate::SessionFilter {
            include_inactive: true,
            ..Default::default()
        },
        None,
    )?;
    let mut ids = std::collections::BTreeSet::new();
    for record in &found.listing.items {
        ids.insert(record.id.to_string());
    }
    for id in &found.skipped_sessions {
        ids.insert(id.to_string());
    }
    for id in ctx.store.read().sessions().control()?.sessions.keys() {
        ids.insert(id.to_string());
    }
    Ok(ids)
}
