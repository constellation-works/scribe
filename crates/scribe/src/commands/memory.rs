//! `scribe memory`: what scribe remembers about the user — the profile,
//! `now` items and milestones (docs/design/scribe/2_design.md §15.4–§15.6).
//!
//! `show` and `rules` read only: no lock, no audit, no replay. `ack`,
//! `forget`, `restore` and `refresh` are audited writes. Memory is
//! scribe-authored, so an agent acknowledges or forgets an item without the
//! user's instruction check, as it may an agent note.

use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{
    MemoryBlockPayload, MemoryPayload, MemoryRulesPayload, Output, Record, RefreshPayload,
    RevisionPayload,
};
use clap::{Args, Subcommand};
use scribe_core::cases::memory_cmd as case;
use scribe_core::{AuthorName, MemoryShown, Models, SessionId};

pub(super) const MEMORY_EXAMPLES: &str = "\
Memory is one fixed shape: memory/profile.md, rebuilt weekly by 'scribe
memory refresh'; memory/now/, dated things the user plans, attends or must
meet, written by the session pass of a session switched on with /scribe on;
and memory/milestones/. It describes, it never binds: rules are their
preferences entries.

Examples:
  scribe memory show
  scribe memory show profile
  scribe memory show 2026-12-01-hackathon-deadline
  scribe memory ack 2026-12-01-hackathon-deadline --session <id> --author opus
  scribe memory refresh";

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
Rebuilds memory/profile.md from the sessions since the last run (at most 40
per run; a longer backlog is taken over several runs), keeps Pinned byte for
byte, and settles now items past their 14 days: each becomes a milestone or
history. It calls models.profile once, within 120 s, and writes nothing when
there is nothing new. Run weekly: the scribe plugin seeds a disabled Orbit
routine, or use cron or launchd.

Examples:
  scribe memory refresh
  scribe memory refresh --json | jq '{result, revision, backlog}'";

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
    /// Rebuild the profile and settle passed now items
    ///
    /// Rebuilds the profile from the sessions since the last run and settles now items past their 14 days.
    #[command(after_help = REFRESH_EXAMPLES)]
    Refresh(RefreshArgs),
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

impl MemoryCommand {
    /// The file and agent a write names. See [`super::Command::subject`].
    pub(super) fn subject(&self, _ctx: &Context) -> AuditSubject {
        let of = |id: &str, author: &AuthorName| AuditSubject {
            target: Some(format!("memory:{}", id.trim())),
            author: Some(author.as_str().to_owned()),
        };
        match self {
            Self::Show(_) | Self::Rules(_) => AuditSubject::default(),
            Self::Ack(args) => of(&args.id, &args.author),
            Self::Forget(args) => of(&args.id, &args.author),
            Self::Restore(_) | Self::Refresh(_) => AuditSubject {
                target: Some(String::from("memory:profile")),
                author: None,
            },
        }
    }

    /// Audit every write; `show` and `rules` are read-only. See
    /// [`super::Command::audit`].
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        let command = match self {
            Self::Show(_) | Self::Rules(_) => return None,
            Self::Ack(_) => "memory ack",
            Self::Forget(_) => "memory forget",
            Self::Restore(_) => "memory restore",
            Self::Refresh(_) => "memory refresh",
        };
        Some(AuditMeta {
            command,
            stepwise: matches!(self, Self::Refresh(_)),
        })
    }
}

/// Run one `memory` verb.
pub(super) fn run(command: MemoryCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        MemoryCommand::Show(args) => show(&args, ctx),
        MemoryCommand::Rules(RulesArgs {}) => Ok(Output::Show {
            record: Record::MemoryRules(Box::new(MemoryRulesPayload::new())),
        }),
        MemoryCommand::Ack(args) => {
            let (item, changed) = case::ack(
                &ctx.core(),
                &args.id,
                &args.session,
                args.author,
                ctx.provider(),
            )?;
            Ok(Output::written(
                Record::Memory(Box::new(MemoryPayload::now(&item))),
                changed,
            ))
        }
        MemoryCommand::Forget(args) => {
            let shown = case::forget(&ctx.core(), &args.id, args.author, args.confirm)?;
            Ok(Output::written(
                Record::Memory(Box::new(MemoryPayload::from_shown(&shown))),
                true,
            ))
        }
        MemoryCommand::Restore(args) => {
            let (shown, written) = case::restore(&ctx.core(), args.revision)?;
            Ok(Output::written(
                Record::Memory(Box::new(MemoryPayload::from_shown(&shown))),
                written,
            ))
        }
        MemoryCommand::Refresh(RefreshArgs {}) => {
            let config = ctx.config()?;
            refresh(ctx, &ctx.models(&config))
        }
    }
}

/// `memory refresh` with the adapter `models` (a test answers with a
/// fake): [`scribe_core::cases::memory_cmd::refresh`].
pub(crate) fn refresh<M: Models>(ctx: &Context, models: &M) -> Result<Output, CliError> {
    let payload = match case::refresh(&ctx.core(), ctx.provider(), models)? {
        None => RefreshPayload {
            result: "nothing_new",
            revision: None,
            sessions: 0,
            backlog: 0,
            milestones: Vec::new(),
            removed: Vec::new(),
        },
        Some(written) => RefreshPayload {
            result: "written",
            revision: written.revision,
            sessions: written.sessions,
            backlog: written.backlog,
            milestones: written.milestones,
            removed: written.removed,
        },
    };
    let changed = payload.result == "written";
    Ok(Output::written(
        Record::MemoryRefresh(Box::new(payload)),
        changed,
    ))
}

fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    if let Some(n) = args.revision {
        let revision = ctx.store().read().memory().profile_revision(n)?;
        return Ok(Output::Show {
            record: Record::MemoryRevision(Box::new(RevisionPayload {
                revision: revision.n,
                created: revision.created,
                text: revision.content,
            })),
        });
    }
    if let Some(id) = &args.id {
        let shown: MemoryShown = ctx.store().read().memory().file(id)?;
        return Ok(Output::Show {
            record: Record::Memory(Box::new(MemoryPayload::from_shown(&shown))),
        });
    }
    let mut block = block(ctx)?;
    if args.page {
        block.warnings.push(String::from(
            "warning: memory show --page is deprecated (usage_deprecated): run memory show with no id",
        ));
    }
    Ok(Output::Show {
        record: Record::MemoryBlock(Box::new(block)),
    })
}

/// The memory block (2_design.md §15.4). Read-only: no lock, no write, no
/// replay.
pub(super) fn block(ctx: &Context) -> Result<MemoryBlockPayload, CliError> {
    let view = ctx.store().read().memory().view()?;
    Ok(MemoryBlockPayload::from_view(&view, ctx.store().root()))
}
