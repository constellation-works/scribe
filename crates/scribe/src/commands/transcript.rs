//! `scribe transcript list` and `transcript show`: model-free readings of
//! Claude Code transcripts that have no record and no switch
//! (docs/design/scribe/2_design.md §5.8).
//!
//! Both read transcripts and the store and write nothing: no lock, no audit
//! line, and they work on a read-only root. `list` is how Daniel picks a
//! past session to catalog; `show` is the preview.

use super::hook::payload::{self, Transcript};
use super::hook::project;
use super::{DateArgs, ListNotice, day_parser};
use crate::app::Context;
use crate::error::CliError;
use crate::output::{Output, Record, Records, TranscriptPayload, WarningPayload};
use clap::{Args, Subcommand};
use scribe_core::{Config, DateRange, Project, SessionFilter, SessionId, transcript};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use time::Date;

/// The default `--min-turns`: long sessions only.
const DEFAULT_MIN_TURNS: u32 = 20;

pub(super) const EXAMPLES: &str = "\
Lists Claude Code transcripts that have no session record and no switch,
newest first, and writes nothing. --min-turns defaults to 20. project is
the configured [projects.<slug>] path with the most mentions, else the
cwd's project. show prints one transcript with Daniel's first five turns.

Examples:
  scribe transcript list --since 2026-10-01 --project scribe
  scribe transcript list --since 2026-10-01 --until 2026-10-31 --min-turns 40
  scribe transcript show <id>
  scribe transcript list --since 2026-10-01 --json | jq '.transcripts[].session'";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe transcript list --since 2026-10-01
  scribe transcript list --since 2026-10-01 --project orbit --project scribe
  scribe transcript list --since 2026-10-01 --min-turns 40 --limit 10
  scribe transcript show <id>";

const SHOW_EXAMPLES: &str = "\
Examples:
  scribe transcript show <id>
  scribe transcript show <id> --json | jq '.daniel_first_turns'";

/// Verbs on the `transcript` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum TranscriptCommand {
    /// List uncataloged transcripts, newest first, writing nothing
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one transcript and Daniel's first five turns, writing nothing
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
}

/// Arguments to `transcript list`.
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Only transcripts whose span, from the first message to the last, overlaps this day, YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = day_parser)]
    pub(crate) since: Date,

    /// Only transcripts on or before this day, YYYY-MM-DD [default: no upper bound]
    #[arg(long, value_name = "DATE", value_parser = day_parser)]
    pub(crate) until: Option<Date>,

    /// Only transcripts attributed to this project; repeat to accept any of them [default: any]
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: Vec<Project>,

    /// Only transcripts with at least this many of Daniel's turns
    #[arg(long, value_name = "N", default_value_t = DEFAULT_MIN_TURNS)]
    pub(crate) min_turns: u32,

    /// Show at most this many transcripts, 1 or more; --json reports the total and whether the list was cut [default: all]
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<std::num::NonZeroUsize>,
}

/// Arguments to `transcript show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The transcript's id, or a unique prefix of it (at least four characters)
    #[arg(value_name = "ID")]
    pub(crate) id: SessionId,
}

impl TranscriptCommand {
    /// `list` and `show` read only, so they are not audited (STD-01 R31).
    pub(super) fn audit(&self) -> Option<crate::audit_middleware::AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
        }
    }
}

/// Run one `transcript` verb.
pub(super) fn run(command: TranscriptCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        TranscriptCommand::List(args) => list(args, ctx),
        TranscriptCommand::Show(args) => show(&args, ctx),
    }
}

/// The transcript `input` names, resolved against filenames under the
/// transcripts directory. Reads only.
pub(in crate::commands) fn resolve_transcript(
    ctx: &Context,
    input: &SessionId,
) -> Result<SessionId, CliError> {
    let Some(dir) = ctx.transcripts().map(Path::to_path_buf) else {
        return Err(CliError::TranscriptNotFound {
            id: input.clone(),
            dir: PathBuf::from("(no transcripts directory)"),
        });
    };
    let files = transcript_files(&dir)?;
    let ids: Vec<String> = files.into_iter().map(|(id, _)| id.to_string()).collect();
    match scribe_core::resolve_prefix(input.as_str(), &ids) {
        Ok(full) => full
            .parse::<SessionId>()
            .map_err(|e| CliError::Usage(e.to_string())),
        Err(err) if err.code() == "id_ambiguous" => Err(err.into()),
        Err(_) => Err(CliError::TranscriptNotFound {
            id: input.clone(),
            dir,
        }),
    }
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let dates = dates(args.since, args.until)?;
    let mut criteria = Vec::new();
    criteria.extend(
        DateArgs {
            since: Some(args.since),
            until: args.until,
        }
        .criteria(),
    );
    for project in &args.project {
        criteria.push(format!("--project {project}"));
    }
    criteria.push(format!("--min-turns {}", args.min_turns));
    let Some(dir) = ctx.transcripts() else {
        return Ok(Output::List {
            notices: vec![String::from(
                "no transcripts directory: set HOME or CLAUDE_CONFIG_DIR",
            )],
            records: Records::Transcripts(Vec::new()),
            total: 0,
            warnings: Vec::new(),
        });
    };
    let config = ctx.store().config()?;
    let cataloged = cataloged(ctx)?;
    let mut warnings = Vec::new();
    let mut rows = Vec::new();
    for (id, path) in transcript_files(dir)? {
        if cataloged.contains(id.as_str()) {
            continue;
        }
        if payload::check_transcript(&path, &id, Some(dir)).is_err() {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                warnings.push(WarningPayload {
                    path: path.display().to_string(),
                    code: "io_error",
                    error: err.to_string(),
                });
                continue;
            }
        };
        let row = payload_from(&id, &bytes, &config, false);
        if row.daniel_turns < args.min_turns {
            continue;
        }
        if !args.project.is_empty() && !args.project.iter().any(|p| p.as_str() == row.project) {
            continue;
        }
        let (Some(started), Some(ended)) = (row.started, row.ended) else {
            continue;
        };
        if !dates.overlaps(started, ended) {
            continue;
        }
        rows.push(row);
    }
    rows.sort_by(|a, b| {
        b.started
            .cmp(&a.started)
            .then_with(|| a.session.cmp(&b.session))
    });
    let total = rows.len();
    if let Some(limit) = args.limit {
        rows.truncate(limit.get());
    }
    let notice = ListNotice {
        noun: "transcripts",
        criteria,
        empty: "no uncataloged transcripts",
    };
    Ok(Output::List {
        notices: notice.line(rows.len(), total).into_iter().collect(),
        records: Records::Transcripts(rows),
        total,
        warnings,
    })
}

fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let id = resolve_transcript(ctx, &args.id)?;
    let dir =
        ctx.transcripts()
            .map(Path::to_path_buf)
            .ok_or_else(|| CliError::TranscriptNotFound {
                id: id.clone(),
                dir: PathBuf::from("(no transcripts directory)"),
            })?;
    let path = find_transcript(&dir, &id).ok_or_else(|| CliError::TranscriptNotFound {
        id: id.clone(),
        dir: dir.clone(),
    })?;
    match payload::check_transcript(&path, &id, Some(&dir)) {
        Ok(Transcript::Present) => {}
        Ok(Transcript::Missing) => {
            return Err(CliError::TranscriptNotFound { id, dir });
        }
        Err(reason) => {
            return Err(CliError::TranscriptRefused { id, reason });
        }
    }
    let bytes = std::fs::read(&path).map_err(|source| CliError::Read {
        what: path.display().to_string(),
        source,
    })?;
    let config = ctx.store().config()?;
    Ok(Output::Show {
        record: Record::Transcript(Box::new(payload_from(&id, &bytes, &config, true))),
    })
}

fn payload_from(
    id: &SessionId,
    bytes: &[u8],
    config: &Config,
    with_turns: bool,
) -> TranscriptPayload {
    let survey = transcript::survey(bytes);
    let cwd = transcript::cwd(bytes).unwrap_or_default();
    let cwd_project = if cwd.is_empty() {
        Project::sanitized("")
    } else {
        project::of(Path::new(&cwd))
    };
    let counts = transcript::mention_counts(bytes, &config.projects);
    let project = transcript::attribute(&counts, &cwd_project);
    let mut mentions = BTreeMap::new();
    for (project, count) in &counts {
        mentions.insert(project.to_string(), *count);
    }
    TranscriptPayload {
        session: id.to_string(),
        project: project.to_string(),
        project_mentions: mentions,
        cwd,
        started: survey.started,
        ended: survey.ended,
        bytes: survey.bytes,
        daniel_turns: survey.daniel_turns,
        first_prompt: survey.first_prompt,
        daniel_first_turns: with_turns.then_some(survey.first_turns),
    }
}

/// The project `session catalog` records when `--project` is absent.
pub(in crate::commands) fn attributed_project(bytes: &[u8], config: &Config) -> Project {
    let cwd = transcript::cwd(bytes).unwrap_or_default();
    let cwd_project = if cwd.is_empty() {
        Project::sanitized("")
    } else {
        project::of(Path::new(&cwd))
    };
    transcript::attribute(
        &transcript::mention_counts(bytes, &config.projects),
        &cwd_project,
    )
}

fn dates(since: Date, until: Option<Date>) -> Result<DateRange, CliError> {
    DateArgs {
        since: Some(since),
        until,
    }
    .range()
}

/// Session ids that already have a record (including a file that did not
/// parse) or a switch. Those transcripts are not candidates.
fn cataloged(ctx: &Context) -> Result<BTreeSet<String>, CliError> {
    let found = ctx.store().list_sessions(&SessionFilter {
        include_inactive: true,
        ..Default::default()
    })?;
    let mut ids = BTreeSet::new();
    for record in &found.listing.items {
        ids.insert(record.id.to_string());
    }
    for skipped in &found.skipped {
        if let Some(stem) = skipped.path.file_stem().and_then(|stem| stem.to_str()) {
            ids.insert(stem.to_owned());
        }
    }
    for id in ctx.store().control()?.sessions.keys() {
        ids.insert(id.to_string());
    }
    Ok(ids)
}

/// `<projects-dir>/<project-dir>/<id>.jsonl`, the first project directory
/// (by name) that has one. A stem that is not a session id is skipped.
fn transcript_files(dir: &Path) -> Result<Vec<(SessionId, PathBuf)>, CliError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(CliError::Read {
                what: dir.display().to_string(),
                source,
            });
        }
    };
    let mut projects: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|item| item.path())
        .collect();
    projects.sort();
    let mut by_id: BTreeMap<SessionId, PathBuf> = BTreeMap::new();
    for project in projects {
        let Ok(files) = std::fs::read_dir(&project) else {
            continue;
        };
        for file in files.filter_map(Result::ok) {
            let path = file.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(stem) = name.strip_suffix(".jsonl") else {
                continue;
            };
            let Ok(id) = stem.parse::<SessionId>() else {
                continue;
            };
            by_id.entry(id).or_insert(path);
        }
    }
    Ok(by_id.into_iter().collect())
}

fn find_transcript(dir: &Path, id: &SessionId) -> Option<PathBuf> {
    transcript_files(dir).ok().and_then(|files| {
        files
            .into_iter()
            .find(|(found, _)| found == id)
            .map(|(_, path)| path)
    })
}
