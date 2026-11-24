//! The `question` noun: `ask`, which answers a question from the store with
//! verbatim-checked quotes (docs/design/scribe/2_design.md §6).
//!
//! [`answer`] is the one implementation behind both `question ask` and
//! the Orbit tool `scribe.ask` (`plugin call`), so the two return the same
//! payload for the same store; the flow is [`scribe_core::cases::ask`].
//! Read-only, so never audited (STD-01 R31).

use crate::app::Context;
use crate::error::CliError;
use crate::output::{AskPayload, Output, SessionsContext};
use clap::{Args, Subcommand, ValueEnum};
use scribe_core::ask::{self, Question, RecordKind, Scope};
use scribe_core::cases::ask::{self as case, AskInput};
use scribe_core::{Config, Models, Project, TagName, Topic};
use time::Date;

pub(super) const QUESTION_EXAMPLES: &str = "\
Examples:
  scribe question ask \"how do we merge agent PRs?\"
  scribe question ask \"can you fetch me last session summary?\" --json";

pub(super) const ASK_EXAMPLES: &str = "\
Lookups (the newest session, a named session, open next steps, a tag's
entries today or this week, month or year, a term the user defined, the last
on-call session for a workspace) return the stored records with no model
call. Anything else is answered by the configured ask model from the
records that match, and every quote it gives is checked verbatim against
the record it cites; claims that fail are dropped and counted.

Examples:
  scribe question ask \"how do we merge agent PRs?\"
  scribe ask \"what remains for the release?\" --group <id>
  scribe question ask \"preferences this week\" --json | jq '.records[].entry.content'
  scribe question ask \"what do I prefer for releases?\" --tag preferences --since 2026-01-01
  scribe question ask \"merge policy\" --include-hidden --json | jq .claims";

/// Verbs on the `question` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum QuestionCommand {
    /// Answer a question from stored records and Orbit tasks
    ///
    /// Answers a question from the recorded entries, session records and Orbit tasks, citing each quote.
    #[command(after_help = ASK_EXAMPLES)]
    Ask(AskArgs),
}

/// Which record kind `--kind` keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum KindArg {
    /// Entries only
    Entry,
    /// Session records only
    Session,
}

/// Arguments to `question ask`.
#[derive(Debug, Args)]
pub(crate) struct AskArgs {
    /// The question, 1 to 2000 characters
    #[arg(value_name = "QUESTION", value_parser = question_parser)]
    pub(crate) question: String,

    /// Only entries in this tag (or alias); repeat for several. Leaves out session records and Orbit tasks [default: any]
    #[arg(long, value_name = "TAG")]
    pub(crate) tag: Vec<TagName>,

    /// Only records carrying this topic; repeat for several. Leaves out Orbit tasks [default: any]
    #[arg(long, value_name = "TOPIC")]
    pub(crate) topic: Vec<Topic>,

    /// Only records of this project (a repository slug) [default: any]
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: Option<Project>,

    /// Use only records of this group's members, including stored turns (id, unique prefix, or project group slug)
    #[arg(long, value_name = "ID")]
    pub(crate) group: Option<String>,

    /// Only this kind of record. Leaves out Orbit tasks [default: any]
    #[arg(long, value_enum, value_name = "KIND")]
    pub(crate) kind: Option<KindArg>,

    /// Only records changed on or after this day, YYYY-MM-DD [default: any]
    #[arg(long, value_name = "DATE", value_parser = date_parser)]
    pub(crate) since: Option<Date>,

    /// Include superseded and retired entries as evidence [default: active only]
    #[arg(long)]
    pub(crate) include_hidden: bool,
}

impl QuestionCommand {
    /// `None`: `ask` reads only (STD-01 R31).
    pub(super) fn audit(&self) -> Option<crate::audit_middleware::AuditMeta> {
        match self {
            Self::Ask(_) => None,
        }
    }
}

/// Run one `question` verb.
pub(super) fn run(command: QuestionCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        QuestionCommand::Ask(args) => {
            let question = Question {
                text: args.question,
                scope: Scope {
                    tags: args.tag,
                    topics: args.topic,
                    project: args.project,
                    kind: args.kind.map(|k| match k {
                        KindArg::Entry => RecordKind::Entry,
                        KindArg::Session => RecordKind::Session,
                    }),
                    since: args.since,
                },
                include_hidden: args.include_hidden,
            };
            let orbit = ctx.tasks();
            let payload = answer(
                ctx,
                AskInput {
                    question,
                    group: args.group.as_deref(),
                    provider: Some(ctx.provider()),
                    models: |config: &Config| ctx.models(config),
                    tasks: &orbit,
                },
            )?;
            Ok(Output::Ask(Box::new(payload)))
        }
    }
}

/// Answer with [`scribe_core::cases::ask::run`] on `ctx`'s store and render
/// the payload: the one implementation behind both `question ask` and the
/// Orbit tool `scribe.ask`.
pub(super) fn answer<M: Models>(
    ctx: &Context,
    input: AskInput<'_, impl FnOnce(&Config) -> M>,
) -> Result<AskPayload, CliError> {
    let asked = case::run(&ctx.core(), input)?;
    let mut skipped = asked.entries.skipped;
    skipped.extend(asked.sessions.skipped);
    let context = SessionsContext {
        control: &asked.sessions.control,
        children: &asked.sessions.children,
        titles: &asked.sessions.titles,
        numbers: &asked.sessions.numbers,
        forgotten: &asked.sessions.forgotten,
        entries: &asked.entries.listing.items,
    };
    Ok(AskPayload::new(&asked.answer, &context, &skipped))
}

/// A question: not blank, at most 2000 characters.
pub(super) fn question_parser(text: &str) -> Result<String, String> {
    ask::check_question(text)
        .map(|()| text.to_owned())
        .map_err(|e| e.to_string())
}

/// `YYYY-MM-DD`.
pub(super) fn date_parser(text: &str) -> Result<Date, String> {
    let format = time::macros::format_description!("[year]-[month]-[day]");
    Date::parse(text, &format).map_err(|_| format!("{text:?} is not a date in the form YYYY-MM-DD"))
}
