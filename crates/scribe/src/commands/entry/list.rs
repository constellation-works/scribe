use super::output;
use super::{parse_instant, state_parser, status_parser};
use crate::commands::{AuthorKindArg, DateArgs};
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::entry::{self, Command, exec};
use scribe_core::composition::Context;
use scribe_core::{
    AuthorKind, AuthorName, EntryFilter, EntryStatus, ItemState, Project, Related, TagName, Topic,
};
use std::num::NonZeroUsize;

pub(super) const LIST_EXAMPLES: &str = "\
Examples:
  scribe entry list
  scribe entry list --tag preferences --status active
  scribe entry list --topic ci --limit 10
  scribe entry list --since 2026-10-01 --until 2026-10-31 --author-kind user
  scribe entry list --related task:<id> --related pr:<owner>/<repo>#<n>
  scribe entry list --tag standard --after 2026-11-15T09:00:00Z --window --json
  scribe entry list --tag open-item --state open --state answered
  scribe entry list --json | jq '.entries[].content'
  scribe entry list | cut -f1,6";

/// Arguments to `entry list`.
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Only entries filed under this tag's name or alias [default: any]
    #[arg(long, value_name = "TAG")]
    pub(crate) tag: Option<TagName>,

    /// Only entries with this status [default: any]
    #[arg(long, value_name = "STATUS", value_parser = status_parser())]
    pub(crate) status: Option<EntryStatus>,

    /// Only entries recorded in this project (a repository slug, or global) [default: any]
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: Option<Project>,

    /// Only entries of member sessions in this group (id, unique prefix, or project group slug)
    #[arg(long, value_name = "ID")]
    pub(crate) group: Option<String>,

    /// Only entries carrying this topic; repeat it to require every one [default: any]
    #[arg(long, value_name = "TOPIC")]
    pub(crate) topic: Vec<Topic>,

    /// Only entries mentioning this reference (task:<id>, pr:<owner>/<repo>#<n>, repo:<slug>, doc:<repo>/<path>#<anchor>, or session:<id>, whose id may be a unique prefix); repeat it to accept any of them [default: any]
    #[arg(long, value_name = "REF")]
    pub(crate) related: Vec<Related>,

    #[command(flatten)]
    pub(crate) dates: DateArgs,

    /// Only entries created strictly after this instant (RFC 3339, such as 2026-11-15T09:00:00Z): an intake's cursor [default: any]
    #[arg(long, value_name = "TIME", value_parser = parse_instant)]
    pub(crate) after: Option<time::OffsetDateTime>,

    /// Include each entry's transcript window (an action entry's chat before its #scribe line) [default: left out]
    #[arg(long)]
    pub(crate) window: bool,

    /// Only entries written under this author name, such as user or an agent's --author [default: any]
    #[arg(long, value_name = "NAME")]
    pub(crate) author: Option<AuthorName>,

    /// Only entries written through this channel: user (a #scribe line or an approval), agent (entry add) or scribe (an idea the session pass captured) [default: any]
    #[arg(long, value_name = "KIND", value_enum)]
    pub(in crate::commands) author_kind: Option<AuthorKindArg>,

    /// Only open items in this derived state; repeat it to accept any of them [default: any entry]
    #[arg(long, value_name = "STATE", value_parser = state_parser())]
    pub(crate) state: Vec<ItemState>,

    /// Show at most this many of the matching entries, 1 or more; --json reports the total and whether the list was cut [default: all]
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<NonZeroUsize>,
}

pub(super) fn run(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let input = list_input(&args, ctx);
    let outcome = exec(ctx, Command::List(Box::new(input)))?;
    let display = output::ListDisplay {
        related: &args.related,
        dates: &args.dates,
        after: args.after,
        window: args.window,
    };
    Ok(output::output(outcome, false, Some(display), ctx))
}

fn list_input(args: &ListArgs, ctx: &Context) -> entry::ListInput {
    entry::ListInput {
        filter: EntryFilter {
            tag: args.tag.clone(),
            status: args.status,
            project: args.project.clone(),
            topics: args.topic.clone(),
            related: args.related.clone(),
            dates: scribe_core::DateRange::default(),
            after: args.after,
            author: args.author.clone().map(scribe_core::AuthorName::current),
            author_kind: args.author_kind.map(AuthorKind::from),
            states: args.state.clone(),
            limit: args.limit,
        },
        since: args.dates.since,
        until: args.dates.until,
        group: args.group.clone(),
        provider: ctx.provider(),
    }
}
