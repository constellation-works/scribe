//! `scribe session catalog`: switch a never-switched session on by name and
//! run every window (docs/design/scribe/2_design.md §5.8).
//!
//! A session that already has a record or a `/scribe on` switch is refused
//! `session_cataloged`. A catalog that failed stays due, and the next run
//! resumes it without resetting `since` or `cataloged_by`. The transcript's
//! age is its last message, not the file's mtime: a day old, the last window
//! marks the record final and the switch off; newer, the switch stays on
//! and due. No entries are written.
//!
//! Who cataloged it is a channel, like every `author`: `--author daniel`
//! records `kind: daniel` only when stdin is a terminal (Daniel typing or
//! pasting the popup's command in a shell) and is refused
//! `reserved_author` otherwise, so an agent driving the CLI through a pipe
//! cannot catalog as Daniel. Any other name records `kind: agent`, and the
//! reserved names (`scribe`, `consistency`) are refused the same way.

use super::pass::{self, find};
use crate::app::Context;
use crate::commands::hook::payload::{self, Transcript as Checked};
use crate::error::CliError;
use crate::output::{Output, PassReport};
use clap::Args;
use scribe_core::control::{self, CatalogGate};
use scribe_core::{Author, AuthorName, Models, Project, SessionFilter, SessionId, transcript};
use std::path::PathBuf;

/// The command path in the audit log.
pub(in crate::commands) const COMMAND: &str = "session catalog";

pub(super) const EXAMPLES: &str = "\
Switches one past session on with cataloged_by and runs every window, with
no deadline. Refuses a session that already has a record or a /scribe on
switch. A transcript whose last message is at least a day old is marked
final and switched off; a newer one stays on and due. A failed catalog
keeps its checkpoint; run the same command again to resume. Writes no entries.

--author daniel is Daniel's channel: it records kind daniel only from an
interactive terminal and is refused otherwise. An agent catalogs under its
own name, recorded as kind agent.

Examples:
  scribe session catalog <id> --author daniel
  scribe session catalog <id> --author daniel --project scribe
  scribe session catalog <id> --author opus --json | jq '.passes[0].result'";

/// Arguments to `session catalog`.
#[derive(Debug, Args)]
pub(crate) struct CatalogArgs {
    /// The transcript's id, or a unique prefix of it (at least four characters)
    #[arg(value_name = "ID")]
    pub(crate) id: SessionId,

    /// Who is asking: lowercase letters, digits, '.', '_' or '-', 1-40 characters
    #[arg(long, value_name = "NAME")]
    pub(crate) author: AuthorName,

    /// Record this project instead of the transcript's attribution [default: the attribution transcript list would use]
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: Option<Project>,
}

/// Catalog with the `claude` CLI as the session model.
pub(super) fn run(args: &CatalogArgs, ctx: &Context) -> Result<Output, CliError> {
    run_with(args, ctx, crate::output::stdin_interactive(), |config| {
        ctx.models(config)
    })
}

/// [`run`] with the `Models` adapter `models` builds, so a test can answer
/// with a fake. `interactive` is whether stdin is a terminal; it decides
/// the `--author daniel` channel (see the module comment).
pub(in crate::commands) fn run_with<M: Models>(
    args: &CatalogArgs,
    ctx: &Context,
    interactive: bool,
    models: impl FnOnce(&scribe_core::Config) -> M,
) -> Result<Output, CliError> {
    let by = catalog_author(&args.author, interactive)?;
    let id = super::super::transcript::resolve_transcript(ctx, &args.id)?;
    let dir = ctx
        .transcripts()
        .map(PathBuf::from)
        .ok_or_else(|| CliError::TranscriptNotFound {
            id: id.clone(),
            dir: PathBuf::from("(no transcripts directory)"),
        })?;
    let path = find(&dir, &id).ok_or_else(|| CliError::TranscriptNotFound {
        id: id.clone(),
        dir: dir.clone(),
    })?;
    match payload::check_transcript(&path, &id, Some(&dir)) {
        Ok(Checked::Present) => {}
        Ok(Checked::Missing) => return Err(CliError::TranscriptNotFound { id, dir }),
        Err(reason) => return Err(CliError::TranscriptRefused { id, reason }),
    }
    let bytes = std::fs::read(&path).map_err(|source| CliError::Read {
        what: path.display().to_string(),
        source,
    })?;
    let store = ctx.store();
    // Refuse a missing redaction value before the switch is written.
    let config = store.config()?;
    let _redactor = ctx.redactor(&config)?;
    let has_record = recorded(ctx, &id)?;
    let state = store.control()?;
    let (switch, wrote_switch) = match control::catalog_gate(&state, &id, has_record) {
        CatalogGate::Refused => return Err(CliError::SessionCataloged(id)),
        CatalogGate::Resume(switch) => (switch, false),
        CatalogGate::Start => {
            store.update_control(|state| control::catalog_on(state, &id, by, ctx.now()))?;
            let switch = store.control()?.get(&id).cloned().ok_or_else(|| {
                CliError::Core(scribe_core::Error::SessionNotFound {
                    id: id.clone(),
                    path: store.root().join("control.json"),
                })
            })?;
            (switch, true)
        }
    };
    let project = args
        .project
        .clone()
        .unwrap_or_else(|| super::super::transcript::attributed_project(&bytes, &config));
    let ended = transcript::survey(&bytes)
        .ended
        .is_some_and(|last| transcript::is_day_old(last, ctx.now()));
    let mut report: PassReport = pass::run_catalog(ctx, models, &id, &switch, project, ended)?;
    report.switch_written = wrote_switch;
    Ok(Output::Pass(report))
}

/// The `cataloged_by` author for `--author <name>`: Daniel's channel only
/// from an interactive terminal, an agent channel under an agent's own
/// name, and a reserved name refused (`reserved_author`) either way.
fn catalog_author(name: &AuthorName, interactive: bool) -> Result<Author, CliError> {
    if name.as_str() == "daniel" && interactive {
        return Ok(Author::daniel());
    }
    scribe_core::check_agent_author(name)?;
    Ok(Author::agent(name.clone()))
}

/// Whether a session file for `id` exists, including one that did not parse.
fn recorded(ctx: &Context, id: &SessionId) -> Result<bool, scribe_core::Error> {
    let found = ctx.store().list_sessions(&SessionFilter {
        include_inactive: true,
        ..Default::default()
    })?;
    let parsed = found.listing.items.iter().any(|record| &record.id == id);
    let skipped = found.skipped.iter().any(|skipped| {
        skipped.path.file_stem().and_then(|stem| stem.to_str()) == Some(id.as_str())
    });
    Ok(parsed || skipped)
}
