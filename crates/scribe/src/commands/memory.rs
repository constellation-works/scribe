//! `scribe memory`: what scribe remembers about the user — the profile,
//! `now` items and milestones (docs/design/scribe/2_design.md §15.4–§15.6,
//! §19.3.5).
//!
//! `show` and `rules` read only: no lock, no audit, no replay. `ack`,
//! `forget`, `restore`, `prepare`, `apply` and `refresh` are audited writes
//! and call no model. Memory is scribe-authored, so an agent acknowledges or
//! forgets an item without the user's instruction check, as it may an agent
//! note.

use crate::error::CliError;
use crate::output::{
    MemoryBlockPayload, MemoryPayload, MemoryRulesPayload, Output, Record, RefreshPayload,
    RefreshRetryPayload, RevisionPayload,
};
use clap::{Args, Subcommand};
use scribe_core::apps::memory::{
    ApplyRefreshOutcome, ApplyRefreshRequest, Command, Outcome, Shown, exec,
};
use scribe_core::composition::Context;
use scribe_core::{AuthorName, SessionId};
use std::io::Read;

/// The most bytes `memory apply` reads from stdin.
const MAX_APPLY_INPUT: u64 = 1024 * 1024;

pub(super) const MEMORY_EXAMPLES: &str = "\
Memory is one fixed shape: memory/profile.md, rebuilt weekly by the
scribe-memory-refresh Orbit job; memory/now/, dated things the user plans,
attends or must meet, written by the session pass of a session switched on
with /scribe on; and memory/milestones/. It describes, it never binds: rules
are their preferences entries.

Examples:
  scribe memory show
  scribe memory show profile
  scribe memory show 2026-12-01-hackathon-deadline
  scribe memory ack 2026-12-01-hackathon-deadline --session <id> --author opus
  scribe memory refresh
  scribe memory prepare --json";

const SHOW_EXAMPLES: &str = "\
With no id, prints the memory block the session-start brief embeds:
Profile, Now (every item, nearest date first) and Milestones (the last
five, newest first). With an id (profile, or a now or milestone file stem),
prints that file. --revision prints one stored profile revision.

Examples:
  scribe memory show
  scribe memory show --json | jq '.now[].id'
  scribe memory show profile
  scribe memory show 2026-12-01-hackathon-deadline --json
  scribe memory show --revision 3";

const RULES_EXAMPLES: &str = "\
Prints memory's fixed rules: how many items a pass writes, how many undated
candidates a session holds, how long a now item stays after its date, the
milestone lines shown, the profile's length and the phrases that make an
event a milestone. Memory has no switches.

Examples:
  scribe memory rules
  scribe memory rules --json | jq '.rules[] | {rule, value}'";

const ACK_EXAMPLES: &str = "\
Run it after raising a now item the user did not bring up themselves, so the
next session does not raise it again unprompted.

Examples:
  scribe memory ack 2026-12-01-hackathon-deadline --session <id> --author opus";

const FORGET_EXAMPLES: &str = "\
Removes a now item or milestone for good, with its history; it cannot be
undone, so it needs --confirm. The profile is corrected by editing
memory/profile.md: the next refresh takes the edit as the user's correction.

Examples:
  scribe memory forget 2026-12-01-hackathon-deadline --author opus --confirm";

const RESTORE_EXAMPLES: &str = "\
Writes a stored profile revision back as the profile, as a new revision.

Examples:
  scribe memory show --revision 3
  scribe memory restore 3";

const REFRESH_EXAMPLES: &str = "\
Runs the deterministic part of memory refresh: replays any open memory
journal, archives legacy format-24 items, and succeeds with nothing_new when
there are no new sessions, profile edits or passed now items. When a model
rebuild is needed, points you to 'orbit job run scribe-memory-refresh' (or
'scribe memory prepare --json' and 'scribe memory apply' for an interactive
agent).

Examples:
  scribe memory refresh
  scribe memory refresh --json";

const PREPARE_EXAMPLES: &str = "\
Replays any open memory journal and prints the bounded inputs, optimistic
basis, cursor and passed now stems needed to rebuild memory/profile.md.
Returns nothing_new: true when there is nothing to refresh.

