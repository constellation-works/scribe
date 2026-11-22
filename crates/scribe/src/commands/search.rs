//! `search`: every stored record ranked for a few
//! words as `question ask` ranks them, with no model and no Orbit call
//! (docs/design/scribe/2_design.md §6.8). It reads only: no lock, no audit
//! line (STD-01 R31), and one load of the entries and the sessions.
//! `--kind message` searches the messages in `scribe.db` instead, ranked by
//! FTS5's `bm25()` (§15.7).

use super::{DateArgs, ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{HitPayload, Output, Records};
use clap::Args;
use scribe_core::ask::{Corpus, RecordKind};
use scribe_core::search::{self, DEFAULT_LIMIT, SearchError, SearchQuery};
use scribe_core::{EntryFilter, Project, SessionFilter, Topic};
use std::num::NonZeroUsize;

pub(super) const SEARCH_EXAMPLES: &str = "\
Ranks each entry, each session header and each segment of a session's log
with BM25, as question ask does, and prints the best first; the user's own
entries weigh 1.5 times. The CITE column is what question ask would cite:
pass it to entry show or session show. Superseded and retired entries and
inactive sessions are left out unless --include-hidden.

--kind message searches the user's stored turns of switched-on sessions
instead (and the assistant turns a claim cites), ranked by SQLite FTS5's
BM25; a message cites its session.

Examples:
  scribe search \"merge queue\"
  scribe search release --kind session --since 2026-11-01
  scribe search \"merge queue\" --kind message
  scribe search \"store format\" --project scribe --topic store --limit 5
  scribe search \"api\" --json | jq -r '.hits[].cite'";

/// The record kinds a search can keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum KindArg {
    // Entries only.
    Entry,
    // Session headers and segments only.
    Session,
    // Stored messages only.
    Message,
}

/// Arguments to `search`.
#[derive(Debug, Args)]
pub(crate) struct SearchArgs {
    /// The words to look for; stopwords and words under three letters are ignored
    #[arg(value_name = "TERMS")]
    pub(crate) terms: String,

    /// Only this kind of record [default: entries and sessions]
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

/// `search` reads only. See [`super::Command::audit`].
pub(super) fn audit(_args: &SearchArgs) -> Option<AuditMeta> {
    None
}

/// Run `search`.
pub(super) fn run(args: SearchArgs, ctx: &Context) -> Result<Output, CliError> {
    let limit = args
        .limit
        .or(NonZeroUsize::new(DEFAULT_LIMIT))
        .unwrap_or(NonZeroUsize::MIN);
    let query = SearchQuery {
        terms: args.terms,
        kind: args.kind.map(|k| match k {
            KindArg::Entry => RecordKind::Entry,
            KindArg::Session => RecordKind::Session,
            KindArg::Message => RecordKind::Message,
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
    let mut read_only_copy = false;
    let found = if query.kind == Some(RecordKind::Message) {
        let stored = store.search_messages(&scribe_core::ask::tokens(&query.terms))?;
        read_only_copy = stored.read_only_copy;
        search::messages(&query, &stored, &sessions.listing.items)
    } else {
        let corpus = Corpus {
            entries: &entries.listing.items,
            sessions: &sessions.listing.items,
            tags: &config.tags,
            user: &config.user,
        };
        search::search(&query, corpus)
    }
    .map_err(|e| match e {
        SearchError::NoTerms(_) => CliError::Usage(e.to_string()),
    })?;
    let mut skipped = entries.skipped;
    skipped.extend(sessions.skipped);
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
        "no records in {}; entries come from #scribe lines, and sessions from /scribe on",
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
    if read_only_copy && let Output::List { notices, .. } = &mut output {
        notices.push(format!(
            "read-only copy: {} has no -wal or -shm file and its directory is read-only, so it was opened immutable; messages a live writer has not checkpointed are not shown",
            store.root().join(scribe_core::DB_FILE).display()
        ));
    }
    Ok(output)
}
