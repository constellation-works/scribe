//! `scribe memory`: read what scribe has noticed about the user, and the
//! agent writes on it (docs/design/scribe/2_design.md §13.4, §13.6, §13.9).
//!
//! `show` and `rules` read only: no lock, no audit. `ack`, `retire` and `forget` are
//! audited agent writes. Items are scribe-authored, so an agent retires or
//! forgets one without the user's instruction check, as it may an agent note.

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{
    MemoryPagePayload, MemoryPayload, MemoryRulesPayload, Output, Record, Records,
};
use clap::{Args, Subcommand};
use scribe_core::memory::{self, MemoryItem, MemoryStatus, RecentSession, Section};
use scribe_core::{AuthorName, Project, SessionFilter, SessionId};

pub(super) const MEMORY_EXAMPLES: &str = "\
Memory is written by the session pass of a session switched on with
/scribe on, from the user's own turns; nothing else writes it. It describes,
it never binds: rules are their preferences entries.

Examples:
  scribe memory show
  scribe memory show --section now
  scribe memory show <id>
  scribe memory show --page
  scribe memory rules
  scribe memory ack <id> --session <id> --author opus";

const SHOW_EXAMPLES: &str = "\
Lists the items on the session-start page, by section; --include-off-page
adds decayed, past-window, retired and forgotten items, each labelled with
why. With an id, prints that item with every occurrence and its quote.

Examples:
  scribe memory show
  scribe memory show --section working --include-off-page
  scribe memory show --project scribe
  scribe memory show <id> --json
  scribe memory show --page";

const RULES_EXAMPLES: &str = "\
Prints the rules that promote a now item to milestones and the windows
after which an item leaves the session-start page, each with its value
and where it came from: shipped, or config with the file and line. Set
them under [memory] and [memory.promote] in config.toml.

Examples:
  scribe memory rules
  scribe memory rules --json | jq '.rules[] | {rule, value}'";

const ACK_EXAMPLES: &str = "\
Run it after raising a now item the user did not bring up themselves, so the
next session does not raise it again unprompted.

Examples:
  scribe memory ack <id> --session <id> --author opus";

const RETIRE_EXAMPLES: &str = "\
Hides an item from the page, lists and ask; its text is kept. Run it when
the user says an item is wrong; the next time they say it, a fresh item opens.

Examples:
  scribe memory retire <id> --author opus --reason 'The user says this is wrong'";

const FORGET_EXAMPLES: &str = "\
Blanks the item's statement and every quote for good (or one occurrence's
quote, <id>#<n>); the id, section, dates and authors stay, and no file is
removed. It cannot be undone, so it needs --confirm.

Examples:
  scribe memory forget <id> --author opus --confirm
  scribe memory forget <id>#2 --author opus --confirm";

/// Verbs on the `memory` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum MemoryCommand {
    /// Show session memory items, one item, or the page
    ///
    /// Shows memory items on the session-start page, one item in full, or the page itself.
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
    /// Show active memory promotion rules and windows
    ///
    /// Shows the promotion rules and page windows in effect, each with its origin.
    #[command(after_help = RULES_EXAMPLES)]
    Rules(RulesArgs),
    /// Mark a now item as raised in this session
    ///
    /// Marks a now item as raised in this session, so it is not raised again unprompted.
    #[command(after_help = ACK_EXAMPLES)]
    Ack(AckArgs),
    /// Retire an item with a reason: off the page, lists and ask, text kept
    #[command(after_help = RETIRE_EXAMPLES)]
    Retire(RetireArgs),
    /// Forget a memory item or occurrence for good
    ///
    /// Forgets an item, or one occurrence, for good: blanks its words and keeps its id.
    #[command(after_help = FORGET_EXAMPLES)]
    Forget(ForgetArgs),
}

/// Arguments to `memory show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// Show this item with every occurrence (its id or a unique prefix of at least four characters) [default: list the items]
    #[arg(value_name = "ID")]
    id: Option<String>,

    /// Only items of this section: profile, projects, working, now or milestones [default: every section]
    #[arg(long, value_name = "SECTION", conflicts_with_all = ["id", "page"])]
    section: Option<Section>,

    /// Only projects items of this project, and the page's project part for it [default: any project; for --page, the current directory's]
    #[arg(long, value_name = "PROJECT", conflicts_with = "id")]
    project: Option<Project>,

    /// Also list items off the page (decayed, past their window, retired, forgotten, section off), labelled with why [default: on-page items only]
    #[arg(long, conflicts_with_all = ["id", "page"])]
    include_off_page: bool,

    /// Print exactly the page the session-start hook injects into an interactive session
    #[arg(long, conflicts_with = "id")]
    page: bool,
}

