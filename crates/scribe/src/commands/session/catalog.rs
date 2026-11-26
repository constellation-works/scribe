//! `scribe session catalog`: switch a never-switched session on by name;
//! its windows run as the `scribe-session-catalog` Orbit job
//! (docs/design/scribe/2_design.md §5.8, §19.3.3).
//!
//! A session that already has a record or a `/scribe on` switch is refused
//! `session_cataloged`, unless its catalog switch is still on and due; that
//! catalog resumes even when a partial record exists. One `#scribe forget`
//! erased is refused `session_forgotten` first. A catalog that failed stays
//! due, and running the job again resumes it without resetting `since` or
//! `cataloged_by`.
//! The transcript's age is its last message, not the file's mtime: a day
//! old, the last window marks the record final and the switch off; newer,
//! the switch stays on and due. No entries are written.
//!
//! Who cataloged it is a channel, like every `author`. The CLI records
//! `kind: agent` under the caller's `--author` and refuses the reserved
//! names (`daniel`, `scribe`, `consistency`) `reserved_author`, with or
//! without a terminal: the user catalogs by typing `/scribe catalog <id>` in
//! their prompt, which the hook turns into a catalog switch with
//! `cataloged_by: {kind: daniel}` ([`begin`]); the job, or the next
//! `session pass`, then runs its windows and keeps that attribution
//! (4_decisions.md "The user writes only through their typed prompt").

use super::passed;
use crate::error::CliError;
use crate::output::Output;
use clap::Args;
#[cfg(test)]
use scribe_core::Author;
#[cfg(test)]
use scribe_core::apps::session as app;
use scribe_core::apps::session::{CatalogInput, Command, exec};
use scribe_core::composition::Context;
use scribe_core::{AuthorName, Project, SessionId};

/// A catalog begun: its switch, whether this run wrote it, and the bytes.
#[cfg(test)]
pub(in crate::commands) use scribe_core::apps::session::Begun;

pub(super) const EXAMPLES: &str = "\
Switches one past session on with cataloged_by and prints the Orbit job
command that runs every window, with no deadline. Refuses a session that
already has a record or a /scribe on switch, unless its catalog switch is
still on and due; that run resumes even with a partial record. A transcript
whose last message is at least a day old is marked final and switched off;
a newer one stays on and due. A failed catalog stays due; run the job again
to resume, or let the next session pass finish it. Writes no entries.

A new catalog is recorded as kind agent under --author; the reserved names
user, scribe and consistency are refused. The user catalogs as themselves by
typing /scribe catalog <id> in their prompt; the job run on that session
afterwards runs its windows and keeps cataloged_by user.

Examples:
  scribe session catalog <id> --author opus
  scribe session catalog <id> --author opus --project scribe
  orbit job run scribe-session-catalog --input '{\"session\":\"<id>\",\"author\":\"opus\"}'";

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

/// Write the catalog switch and name the job.
pub(super) fn run(args: &CatalogArgs, ctx: &Context) -> Result<Output, CliError> {
    run_audited(args, ctx, exec)
}

/// [`run`] unaudited, as the tests drive it.
#[cfg(test)]
pub(in crate::commands) fn run_unaudited(
    args: &CatalogArgs,
    ctx: &Context,
) -> Result<Output, CliError> {
    run_audited(args, ctx, app::exec_unaudited)
}

/// [`run`], run by `exec`: the session app's audited `exec`, or the
/// unaudited one the tests drive.
fn run_audited(args: &CatalogArgs, ctx: &Context, exec: super::Exec) -> Result<Output, CliError> {
    let input = CatalogInput {
        id: &args.id,
        author: &args.author,
        project: args.project.clone(),
        provider: ctx.provider(),
    };
    Ok(passed(exec(ctx, Command::Catalog(input))?, ctx))
}

/// Begin a catalog of `input` by `by` (the session app's `begin_catalog`).
#[cfg(test)]
pub(in crate::commands) fn begin(
    ctx: &Context,
    input: &SessionId,
    by: Author,
) -> Result<Begun, CliError> {
    Ok(app::begin_catalog(ctx, ctx.provider(), input, by)?)
}