Examples:
  scribe memory prepare --json";

const APPLY_EXAMPLES: &str = "\
Reads one JSON apply request on stdin ({basis, cursor_to, session_ids,
backlog, passed_stems, answer[, allow_retry]}), verifies the optimistic
basis, profile shape, Pinned byte identity, links, redaction and milestone
bounds, and commits atomically via the memory journal.

Examples:
  scribe memory apply --json < apply.json";

/// Verbs on the `memory` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum MemoryCommand {
    /// Show the memory block, one memory file, or a profile revision
    ///
    /// Shows the memory block (Profile, Now, Milestones), one file by id, or one stored profile revision.
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
    /// Show memory's fixed rules
    ///
    /// Shows memory's fixed rules and values; memory has no switches.
    #[command(after_help = RULES_EXAMPLES)]
    Rules(RulesArgs),
    /// Mark a now item as raised in this session
    ///
    /// Marks a now item as raised in this session, so it is not raised again unprompted.
    #[command(after_help = ACK_EXAMPLES)]
    Ack(AckArgs),
    /// Forget a now item or milestone for good
    ///
    /// Removes a now item or milestone file for good, with its history.
    #[command(after_help = FORGET_EXAMPLES)]
    Forget(ForgetArgs),
    /// Write a stored profile revision back as the profile
    #[command(after_help = RESTORE_EXAMPLES)]
    Restore(RestoreArgs),
    /// Run deterministic refresh checks or point to the Orbit refresh job
    ///
    /// Replays any open memory journal, succeeds when nothing is new, and otherwise directs you to the scribe-memory-refresh Orbit job.
    #[command(after_help = REFRESH_EXAMPLES)]
    Refresh(RefreshArgs),
    /// Prepare bounded profile refresh inputs and optimistic basis
    ///
    /// Replays any open memory journal and emits the bounded inputs and basis for scribe_profile_refresh or an interactive agent.
    #[command(after_help = PREPARE_EXAMPLES)]
    Prepare(PrepareArgs),
    /// Validate and commit a rebuilt profile answer from stdin JSON
    ///
    /// Checks the optimistic basis, profile rules, redaction and milestone bounds, then commits the refresh atomically.
    #[command(after_help = APPLY_EXAMPLES)]
    Apply(ApplyArgs),
}

/// Arguments to `memory show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// Show this file: profile, or a now or milestone file stem [default: the memory block]
    #[arg(value_name = "ID")]
    id: Option<String>,

    /// Show this stored profile revision instead
    #[arg(long, value_name = "N", conflicts_with = "id")]
    revision: Option<i64>,

    /// Deprecated alias of `memory show` with no id: the memory block
    #[arg(long, conflicts_with_all = ["id", "revision"], hide = true)]
    page: bool,
}

/// Arguments to `memory rules`: none.
#[derive(Debug, Args)]
pub(crate) struct RulesArgs {}

/// Arguments to `memory ack`.
#[derive(Debug, Args)]
pub(crate) struct AckArgs {
    /// The now item's file stem
    id: String,
    /// The session the item was raised in (an id or a unique prefix of it)
    #[arg(long, value_name = "ID")]
    session: SessionId,
    /// Your crew or model name
    #[arg(long, value_name = "NAME")]
    author: AuthorName,
}

/// Arguments to `memory forget`.
#[derive(Debug, Args)]
pub(crate) struct ForgetArgs {
    /// The now item's or milestone's file stem
    id: String,
    /// Your crew or model name
    #[arg(long, value_name = "NAME")]
    author: AuthorName,
    /// Confirm removing it for good; it cannot be undone
    #[arg(long)]
    confirm: bool,
}

/// Arguments to `memory restore`.
#[derive(Debug, Args)]
pub(crate) struct RestoreArgs {
    /// The revision to write back, as `memory show --revision` names it
    #[arg(value_name = "N")]
    revision: i64,
}

/// Arguments to `memory refresh`: none.
#[derive(Debug, Args)]
pub(crate) struct RefreshArgs {}