/// Arguments to `memory rules`: none.
#[derive(Debug, Args)]
pub(crate) struct RulesArgs {}

/// Arguments to `memory ack`.
#[derive(Debug, Args)]
pub(crate) struct AckArgs {
    /// The now item (its id or a unique prefix of at least four characters)
    id: String,
    /// The session the item was raised in
    #[arg(long, value_name = "ID")]
    session: SessionId,
    /// Your crew or model name
    #[arg(long, value_name = "NAME")]
    author: AuthorName,
}

/// Arguments to `memory retire`.
#[derive(Debug, Args)]
pub(crate) struct RetireArgs {
    /// The item (its id or a unique prefix of at least four characters)
    id: String,
    /// Your crew or model name
    #[arg(long, value_name = "NAME")]
    author: AuthorName,
    /// Why it no longer applies, 1-300 characters
    #[arg(long, value_name = "TEXT")]
    reason: String,
}

/// Arguments to `memory forget`.
#[derive(Debug, Args)]
pub(crate) struct ForgetArgs {
    /// The item, or <id>#<n> for one occurrence
    id: String,
    /// Your crew or model name
    #[arg(long, value_name = "NAME")]
    author: AuthorName,
    /// Confirm blanking the words for good; it cannot be undone
    #[arg(long)]
    confirm: bool,
}

impl MemoryCommand {
    /// The item and agent a write names. See [`super::Command::subject`].
    pub(super) fn subject(&self, ctx: &Context) -> AuditSubject {
        let of = |id: &str, author: &AuthorName| AuditSubject {
            target: ctx
                .store()
                .resolve_memory(split_occurrence(id).0)
                .ok()
                .map(|item| format!("memory:{}", item.id)),
            author: Some(author.as_str().to_owned()),
        };
        match self {
            Self::Show(_) | Self::Rules(_) => AuditSubject::default(),
            Self::Ack(args) => of(&args.id, &args.author),
            Self::Retire(args) => of(&args.id, &args.author),
            Self::Forget(args) => of(&args.id, &args.author),
        }
    }

    /// Audit every write; `show` is read-only. See [`super::Command::audit`].
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Show(_) | Self::Rules(_) => None,
            Self::Ack(_) => Some(AuditMeta {
                command: "memory ack",
            }),
            Self::Retire(_) => Some(AuditMeta {
                command: "memory retire",
            }),
            Self::Forget(_) => Some(AuditMeta {
                command: "memory forget",
            }),
        }
    }
}

/// Run one `memory` verb.
pub(super) fn run(command: MemoryCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        MemoryCommand::Show(args) => show(&args, ctx),
        MemoryCommand::Rules(RulesArgs {}) => {
            let config = ctx.config()?;
            let file = ctx.store().config_path().display().to_string();
            Ok(Output::Show {
                record: Record::MemoryRules(Box::new(MemoryRulesPayload::new(
                    &config.memory,
                    &file,
                ))),
            })
        }
        MemoryCommand::Ack(args) => {
            let (item, changed) =
                ctx.store()
                    .ack_memory(&args.id, args.session, args.author, ctx.now())?;
            written(ctx, &item, changed)
        }
        MemoryCommand::Retire(args) => {
            let config = ctx.config()?;
            let redactor = ctx.redactor(&config)?;
            let (item, changed) = ctx.store().retire_memory(
                &args.id,
                args.author,
                &args.reason,
                &redactor,
                ctx.now(),
            )?;
            written(ctx, &item, changed)
        }
        MemoryCommand::Forget(args) => {
            let (id, occurrence) = split_occurrence(&args.id);
            let occurrence = occurrence
                .map(|n| {
                    n.parse::<u32>().ok().filter(|n| *n > 0).ok_or_else(|| {
                        CliError::Usage(format!(
                            "'{}' is not <id> or <id>#<n> with n a positive number",
                            args.id
                        ))
                    })
                })
                .transpose()?;
            if !args.confirm {
                let item = ctx.store().resolve_memory(id)?;
                let target = match occurrence {
                    Some(n) => format!("{}#{n}", item.id),
                    None => item.id.to_string(),
                };
                return Err(CliError::ForgetConfirmRequired(target));
            }
            let (item, changed) =
                ctx.store()
                    .forget_memory(id, occurrence, args.author, ctx.now())?;
            written(ctx, &item, changed)
        }
    }
}

