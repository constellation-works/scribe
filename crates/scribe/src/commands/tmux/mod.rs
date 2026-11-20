//! `scribe tmux`: the tmux popup, a read-only fzf list of sessions,
//! entries, proposals and backfill candidates (docs/design/scribe/2_design.md
//! §1.2).
//!
//! The popup is a POSIX shell script embedded in the binary and run by
//! [`crate::popup`]; its child commands run this same executable, so no
//! separate file is installed and no other `scribe` on `PATH` is consulted. The command
//! never opens the store: it writes nothing, takes no lock and is not audited
//! (STD-01 R31). The popup's children own the terminal.

use crate::app::Context;
use crate::error::CliError;
use crate::output::{FORMAT_ENV, Output};
use crate::popup;

mod internal;
use clap::Args;

pub(super) const EXAMPLES: &str = "\
Browse scribe in an fzf list: sessions, entries, proposals and backfill
candidates, each record previewed in full. It only reads and needs fzf on
PATH; run it from a terminal, usually in a tmux popup. Add this to
~/.tmux.conf, reload it (tmux source-file ~/.tmux.conf), and press prefix
then S:

  bind-key S display-popup -E -w 90% -h 85% scribe tmux

Rows read as a start date (MM-DD, or YYYY-MM-DD for another year), then:
  sessions     status (open or final), title, project; newest started first
  entries      tag, the first line of the text
  proposals    state, what is proposed
  backfill     project, the first prompt
The id is not shown; it stays attached to the row for the preview, enter and
ctrl-y, and topics stay searchable. The preview leads with the title and
summary for a session, followed by its goals, next steps, metadata and log.
Entries show their text and metadata. Enter opens the record in the pager.
Color follows NO_COLOR and TERM, and inactive sessions are dimmed.

Keys:
  enter       open the record in $PAGER (else less): a session with its log,
              an entry in full, a proposal as its fields. On a backfill
              candidate, put '/scribe catalog <id>' in the tmux paste buffer
              and say so; typed into a Claude Code prompt, it catalogs the
              session as Daniel's (the CLI refuses --author daniel)
  ctrl-y      copy the record's id (tmux buffer, and pbcopy when present)
  ctrl-s      sessions            ctrl-e  active entries
  ctrl-p      open proposals      ctrl-b  backfill candidates (uncataloged
                                          transcripts of the last 30 days)
  alt-a       toggle hidden records: inactive sessions, and non-active entries

A list that is empty or fails shows one notice row instead of failing the
popup. The popup's commands use the same data directory (--root or
SCRIBE_ROOT) and the executable you started.

Examples:
  scribe tmux
  scribe tmux --root ~/.scribe";

/// Arguments to `tmux`: none beyond the global flags. The hidden
/// subcommands are the popup's own callbacks, not part of the documented
/// surface: they read the same records the list and show commands do and
/// print rows or a preview for the popup's fzf, and they write nothing.
#[derive(Debug, Args)]
pub(crate) struct TmuxArgs {
    #[command(subcommand)]
    internal: Option<internal::Internal>,
}

/// Run the popup and wait for it. Read-only: nothing here touches the store.
pub(super) fn run(args: TmuxArgs, ctx: &Context) -> Result<Output, CliError> {
    if let Some(internal) = args.internal {
        return internal::run(internal, ctx);
    }
    if !popup::on_path("fzf") {
        return Err(CliError::FzfMissing);
    }
    let exe = std::env::current_exe().map_err(CliError::TmuxSpawn)?;
    // A format set for the whole session must not reshape the lists the
    // popup parses; its detail views ask for theirs explicitly.
    let status =
        popup::run(&exe, ctx.store().root(), &[FORMAT_ENV]).map_err(CliError::TmuxSpawn)?;
    if status.success() {
        Ok(Output::Popup)
    } else {
        Err(CliError::TmuxFailed(status.code()))
    }
}
