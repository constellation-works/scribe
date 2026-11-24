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
use scribe_core::search::{self, DEFAULT_LIMIT, Found, SearchError, SearchQuery};
use scribe_core::{EntryFilter, Project, SessionFilter, TagName, Topic};
use std::num::NonZeroUsize;

pub(super) const SEARCH_EXAMPLES: &str = "\
Ranks each entry, each session header and each segment of a session's log
with BM25, as question ask does, and prints the best first; the user's own
entries weigh 1.5 times. The CITE column is what question ask would cite:
pass it to entry show or session show. Superseded and retired entries and
inactive sessions are left out unless --include-hidden.

--kind message searches the user's stored turns from switched-on sessions
and group members, plus assistant turns a claim cites. SQLite FTS5 ranks
the turns and each cites its session.

Examples:
  scribe search \"merge queue\"
  scribe search release --kind session --since 2026-11-01
  scribe search \"merge queue\" --kind message
  scribe search \"merge queue\" --tag decisions
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

    /// Only entries filed under this tag, by its name as `tag list` prints it; sessions and messages carry no tag [default: any]
    #[arg(long, value_name = "TAG")]
    pub(crate) tag: Option<TagName>,

    /// Only records of this project (a repository slug, or global) [default: any]
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: Option<Project>,

    /// Only records of member sessions in this group (id, unique prefix, or project group slug)
    #[arg(long, value_name = "ID")]
    pub(crate) group: Option<String>,

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
    // A tag names an entry, so a session or message kind can never match it.
    if args.tag.is_some() && matches!(args.kind, Some(KindArg::Session | KindArg::Message)) {
        return Err(CliError::Usage(String::from(
            "--tag filters entries only: sessions and messages carry no tag; drop --kind session or --kind message",
        )));
    }
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
        tag: args.tag,
        project: args.project,
        topics: args.topic,
        dates: args.dates.range()?,
        include_hidden: args.include_hidden,
        limit,
    };
    let store = ctx.store();
    let group = args
        .group
        .as_deref()
        .map(|id| store.group(id))
        .transpose()?;
    let members = group.as_ref().map(|group| group.members());
    let config = store.config()?;
    let entries = store.list_entries_scoped(&EntryFilter::default(), members.as_ref())?;
    let sessions = store.list_sessions_scoped(
        &SessionFilter {
            include_inactive: true,
            ..SessionFilter::default()
        },
        members.as_ref(),
    )?;
    // The stored messages `query` keeps, and whether the store was opened as
    // a read-only copy. Messages are ranked by the store, not by `search`.
    let stored_messages = |query: &SearchQuery| -> Result<(Found, bool), CliError> {
        let tokens = scribe_core::ask::tokens(&query.terms);
        let stored = match &group {
            Some(group) => store.search_group_messages(&group.id, &tokens)?,
            None => store.search_messages(&tokens)?,
        };
        let found = search::messages(query, &stored, &sessions.listing.items).map_err(usage)?;
        Ok((found, stored.read_only_copy))
    };
    let mut read_only_copy = false;
    let found = if query.kind == Some(RecordKind::Message) {
        let (found, copy) = stored_messages(&query)?;
        read_only_copy = copy;
        found
    } else {
        let corpus = Corpus {
            entries: &entries.listing.items,
            sessions: &sessions.listing.items,
            messages: &[],
            tags: &config.tags,
            user: &config.user,
        };
        search::search(&query, corpus).map_err(usage)?
    };
    // A default search that finds no entry or session says when stored
    // messages match, since a line the user typed is only ever a message.
    // The count uses the same filters, so the command lists those messages.
    // A tag leaves messages out too: they carry none, so they never stand in.
    let mut message_matches = None;
    if query.kind.is_none() && query.tag.is_none() && found.total == 0 {
        let message_query = SearchQuery {
            kind: Some(RecordKind::Message),
            ..query.clone()
        };
        let (stored, copy) = stored_messages(&message_query)?;
        if stored.total > 0 {
            message_matches = Some(stored.total);
            read_only_copy = copy;
        }
    }
    let mut skipped = entries.skipped;
    skipped.extend(sessions.skipped);
    let mut criteria = vec![format!("{:?}", query.terms)];
    if let Some(kind) = query.kind {
        criteria.push(format!("--kind {}", kind.as_str()));
    }
    if let Some(tag) = &query.tag {
        criteria.push(format!("--tag {tag}"));
    }
    if let Some(project) = &query.project {
        criteria.push(format!("--project {project}"));
    }
    if let Some(group) = &group {
        criteria.push(format!("--group {}", group.id));
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
    // Last, so it ends the output: the stderr line and the JSON count.
    if let Some(count) = message_matches
        && let Output::List {
            notices,
            message_matches: shown,
            ..
        } = &mut output
    {
        notices.push(format!(
            "{count} stored message(s) match, and no entry or session does: {} lists them",
            message_command(&query, args.group.as_deref(), &args.dates)
        ));
        *shown = Some(count);
    }
    Ok(output)
}

/// A refused query as a usage error (exit 2).
fn usage(e: SearchError) -> CliError {
    match e {
        SearchError::NoTerms(_) => CliError::Usage(e.to_string()),
    }
}

/// The `scribe search` command that lists the messages a hint counts: the
/// same terms and filters, with `--kind message` (the order the criteria
/// notice uses).
fn message_command(query: &SearchQuery, group: Option<&str>, dates: &DateArgs) -> String {
    let mut words = vec![
        String::from("scribe search"),
        shell_word(&query.terms),
        String::from("--kind message"),
    ];
    if let Some(project) = &query.project {
        words.push(format!("--project {project}"));
    }
    if let Some(group) = group {
        words.push(format!("--group {}", shell_word(group)));
    }
    words.extend(query.topics.iter().map(|topic| format!("--topic {topic}")));
    words.extend(dates.criteria());
    if query.include_hidden {
        words.push(String::from("--include-hidden"));
    }
    words.join(" ")
}

/// `text` as one shell word: bare when a shell reads it as itself, else
/// single-quoted.
fn shell_word(text: &str) -> String {
    let bare = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=,@%+".contains(c));
    if bare {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}