/// `<id>#<n>` split into the id and the occurrence number's text.
fn split_occurrence(input: &str) -> (&str, Option<&str>) {
    match input.split_once('#') {
        Some((id, n)) => (id, Some(n)),
        None => (input, None),
    }
}

fn written(ctx: &Context, item: &MemoryItem, changed: bool) -> Result<Output, CliError> {
    let config = ctx.config()?;
    let payload = MemoryPayload::from_item(item, &config.memory, ctx.now().date());
    Ok(Output::written(Record::Memory(Box::new(payload)), changed))
}

fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let config = ctx.config()?;
    let today = ctx.now().date();
    if args.page {
        let project = args.project.clone().or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|cwd| super::hook::project::of(&cwd))
        });
        let page = page(ctx, None, project.as_ref())?;
        return Ok(Output::Show {
            record: Record::MemoryPage(Box::new(MemoryPagePayload {
                chars: page.as_ref().map_or(0, |p| p.text.chars().count()),
                over_budget: page.as_ref().is_some_and(|p| p.over_budget),
                page: page.map(|p| p.text),
            })),
        });
    }
    if let Some(id) = &args.id {
        let item = ctx.store().resolve_memory(id)?;
        return Ok(Output::Show {
            record: Record::Memory(Box::new(MemoryPayload::from_item(
                &item,
                &config.memory,
                today,
            ))),
        });
    }
    let (items, skipped) = ctx.store().memory_items()?;
    let shown: Vec<MemoryPayload> = items
        .iter()
        .filter(|i| args.section.is_none_or(|s| i.section == s))
        .filter(|i| {
            args.project
                .as_ref()
                .is_none_or(|p| i.section == Section::Projects && i.project.as_ref() == Some(p))
        })
        .filter(|i| args.include_off_page || memory::off_page(i, &config.memory, today).is_none())
        .map(|i| MemoryPayload::from_item(i, &config.memory, today))
        .collect();
    let mut sorted = shown;
    sorted.sort_by_key(|p| (section_rank(p.section), p.number));
    let empty = format!(
        "no memory on the page in {}; it is written by the session pass of a session switched on with /scribe on",
        ctx.store().root().display()
    );
    let mut criteria = Vec::new();
    if let Some(section) = args.section {
        criteria.push(format!("--section {section}"));
    }
    if let Some(project) = &args.project {
        criteria.push(format!("--project {project}"));
    }
    if args.include_off_page {
        criteria.push(String::from("--include-off-page"));
    }
    let notice = ListNotice {
        noun: "memory items",
        criteria,
        empty: &empty,
    };
    let total = sorted.len();
    Ok(list_output(
        Records::Memory(sorted),
        total,
        &skipped,
        &notice,
    ))
}

/// The section's place on the page.
fn section_rank(section: &str) -> usize {
    Section::ALL
        .iter()
        .position(|s| s.as_str() == section)
        .unwrap_or(usize::MAX)
}

/// The session-start page for a session (`session`, left out of the recent
/// sessions) in `project`, or `None` when memory is switched off or there
/// is nothing to show (§13.6). Read-only: no lock, no write.
pub(super) fn page(
    ctx: &Context,
    session: Option<&SessionId>,
    project: Option<&Project>,
) -> Result<Option<memory::Page>, CliError> {
    let config = ctx.config()?;
    if !config.memory.enabled {
        return Ok(None);
    }
    let (items, _) = ctx.store().memory_items()?;
    let items: Vec<MemoryItem> = items
        .into_iter()
        .filter(|i| i.status == MemoryStatus::Active)
        .collect();
    let mut recent = Vec::new();
    if config.memory.recent_sessions > 0 {
        let listed = ctx.store().list_sessions(&SessionFilter::default())?;
        recent = listed
            .listing
            .items
            .iter()
            .filter(|r| r.active && session.is_none_or(|s| &r.id != s))
            .map(|r| RecentSession {
                id: r.id.clone(),
                title: r.title.clone(),
                at: r.log.last().map_or(r.updated, |s| s.to),
            })
            .collect();
        recent.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
        recent.truncate(config.memory.recent_sessions);
    }
    Ok(memory::page(
        &items,
        &recent,
        project,
        &config.memory,
        ctx.now().date(),
    ))
}
