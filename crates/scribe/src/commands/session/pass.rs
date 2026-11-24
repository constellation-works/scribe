//! `scribe session pass`: the arguments, then [`scribe_core::cases::pass::run`].

use crate::app::Context;
use crate::commands::RunLimits;
use crate::error::CliError;
use crate::output::Output;
use clap::Args;
use scribe_core::cases::pass::{self as case, PassInput};
use scribe_core::{Config, Models, SessionId};

/// Redact, flatten and bound a failure reason, as a pass report has it.
#[cfg(test)]
pub(super) use scribe_core::cases::pass::short_reason;

pub(in crate::commands) const COMMAND: &str = case::PASS_COMMAND;

pub(super) const PASS_EXAMPLES: &str = "\
The plugin routine ships disabled; once enabled on a host it runs every
5 minutes. Also run this command by hand where each configured provider CLI
is on PATH. Each session uses its stored provider (Codex defaults to Luna). Takes
at most five due sessions per run, processing every new-message window
(default 40 messages / 15000 characters; [catalog] window_messages and
window_chars). Each window rewrites the header and open segment, appends
new segments and checkpoints the record. Sealed segments stay unchanged;
[catalog] segment_windows (default 3) bounds how long a segment stays open.
--json reports windows, segments, seals and how many due sessions remain.
Also proposes up to 3 claims per session per run, only from the user's turns
that fire a trigger of a tag's capture rules (see scribe tag show <tag>),
until 20 candidate proposals are open. Review with scribe proposal list;
only a typed #scribe approve <id> writes an entry; #scribe reject <id> drops it.
Under a tag whose route is write (idea ships so), the pass writes up to 3
ideas per session per run itself, with their outcome (open, or parked in
the user's words); an idea that became a task in the session is not written.
List them with scribe entry list --tag idea --author-kind scribe; cancel one
with #scribe retire <id> or #scribe forget <id>.
A failed pass stays due with automatic retry delays of 5, 10, 20, 40,
then at most 60 minutes. --session retries immediately. Exits 1 when any
pass failed, naming each failed session, its error code and short reason.

Examples:
  scribe session pass
  scribe session pass --session <id> --session <id>
  scribe session pass --json | jq '.passes[] | select(.result == \"failed\")'";

/// Arguments to `session pass`.
#[derive(Debug, Args)]
pub(crate) struct PassArgs {
    /// Pass this session (its id or a unique prefix of at least four characters) now, ignoring retry backoff and whether a pass is due (it must be switched on); repeat for several [default: every due session, at most five]
    #[arg(long = "session", value_name = "ID")]
    pub(crate) sessions: Vec<SessionId>,
}

/// Pass the due sessions, or the sessions `--session` names (each a prefix
/// of its id).
pub(super) fn run(args: &PassArgs, ctx: &Context) -> Result<Output, CliError> {
    let max = if args.sessions.is_empty() {
        scribe_core::pass::MAX_PER_RUN
    } else {
        args.sessions.len()
    };
    let report = case::run(
        &ctx.core(),
        PassInput {
            sessions: &args.sessions,
            resolve: true,
            models: |config: &Config| ctx.models(config),
            limits: RunLimits::new(max),
            host: ctx.host(),
        },
    )?;
    Ok(Output::Pass(report))
}

/// [`run`] with the `Models` adapter `models` builds for the config, so a
/// test can answer with a fake, and within `limits`: the plugin's tool
/// starts no session after its start-by time, so a call ends inside
/// Orbit's backend timeout. `only` names the sessions to pass whether or
/// not they are due; empty takes the due ones.
pub(in crate::commands) fn run_with<M: Models>(
    ctx: &Context,
    models: impl FnMut(&Config) -> M,
    limits: RunLimits,
    only: &[SessionId],
) -> Result<Output, CliError> {
    let report = case::run(
        &ctx.core(),
        PassInput {
            sessions: only,
            resolve: false,
            models,
            limits,
            host: ctx.host(),
        },
    )?;
    Ok(Output::Pass(report))
}