/// Arguments to `memory prepare`: none.
#[derive(Debug, Args)]
pub(crate) struct PrepareArgs {}

/// Arguments to `memory apply`: none (JSON request is read on stdin).
#[derive(Debug, Args)]
pub(crate) struct ApplyArgs {}

/// Run one `memory` verb.
pub(super) fn run(command: MemoryCommand, ctx: &Context) -> Result<Output, CliError> {
    let mut page = false;
    let command = match command {
        MemoryCommand::Show(args) => {
            page = args.page;
            Command::Show {
                revision: args.revision,
                id: args.id,
            }
        }
        MemoryCommand::Rules(RulesArgs {}) => {
            return Ok(Output::Show {
                record: Record::MemoryRules(Box::new(MemoryRulesPayload::new())),
            });
        }
        MemoryCommand::Ack(args) => Command::Ack {
            id: args.id,
            session: args.session,
            author: args.author,
            provider: ctx.provider(),
        },
        MemoryCommand::Forget(args) => Command::Forget {
            id: args.id,
            author: args.author,
            confirm: args.confirm,
        },
        MemoryCommand::Restore(args) => Command::Restore {
            revision: args.revision,
        },
        MemoryCommand::Refresh(RefreshArgs {}) => Command::Refresh,
        MemoryCommand::Prepare(PrepareArgs {}) => Command::Prepare,
        MemoryCommand::Apply(ApplyArgs {}) => {
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
            Command::Apply {
                provider: ctx.provider(),
                request: Box::new(request),
                routine: false,
            }
        }
    };
    Ok(output(exec(ctx, command)?, ctx, page))
}

/// The output of `outcome`. `page` is `memory show --page`.
fn output(outcome: Outcome, ctx: &Context, page: bool) -> Output {
    match outcome {
        Outcome::Show(shown) => match *shown {
            Shown::Revision(revision) => Output::Show {
                record: Record::MemoryRevision(Box::new(RevisionPayload {
                    revision: revision.n,
                    created: revision.created,
                    text: revision.content,
                })),
            },
            Shown::Item(shown) => Output::Show {
                record: Record::Memory(Box::new(MemoryPayload::from_shown(&shown))),
            },
            Shown::Block(view) => {
                let mut block = MemoryBlockPayload::from_view(&view, ctx.root());
                if page {
                    block.warnings.push(String::from(
                        "warning: memory show --page is deprecated (usage_deprecated): run memory show with no id",
                    ));
                }
                Output::Show {
                    record: Record::MemoryBlock(Box::new(block)),
                }
            }
        },
        Outcome::Acked(item, changed) => {
            Output::written(Record::Memory(Box::new(MemoryPayload::now(&item))), changed)
        }
        Outcome::Forgotten(shown) => Output::written(
            Record::Memory(Box::new(MemoryPayload::from_shown(&shown))),
            true,
        ),
        Outcome::Restored(shown, written) => Output::written(
            Record::Memory(Box::new(MemoryPayload::from_shown(&shown))),
            written,
        ),
        Outcome::RefreshedNothingNew => Output::written(
            Record::MemoryRefresh(Box::new(RefreshPayload {
                result: "nothing_new",
                revision: None,
                sessions: 0,
                backlog: 0,
                milestones: Vec::new(),
                removed: Vec::new(),
            })),
            false,
        ),
        Outcome::Prepared(prepared) => Output::Show {
            record: Record::MemoryPrepare(prepared),
        },
        Outcome::Applied(outcome) => match *outcome {
            ApplyRefreshOutcome::Written(written) => Output::written(
                Record::MemoryRefresh(Box::new(RefreshPayload {
                    result: "written",
                    revision: written.revision,
                    sessions: written.sessions,
                    backlog: written.backlog,
                    milestones: written.milestones,
                    removed: written.removed,
                })),
                true,
            ),
            ApplyRefreshOutcome::Retry { code, correction } => Output::Show {
                record: Record::MemoryRefreshRetry(Box::new(RefreshRetryPayload {
                    status: "retry",
                    code,
                    correction,
                })),
            },
        },
    }
}
