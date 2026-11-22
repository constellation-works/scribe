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
//! Who cataloged it is a channel, like every `author`. The CLI records
//! `kind: agent` under the caller's `--author` and refuses the reserved
//! names (`daniel`, `scribe`, `consistency`) `reserved_author`, with or
//! without a terminal: the user catalogs by typing `/scribe catalog <id>` in
//! their prompt, which the hook turns into a catalog switch with
//! `cataloged_by: {kind: daniel}` ([`begin`]); this command, or the next
//! `session pass`, then runs its windows and keeps that attribution
//! (4_decisions.md "The user writes only through their typed prompt").

use super::pass;
use crate::app::Context;
use crate::commands::hook::payload;
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

A new catalog is recorded as kind agent under --author; the reserved names
user, scribe and consistency are refused. The user catalogs as themselves by
typing /scribe catalog <id> in their prompt; running this command on that
session afterwards runs its windows and keeps cataloged_by user.

Examples:
  scribe session catalog <id> --author opus
  scribe session catalog <id> --author opus --project scribe
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
    run_with(args, ctx, |config| ctx.models(config))
}

/// [`run`] with the `Models` adapter `models` builds, so a test can answer
/// with a fake.
pub(in crate::commands) fn run_with<M: Models>(
    args: &CatalogArgs,
    ctx: &Context,
    models: impl FnOnce(&scribe_core::Config) -> M,
) -> Result<Output, CliError> {
    scribe_core::check_agent_author(&args.author)?;
    let begun = begin(ctx, &args.id, Author::agent(args.author.clone()))?;
    let config = ctx.config()?;
    let project = args
        .project
        .clone()
        .unwrap_or_else(|| super::super::transcript::attributed_project(&begun.bytes, &config));
    let ended = transcript::survey(&begun.bytes)
        .ended
        .is_some_and(|last| transcript::is_day_old(last, ctx.now()));
    let mut report: PassReport = pass::run_catalog(
        ctx,
        models,
        &begun.id,
        &begun.switch,
        project,
        ended,
        args.author.as_str(),
    )?;
    report.switch_written = begun.wrote_switch;
    Ok(Output::Pass(report))
}

/// A catalog switch, started or resumed by [`begin`].
pub(in crate::commands) struct Begun {
    /// The resolved session id.
    pub(in crate::commands) id: SessionId,
    /// The switch as it now stands.
    pub(in crate::commands) switch: scribe_core::SessionControl,
    /// Whether this call wrote the switch (a start, not a resume).
    pub(in crate::commands) wrote_switch: bool,
    /// The transcript, read once for the caller's attribution and age.
    pub(in crate::commands) bytes: Vec<u8>,
}

/// Resolve `input` to a transcript, check it, refuse a missing redaction
/// value, and start a catalog switch recorded as cataloged by `by`, or
/// resume one a catalog already wrote (keeping its `since` and
/// `cataloged_by`). Shared by this command (`by` an agent) and the
/// `/scribe catalog <id>` prompt command (`by` the user). Runs no window.
///
/// # Errors
///
/// The transcript's refusal, `session_cataloged` for a session that already
/// has a record or a non-catalog switch, or a store or config error.
pub(in crate::commands) fn begin(
    ctx: &Context,
    input: &SessionId,
    by: Author,
) -> Result<Begun, CliError> {
    let id = super::super::transcript::resolve_transcript(ctx, input)?;
    let dir = ctx
        .transcripts()
        .map(PathBuf::from)
        .ok_or_else(|| CliError::TranscriptNotFound {
            id: id.clone(),
            dir: PathBuf::from("(no transcripts directory)"),
        })?;
    let path = payload::locate(ctx, &id)?.ok_or_else(|| CliError::TranscriptNotFound {
        id: id.clone(),
        dir: dir.clone(),
    })?;
    // Checked and read through one handle, opened without following links.
    let bytes = match payload::read_checked(ctx, &path, &id) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Err(CliError::TranscriptNotFound { id, dir }),
        Err(payload::Unreadable::Refused(reason)) => {
            return Err(CliError::TranscriptRefused { id, reason });
        }
        Err(payload::Unreadable::Io(source)) => {
            return Err(CliError::Read {
                what: path.display().to_string(),
                source,
            });
        }
    };
    let store = ctx.store();
    // Refuse a missing redaction value before the switch is written.
    let config = ctx.config()?;
    let _redactor = ctx.redactor(&config)?;
    // The gate, including the record lookup, runs inside the store lock, so
    // a `/scribe on` or a second catalog that lands first is seen and
    // refused, never overwritten.
    let (switch, wrote_switch) = store.update_control(|state| -> Result<_, CliError> {
        let has_record = recorded(ctx, &id)?;
        match control::catalog_gate(state, &id, has_record) {
            CatalogGate::Refused => Err(CliError::SessionCataloged(id.clone())),
            CatalogGate::Resume(switch) => {
                if switch.provider != ctx.provider() {
                    return Err(CliError::Usage(String::from(
                        "catalog switch belongs to another provider",
                    )));
                }
                Ok((switch, false))
            }
            CatalogGate::Start => {
                control::catalog_on(state, &id, by, ctx.now());
                let switch = state.sessions.get_mut(&id).ok_or_else(|| {
                    CliError::Core(scribe_core::Error::SessionNotFound {
                        id: id.clone(),
                        path: store.root().join("control.json"),
                    })
                })?;
                switch.provider = ctx.provider();
                Ok((switch.clone(), true))
            }
        }
    })??;
    Ok(Begun {
        id,
        switch,
        wrote_switch,
        bytes,
    })
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
