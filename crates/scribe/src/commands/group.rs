//! Quote-checked group opening, caller-bound seat membership and roster reads.

use super::{ListNotice, list_output};
use crate::error::CliError;
use crate::output::{Output, Record, Records};
use clap::Subcommand;
use scribe_core::apps::group::{
    Command, GroupBackfilled, GroupCloseInput, GroupJoinInput, GroupOpenInput, Outcome, exec,
};
use scribe_core::composition::Context;
use scribe_core::{AuthorName, Label, Project, Related};

pub(super) const EXAMPLES: &str = "Examples:
  scribe group open --title 'Release review' --kind review --project scribe --quote 'open a release review'
  scribe group join <id> --as editor --author aide --role reviewer
  scribe group close <id> --summary 'Review complete'
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
    #[command(
        after_help = "Draft with `scribe group evidence <id> --json`, then use --summary and --if-evidence, or run `orbit job run scribe-group-close`.\nExample: scribe group close <id> --summary 'Review complete'"
    )]
    Close {
        /// Group id, unique prefix, or project group slug
        id: String,
        /// Words the user typed in this session that contain the whole group id (g-1
        /// does not match g-12); closes the group for a session that holds no seat in it
        #[arg(long)]
        quote: Option<String>,
        /// Summary text, or - to read stdin
        #[arg(long, conflicts_with = "no_summary")]
        summary: Option<String>,
        /// Close without a summary
        #[arg(long)]
        no_summary: bool,
        /// Refuse if the evidence has changed
        #[arg(long)]
        if_evidence: Option<String>,
    },
    /// Read bounded evidence for drafting a close summary
    Evidence {
        /// Group id, unique prefix, or project group slug
        id: String,
        /// Words the user typed in this session naming the group
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

pub(super) fn run(command: GroupCommand, ctx: &Context) -> Result<Output, CliError> {
    let command = match command {
        GroupCommand::Open {
            title,
            kind,
            project,
            related,
            quote,
        } => Command::Open(GroupOpenInput {
            session: ctx.caller(),
            title,
            kind,
            project,
            related,
            quote,
            provider: ctx.provider(),
            notes: ctx,
        }),
        GroupCommand::Join {
            id,
            seat,
            author,
            role,
            replace,
        } => Command::Join(GroupJoinInput {
            session: ctx.caller(),
            id,
            seat,
            author,
            role,
            replace,
            provider: ctx.provider(),
        }),
        GroupCommand::Backfill { id } => Command::Backfill {
            id,
            provider: ctx.provider(),
        },
        GroupCommand::Leave => Command::Leave {
            session: ctx.caller(),
            provider: ctx.provider(),
        },
        GroupCommand::Close {
            id,
            quote,
            summary,
            no_summary: _,
            if_evidence,
        } => Command::Close(GroupCloseInput {
            session: ctx.caller(),
            id,
            quote,
            provider: ctx.provider(),
            summary: match summary {
                Some(s) if s == "-" => {
                    use std::io::Read;
                    let mut text = String::new();
                    std::io::stdin()
                        .read_to_string(&mut text)
                        .map_err(|source| CliError::Read {
                            what: "stdin".into(),
                            source,
                        })?;
                    Some(text)
                }
                other => other,
            },
            if_evidence,
            notes: ctx,
        }),
        GroupCommand::Evidence { id, quote } => Command::Evidence {
            session: ctx.caller(),
            provider: ctx.provider(),
            id,
            quote,
        },
        GroupCommand::Show { id } => Command::Show(id),
        GroupCommand::List => Command::List,
    };
    Ok(output(exec(ctx, command)?))
}

/// A `group` outcome as its output.
fn output(outcome: Outcome) -> Output {
    match outcome {
        Outcome::Written(written) => {
            let written = *written;
            Output::Written {
                record: Record::Group(Box::new(written.group)),
                changed: written.changed,
                notices: written.notices,
            }
        }
        Outcome::Backfilled(report) => backfilled(*report),
        Outcome::Evidence(value) => Output::GroupEvidence(value),
        Outcome::Shown(group) => Output::Show {
            record: Record::Group(group),
        },
        Outcome::Listed(groups) => {
            let total = groups.len();
            list_output(
                Records::Groups(groups),
                total,
                &[],
                &ListNotice {
                    noun: "groups",
                    criteria: Vec::new(),
                    empty: "no session groups in the store",
                },
            )
        }
    }
}

/// What `group backfill` stored.
fn backfilled(report: GroupBackfilled) -> Output {
    Output::GroupBackfill(crate::output::GroupBackfill {
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
    })
}
