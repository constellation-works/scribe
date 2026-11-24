//! The `question` noun: `ask`, which answers a question from the store with
//! verbatim-checked quotes (docs/design/scribe/2_design.md §6).
//!
//! [`answer_with`] is the one implementation behind both `question ask` and
//! the Orbit tool `scribe.ask` (`plugin call`), so the two return the same
//! payload for the same store. It reads the store once (entries, session
//! records and `control.json`, no lock, no write), then runs
//! `scribe_core::ask::answer`: a lookup answers from the stored records
//! with no model call, otherwise retrieval packs evidence for the `ask`
//! model. Read-only, so never audited (STD-01 R31).

use crate::app::Context;
use crate::error::CliError;
use crate::output::{AskPayload, Output, SessionsContext};
use clap::{Args, Subcommand, ValueEnum};
use scribe_core::ask::{self, Corpus, NoTasks, Question, RecordKind, Scope, Tasks};
use scribe_core::group::Group;
use scribe_core::{Config, EntryFilter, Models, Project, SessionFilter, Store, TagName, Topic};
use time::{Date, OffsetDateTime};

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
            let config = ctx.config()?;
            let group = args
                .group
                .as_deref()
                .map(|id| ctx.store().read().groups().get(id))
                .transpose()?;
            let orbit = ctx.tasks();
            let tasks: &dyn Tasks = if group.is_some() { &NoTasks } else { &orbit };
            let payload = answer_with_config(
                &config,
                ctx.store(),
                question,
                |config| ctx.models(config),
                tasks,
                ctx.now(),
                group.as_ref(),
            )?;
            Ok(Output::Ask(Box::new(payload)))
        }
    }
}

/// Answer `question` from `store`, with the `Models` adapter `models` builds
/// for the store's config and the task source `tasks`. Tag names in the
/// scope are resolved through the configured aliases.
pub(super) fn answer_with<M: Models, T: Tasks + ?Sized>(
    store: &Store,
    question: Question,
    models: impl FnOnce(&Config) -> M,
    tasks: &T,
    now: OffsetDateTime,
) -> Result<AskPayload, CliError> {
    let config = store.read().tags().config()?;
    answer_with_config(&config, store, question, models, tasks, now, None)
}

fn answer_with_config<M: Models, T: Tasks + ?Sized>(
    config: &Config,
    store: &Store,
    mut question: Question,
    models: impl FnOnce(&Config) -> M,
    tasks: &T,
    now: OffsetDateTime,
    group: Option<&Group>,
) -> Result<AskPayload, CliError> {
    let members = group.map(Group::members);
    let entries = store
        .read()
        .entries()
        .list_scoped(&EntryFilter::default(), members.as_ref())?;
    let sessions = store.read().sessions().list_scoped(
        &SessionFilter {
            include_inactive: group.is_some(),
            ..SessionFilter::default()
        },
        members.as_ref(),
    )?;
    let messages = if let Some(group) = group {
        store
            .read()
            .groups()
            .search_messages(&group.id, &ask::tokens(&question.text))?
            .matches
    } else {
        Vec::new()
    };
    let mut historical = Vec::new();
    for tag in &mut question.scope.tags {
        if let Some(def) = config.tags.resolve(tag.as_str()) {
            // The new alias also names a historical tag with its own entries.
            if tag.as_str() == "decisions" && def.name.as_str() == "decision" {
                historical.push(tag.clone());
            }
            *tag = def.name.clone();
        }
    }
    question.scope.tags.extend(historical);
    let corpus = Corpus {
        entries: &entries.listing.items,
        sessions: &sessions.listing.items,
        messages: &messages,
        tags: &config.tags,
        user: &config.user,
    };
    let adapter = models(config);
    let answer = ask::answer(
        &question,
        corpus,
        &adapter,
        tasks,
        &config.models.ask,
        &config.voice(scribe_core::voice::ModelUse::Ask),
        now,
    )?;
    let mut skipped = entries.skipped;
    skipped.extend(sessions.skipped);
    let context = SessionsContext {
        control: &sessions.control,
        children: &sessions.children,
        titles: &sessions.titles,
        numbers: &sessions.numbers,
        forgotten: &sessions.forgotten,
        entries: &entries.listing.items,
    };
    Ok(AskPayload::new(&answer, &context, &skipped))
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
