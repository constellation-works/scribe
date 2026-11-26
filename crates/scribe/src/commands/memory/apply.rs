use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::memory::{ApplyRefreshRequest, Command, exec};
use scribe_core::composition::Context;
use std::io::Read;

/// The most bytes `memory apply` reads from stdin.
const MAX_APPLY_INPUT: u64 = 1024 * 1024;

pub(super) const APPLY_EXAMPLES: &str = "\
Reads one JSON apply request on stdin ({basis, cursor_to, session_ids,
backlog, passed_stems, answer[, allow_retry]}), verifies the optimistic
basis, profile shape, Pinned byte identity, links, redaction and milestone
bounds, and commits atomically via the memory journal.

Examples:
  scribe memory apply --json < apply.json";

/// Arguments to `memory apply`: none (JSON request is read on stdin).
#[derive(Debug, Args)]
pub(crate) struct ApplyArgs {}

pub(super) fn run(ApplyArgs {}: ApplyArgs, ctx: &Context) -> Result<Output, CliError> {
    let mut raw = Vec::new();
    std::io::stdin()
        .take(MAX_APPLY_INPUT)
        .read_to_end(&mut raw)
        .map_err(|source| CliError::Read {
            what: String::from("stdin"),
            source,
        })?;
    let request: ApplyRefreshRequest = serde_json::from_slice(&raw).map_err(|err| {
        CliError::Usage(format!(
            "memory apply expects JSON on stdin ({{basis, cursor_to, session_ids, backlog, passed_stems, answer}}): {err}"
        ))
    })?;
    let command = Command::Apply {
        provider: ctx.provider(),
        request: Box::new(request),
        routine: false,
    };
    Ok(output(exec(ctx, command)?, ctx))
}
