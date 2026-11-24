//! `scribe brief`: the session-start page (`scribe_core::brief`). The
//! memory block (Profile, Now and Milestones, as `memory show` prints it),
//! then the last sessions, this project's first, then every open claim,
//! grouped by tag with the id that settles it.
//!
//! Model-free and read-only: it reads the memory files, the session records
//! and `proposals.json`, takes no lock, writes nothing and is not audited
//! (STD-01 R31). The reads are the brief app's, which the
//! `SessionStart` hook also uses for an interactive session's context.

use crate::app::Context;
use crate::error::CliError;
use crate::output::{BriefPayload, MemoryBlockPayload, Output, Record};
use clap::Args;
use scribe_core::Project;
use scribe_core::apps::brief::{BriefInput, Command, Outcome, exec};
use scribe_core::brief::{DEFAULT_SESSIONS, MAX_SESSIONS};

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

    /// List this project's sessions first [default: the current directory's project]
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
    let input = BriefInput {
        project: project.as_ref(),
        sessions: args.sessions,
        provider: ctx.provider(),
    };
    let Outcome::Page(page) = exec(&ctx.runtime(), Command::Brief(input))?;
    let block = MemoryBlockPayload::from_view(&page.view, ctx.store().root());
    let mut payload = BriefPayload::new(&page.brief, block);
    payload.text = page.brief.text_with_ids(&page.ids);
    Ok(Output::Show {
        record: Record::Brief(Box::new(payload)),
    })
}
