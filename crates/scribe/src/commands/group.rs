//! Quote-checked group opening, caller-bound seat membership and roster reads.

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{Output, Record, Records};
use clap::Subcommand;
use scribe_core::cases::group_close::{CLOSE_COMMAND, GroupCloseInput};
use scribe_core::cases::group_join::GroupJoinInput;
use scribe_core::cases::group_open::GroupOpenInput;
use scribe_core::cases::{group_backfill, group_close, group_join, group_leave, group_open};
use scribe_core::{AuthorName, Config, Label, Project, Provider, Related, SessionId};

pub(super) const EXAMPLES: &str = "Examples:
  scribe group open --title 'Release review' --kind review --project scribe --quote 'open a release review'
  scribe group join <id> --as editor --author aide --role reviewer
  scribe group close <id>
  scribe group show <id> --json";

#[derive(Debug, Subcommand)]
pub(crate) enum GroupCommand {
    /// Open a group on a verbatim quote from the calling session
    #[command(
        after_help = "Example: scribe group open --title 'Release review' --kind review --quote 'open a release review'"
    )]
    Open {
        /// Title of the work
        #[arg(long)]
        title: String,
        /// Free label for the kind of work
        #[arg(long)]
        kind: String,
        /// Project slug
        #[arg(long, default_value = "global")]
        project: Project,
        /// Task, PR or repository reference, repeatable
        #[arg(long)]
        related: Vec<Related>,
        /// Words the user typed in this session
        #[arg(long)]
        quote: Option<String>,
    },
    /// Join a named seat under your own author name
    #[command(
        after_help = "Example: scribe group join <id> --as editor --author aide --role reviewer"
    )]
    Join {
        /// Group id, unique prefix, or project group slug
        id: String,
        /// Seat name (defaults to the short session id for a project group)
        #[arg(long = "as", value_name = "SEAT", value_parser = seat_name)]
        seat: Option<Label>,
        /// Your agent author name
        #[arg(long)]
        author: AuthorName,
        /// Role; defaults to member for a new project-group seat, otherwise the seat name.
        /// Omission on an existing seat keeps its current role.
        #[arg(long)]
        role: Option<String>,
        /// Explicitly replace the newest holder of an occupied seat
        #[arg(long)]
        replace: bool,
    },
    /// Store earlier typed turns for each current seat holder
    #[command(after_help = "Example: scribe group backfill <id> --json")]
    Backfill {
        /// Group id, unique prefix, or project group slug
        id: String,
    },
    /// Remove the calling session from its seat
    #[command(after_help = "Example: scribe group leave")]
    Leave,
    /// Summarize and close the group from its opener, seat holder or a quote
    #[command(after_help = "Example: scribe group close <id>")]
    Close {
        /// Group id, unique prefix, or project group slug
        id: String,
        /// Words the user typed in this session that contain the whole group id (g-1
        /// does not match g-12); closes the group for a session that holds no seat in it
        #[arg(long)]
        quote: Option<String>,
    },
    /// Show one group, its references and its seat chains
    #[command(after_help = "Example: scribe group show <id> --json")]
    Show {
        /// Group id, unique prefix, or project group slug
        id: String,
    },
    /// List groups newest first with their rosters
    #[command(after_help = "Example: scribe group list --json")]
    List,
}

fn seat_name(value: &str) -> Result<Label, scribe_core::RecordError> {
    Label::try_from(value.to_owned())
}

impl GroupCommand {
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        let command = match self {
            Self::Open { .. } => "group open",
            Self::Join { .. } => "group join",
            Self::Leave => "group leave",
            Self::Backfill { .. } => "group backfill",
            Self::Close { .. } => CLOSE_COMMAND,
            Self::List | Self::Show { .. } => return None,
        };
        Some(AuditMeta {
            command,
            stepwise: matches!(self, Self::Close { .. }),
        })
    }

    pub(super) fn subject(&self, ctx: &Context) -> AuditSubject {
        match self {
            Self::Join { id, author, .. } => AuditSubject {
                author: Some(author.to_string()),
                target: ctx.store().read().groups().get(id).ok().map(|g| g.id),
            },
            Self::Close { id, .. } | Self::Backfill { id } => AuditSubject {
                author: None,
                target: ctx.store().read().groups().get(id).ok().map(|g| g.id),
            },
            Self::Open { .. } | Self::Leave | Self::List | Self::Show { .. } => {
                AuditSubject::default()
            }
        }
    }
}

fn caller(ctx: &Context) -> Result<SessionId, CliError> {
    let key = match ctx.provider() {
        Provider::ClaudeCode => "CLAUDE_CODE_SESSION_ID",
        Provider::Codex => "CODEX_THREAD_ID",
    };
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            CliError::Usage(format!(
                "run this command in the calling session with a valid {key}"
            ))
        })
}

pub(super) fn run(command: GroupCommand, ctx: &Context) -> Result<Output, CliError> {
    let store = ctx.store();
    let (id, changed, notices) = match command {
        GroupCommand::Open {
            title,
            kind,
            project,
            related,
            quote,
        } => {
            let session = caller(ctx)?;
            let opened = group_open::run(
                &ctx.core(),
                GroupOpenInput {
                    session,
                    title,
                    kind,
                    project,
                    related,
                    quote,
                    provider: ctx.provider(),
                    notes: ctx,
                },
            )?;
            (opened.id, true, opened.notices)
        }
        GroupCommand::Join {
            id,
            seat,
            author,
            role,
            replace,
        } => {
            let session = caller(ctx)?;
            let joined = group_join::run(
                &ctx.core(),
                GroupJoinInput {
                    session,
                    id,
                    seat,
                    author,
                    role,
                    replace,
                    provider: ctx.provider(),
                },
            )?;
            (joined.id, joined.changed, joined.notices)
        }
        GroupCommand::Backfill { id } => {
            let report = group_backfill::run(&ctx.core(), &id, ctx.provider())?;
            return Ok(Output::GroupBackfill(crate::output::GroupBackfill {
                group: report.group,
                members: report
                    .members
                    .into_iter()
                    .map(|m| crate::output::GroupBackfillMember {
                        seat: m.seat,
                        session: m.session,
                        stored: m.stored,
                        skipped: m.skipped,
                        transcript_unavailable: m.transcript_unavailable,
                    })
                    .collect(),
                stored: report.stored,
                skipped: report.skipped,
            }));
        }
        GroupCommand::Leave => {
            let session = caller(ctx)?;
            (group_leave::run(&ctx.core(), &session)?, true, Vec::new())
        }
        GroupCommand::Close { id, quote } => {
            let session = caller(ctx)?;
            let closed = group_close::run(
                &ctx.core(),
                GroupCloseInput {
                    session,
                    id,
                    quote,
                    provider: ctx.provider(),
                    models: |config: &Config| ctx.models(config),
                    notes: ctx,
                },
            )?;
            (closed.id, closed.changed, closed.notices)
        }
        GroupCommand::Show { id } => {
            return Ok(Output::Show {
                record: Record::Group(Box::new(store.read().groups().get(&id)?)),
            });
        }
        GroupCommand::List => {
            let groups = store.read().groups().all()?;
            let total = groups.len();
            return Ok(list_output(
                Records::Groups(groups),
                total,
                &[],
                &ListNotice {
                    noun: "groups",
                    criteria: Vec::new(),
                    empty: "no session groups in the store",
                },
            ));
        }
    };
    Ok(Output::Written {
        record: Record::Group(Box::new(store.read().groups().get(&id)?)),
        changed,
        notices,
    })
}
