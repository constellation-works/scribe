//! The `record` noun: `record search`, every stored record ranked for a few
//! words as `question ask` ranks them, with no model and no Orbit call
//! (docs/design/scribe/2_design.md §6.8). It reads only: no lock, no audit
//! line (STD-01 R31), and one load of the entries, the sessions and the
//! issues.

use super::{DateArgs, ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{HitPayload, Output, Records};
use clap::{Args, Subcommand};
use scribe_core::ask::{Corpus, RecordKind};
use scribe_core::search::{self, DEFAULT_LIMIT, SearchError, SearchQuery};
use scribe_core::{EntryFilter, Project, SessionFilter, Topic};
use std::num::NonZeroUsize;

pub(super) const RECORD_EXAMPLES: &str = "\
Examples:
  scribe record search \"merge queue\"
  scribe record search release --kind session --since 2026-11-01
  scribe record search \"api\" --json | jq -r '.hits[].cite'";

const SEARCH_EXAMPLES: &str = "\
Ranks each entry, each session header, each segment of a session's log and
each issue with BM25, as question ask does, and prints the best first;
the user's own entries weigh 1.5 times. The CITE column is what question ask
would cite: pass it to entry show, session show or issue show. An issue is
the agent's account, with the user's request quoted. Superseded and retired
entries and inactive sessions are left out unless --include-hidden.

Examples:
  scribe record search \"merge queue\"
  scribe record search release --kind session --since 2026-11-01
  scribe record search \"store format\" --project scribe --topic store --limit 5
  scribe record search \"rate limit\" --kind issue
  scribe record search \"api\" --json | jq -r '.hits[].cite'";

/// Verbs on the `record` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum RecordCommand {
    /// Rank stored records for words with no model call
    ///
    /// Ranks entries, session headers, segments and issues for words, best first, with no model call.
    #[command(after_help = SEARCH_EXAMPLES)]
    Search(SearchArgs),
}

/// The record kinds a search can keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum KindArg {
    // Entries only.
    Entry,
    // Session headers and segments only.
    Session,
    // Issue records only.
    Issue,
}

/// Arguments to `record search`.
#[derive(Debug, Args)]
pub(crate) struct SearchArgs {
    /// The words to look for; stopwords and words under three letters are ignored
    #[arg(value_name = "TERMS")]
    pub(crate) terms: String,

    /// Only this kind of record [default: every kind]
    #[arg(long, value_name = "KIND", value_enum)]
    pub(crate) kind: Option<KindArg>,

    /// Only records of this project (a repository slug, or global) [default: any]
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: Option<Project>,

    /// Only records carrying this topic (a segment: its session's or its own); repeat it to require every one [default: any]
    #[arg(long, value_name = "TOPIC")]
    pub(crate) topic: Vec<Topic>,

    /// Only entries created, and sessions and segments running, on a day in the range
    #[command(flatten)]
    pub(crate) dates: DateArgs,

    /// Include superseded and retired entries and inactive sessions
    #[arg(long)]
    pub(crate) include_hidden: bool,

    /// Show at most this many hits, 1 or more; --json reports the total and whether the list was cut [default: 20]
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<NonZeroUsize>,
}

impl RecordCommand {
    /// `record search` reads only. See [`super::Command::audit`].
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Search(_) => None,
        }
    }
}

/// Run one `record` verb.
pub(super) fn run(command: RecordCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        RecordCommand::Search(args) => search(args, ctx),
    }
}

fn search(args: SearchArgs, ctx: &Context) -> Result<Output, CliError> {
    let limit = args
        .limit
        .or(NonZeroUsize::new(DEFAULT_LIMIT))
        .unwrap_or(NonZeroUsize::MIN);
    let query = SearchQuery {
        terms: args.terms,
        kind: args.kind.map(|k| match k {
            KindArg::Entry => RecordKind::Entry,
            KindArg::Session => RecordKind::Session,
            KindArg::Issue => RecordKind::Issue,
        }),
        project: args.project,
        topics: args.topic,
        dates: args.dates.range()?,
        include_hidden: args.include_hidden,
        limit,
    };
    let store = ctx.store();
    let config = store.config()?;
    let entries = store.list_entries(&EntryFilter::default())?;
    let sessions = store.list_sessions(&SessionFilter {
        include_inactive: true,
        ..SessionFilter::default()
    })?;
    let (issues, issues_skipped) = store.issues()?;
    let corpus = Corpus {
        entries: &entries.listing.items,
        sessions: &sessions.listing.items,
        issues: &issues,
        tags: &config.tags,
        user: &config.user,
    };
    let found = search::search(&query, corpus).map_err(|e| match e {
        SearchError::NoTerms(_) => CliError::Usage(e.to_string()),
    })?;
    let mut skipped = entries.skipped;
    skipped.extend(sessions.skipped);
    skipped.extend(issues_skipped);
    let mut criteria = vec![format!("{:?}", query.terms)];
    if let Some(kind) = query.kind {
        criteria.push(format!("--kind {}", kind.as_str()));
    }
    if let Some(project) = &query.project {
        criteria.push(format!("--project {project}"));
    }
    for topic in &query.topics {
        criteria.push(format!("--topic {topic}"));
    }
    criteria.extend(args.dates.criteria());
    let empty = format!(
        "no records in {}; entries come from #scribe lines, sessions from /scribe on and issues from scribe issue add",
        store.root().display()
    );
    let notice = ListNotice {
        noun: "records",
        criteria,
        empty: &empty,
    };
    let records = Records::Hits(found.hits.iter().map(HitPayload::from).collect());
    let mut output = list_output(records, found.total, &skipped, &notice);
    if found.hidden_matches > 0
        && let Output::List { notices, .. } = &mut output
    {
        notices.push(format!(
            "{} hidden record(s) also match: superseded or retired entries, or inactive sessions; --include-hidden shows them",
            found.hidden_matches
        ));
    }
    Ok(output)
}
