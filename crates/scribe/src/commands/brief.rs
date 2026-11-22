//! `scribe brief`: the session-start page (`scribe_core::brief`). The
//! memory block (today the memory page, as `memory show --page` prints it),
//! then the last sessions, this project's first, then every open claim,
//! grouped by tag with the id that settles it.
//!
//! Model-free and read-only: it reads the memory items, the session records
//! and `proposals.json`, takes no lock, writes nothing and is not audited
//! (STD-01 R31). The `SessionStart` hook injects the same text into an
//! interactive session's context through [`brief`].

use crate::app::Context;
use crate::error::CliError;
use crate::output::{BriefPayload, Output, Record};
use clap::Args;
use scribe_core::brief::{self, Brief, DEFAULT_SESSIONS, MAX_SESSIONS};
use scribe_core::{Project, SessionFilter, SessionId};

pub(super) const EXAMPLES: &str = "\
Examples:
  scribe brief
  scribe brief --sessions 10 --project scribe
  scribe brief --json | jq '.claims[].claims[].id'";

/// Arguments to `brief`.
#[derive(Debug, Args)]
pub(crate) struct BriefArgs {
    /// How many of the last sessions to show, 0 to 20
    #[arg(long, value_name = "N", default_value_t = DEFAULT_SESSIONS, value_parser = sessions_parser)]
    sessions: usize,

    /// List this project's sessions first, and show its memory page [default: the current directory's project]
    #[arg(long, value_name = "PROJECT")]
    project: Option<Project>,
}

/// `0` to [`MAX_SESSIONS`]; anything else is a usage error (exit 2).
fn sessions_parser(text: &str) -> Result<usize, String> {
    text.parse::<usize>()
        .ok()
        .filter(|n| *n <= MAX_SESSIONS)
        .ok_or_else(|| format!("expected a whole number from 0 to {MAX_SESSIONS}"))
}

/// Run `brief`.
pub(super) fn run(args: &BriefArgs, ctx: &Context) -> Result<Output, CliError> {
    let project = args.project.clone().or_else(|| {
        std::env::current_dir()
            .ok()
            .map(|cwd| super::hook::project::of(&cwd))
    });
    let memory = super::memory::page(ctx, None, project.as_ref())?.map(|p| p.text);
    let brief = brief(ctx, memory, project.as_ref(), None, args.sessions)?;
    Ok(Output::Show {
        record: Record::Brief(Box::new(BriefPayload::from(&brief))),
    })
}

/// The brief for `project` under the `memory` block, leaving out `exclude`
/// (the session starting). Read-only: no lock, no write.
pub(in crate::commands) fn brief(
    ctx: &Context,
    memory: Option<String>,
    project: Option<&Project>,
    exclude: Option<&SessionId>,
    sessions: usize,
) -> Result<Brief, CliError> {
    let listed = ctx.store().list_sessions(&SessionFilter::default())?;
    let proposals = ctx.store().proposals()?;
    let mut brief = brief::build(
        &listed.listing.items,
        &proposals,
        project,
        exclude,
        sessions,
    );
    brief.memory = memory;
    Ok(brief)
}
