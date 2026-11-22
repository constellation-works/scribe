//! `scribe search` over fixture records (2_design.md §6.8): `ask`'s units,
//! weights and filters, no model and no tasks.

use super::{T0, entry, segment, session};
use crate::ask::{Corpus, RecordKind};
use crate::entry::{Entry, EntryStatus};
use crate::record::{Author, DateRange};
use crate::search::{self, SNIPPET_CHARS, SearchError, SearchQuery};
use crate::session::{SegmentState, SessionRecord};
use crate::tag::TagTable;
use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use time::Duration;

fn query(terms: &str) -> SearchQuery {
    SearchQuery {
        terms: terms.to_owned(),
        kind: None,
        project: None,
        topics: Vec::new(),
        dates: DateRange::default(),
        include_hidden: false,
        limit: NonZeroUsize::new(20).unwrap(),
    }
}

fn noted(id: &str, content: &str) -> Entry {
    let mut e = entry(id, T0);
    content.clone_into(&mut e.content);
    e
}

fn run(q: &SearchQuery, entries: &[Entry], sessions: &[SessionRecord]) -> search::Found {
    let tags = TagTable::defaults();
    search::search(
        q,
        Corpus {
            entries,
            sessions,
            tags: &tags,
            user: &crate::voice::UserName::default(),
        },
    )
    .unwrap()
}

fn cites(found: &search::Found) -> Vec<String> {
    found.hits.iter().map(|h| h.cite.clone()).collect()
}

#[test]
fn daniel_entries_weigh_half_again_as_much_as_agent_notes() {
    let daniel = noted("preferences-1", "squash merge agent pull requests");
    let mut agent = noted("preferences-2", "squash merge agent pull requests");
    agent.author = Author::agent("opus".parse().unwrap());
    let found = run(&query("squash merge"), &[daniel, agent], &[]);
    assert_eq!(cites(&found), ["preferences-1", "preferences-2"]);
    let ratio = found.hits[0].score / found.hits[1].score;
    assert!((ratio - 1.5).abs() < 0.01, "{ratio}");
    assert_eq!(found.hits[1].author.as_ref().unwrap().name.as_str(), "opus");
}

#[test]
fn sessions_rank_as_a_header_and_one_unit_per_segment() {
    let mut r = session("s-1", T0);
    r.title = String::from("release planning");
    r.log = vec![
        segment(1, SegmentState::Sealed, T0, "nothing relevant here"),
        segment(
            2,
            SegmentState::Open,
            T0 + Duration::hours(2),
            "the release train leaves friday",
        ),
    ];
    let found = run(&query("release train"), &[], &[r]);
    assert_eq!(cites(&found), ["session:s-1#2", "session:s-1"]);
    assert_eq!(found.hits[0].segment, Some(2));
    assert_eq!(found.hits[0].title, "part 2");
    assert_eq!(found.hits[1].title, "release planning");
    assert!(found.hits.iter().all(|h| h.kind == RecordKind::Session));
}

#[test]
fn hidden_records_are_counted_and_kept_only_on_request_and_forgotten_never() {
    let mut superseded = noted("preferences-1", "deploy on tuesdays");
    superseded.status = EntryStatus::Superseded;
    let mut forgotten = noted("preferences-2", "deploy on fridays");
    forgotten.status = EntryStatus::Forgotten;
    let mut inactive = session("s-1", T0);
    inactive.active = false;
    inactive.summary = String::from("deploy cadence");
    let entries = [superseded, forgotten];
    let sessions = [inactive];
    let found = run(&query("deploy"), &entries, &sessions);
    assert!(found.hits.is_empty());
    assert_eq!(found.hidden_matches, 2);
    let mut all = query("deploy");
    all.include_hidden = true;
    let found = run(&all, &entries, &sessions);
    assert_eq!(cites(&found), ["preferences-1", "session:s-1"]);
    assert_eq!(found.hits[0].status, "superseded");
    assert_eq!(found.hits[0].active, None);
    assert_eq!(found.hits[1].active, Some(false));
    assert_eq!(found.hidden_matches, 0);
}

#[test]
fn filters_narrow_by_kind_project_topics_and_dates() {
    let mut e = noted("preferences-1", "pin the toolchain");
    e.topics = vec!["ci".parse().unwrap()].try_into().unwrap();
    let mut r = session("s-1", T0 + Duration::days(10));
    r.summary = String::from("toolchain upgrade");
    r.log[0].text = String::from("toolchain bumped");
    r.log[0].topics = vec!["ci".parse().unwrap()].try_into().unwrap();
    let entries = [e];
    let sessions = [r];
    let with = |f: &dyn Fn(&mut SearchQuery)| {
        let mut q = query("toolchain");
        f(&mut q);
        cites(&run(&q, &entries, &sessions))
    };
    assert_eq!(with(&|_| {}).len(), 3);
    assert_eq!(
        with(&|q| q.kind = Some(RecordKind::Entry)),
        ["preferences-1"]
    );
    assert_eq!(with(&|q| q.kind = Some(RecordKind::Session)).len(), 2);
    assert!(with(&|q| q.project = Some("orbit".parse().unwrap())).is_empty());
    // A segment carries its own topics as well as its session's.
    let mut ci = with(&|q| q.topics = vec!["ci".parse().unwrap()]);
    ci.sort();
    assert_eq!(ci, ["preferences-1", "session:s-1#1"]);
    let later = with(&|q| q.dates.since = Some((T0 + Duration::days(5)).date()));
    assert_eq!(later.len(), 2);
    assert!(!later.contains(&String::from("preferences-1")));
    let earlier = with(&|q| q.dates.until = Some((T0 + Duration::days(5)).date()));
    assert_eq!(earlier, ["preferences-1"]);
}

#[test]
fn the_limit_cuts_after_counting_every_hit() {
    let entries: Vec<Entry> = (1..=5)
        .map(|n| noted(&format!("preferences-{n}"), "rotate the keys"))
        .collect();
    let mut q = query("rotate keys");
    q.limit = NonZeroUsize::new(2).unwrap();
    let found = run(&q, &entries, &[]);
    assert_eq!(found.hits.len(), 2);
    assert_eq!(found.total, 5);
}

#[test]
fn terms_with_no_searchable_word_are_refused() {
    let tags = TagTable::defaults();
    let err = search::search(
        &query("the of it"),
        Corpus {
            entries: &[],
            sessions: &[],
            tags: &tags,
            user: &crate::voice::UserName::default(),
        },
    )
    .unwrap_err();
    assert!(matches!(err, SearchError::NoTerms(_)));
}

#[test]
fn a_snippet_is_bounded_and_starts_near_the_first_match() {
    let wanted: BTreeSet<String> = crate::ask::tokens("needle").into_iter().collect();
    let long = format!("{} needle {}", "word ".repeat(200), "tail ".repeat(200));
    let snip = search::snippet(&long, &wanted);
    assert!(
        snip.chars().count() <= SNIPPET_CHARS,
        "{}",
        snip.chars().count()
    );
    assert!(snip.starts_with('…') && snip.ends_with('…'));
    assert!(snip.contains("needle"));
    assert_eq!(
        search::snippet("short needle text", &wanted),
        "short needle text"
    );
    let at_end = format!("{} needle", "word ".repeat(200));
    let snip = search::snippet(&at_end, &wanted);
    assert!(snip.ends_with("needle") && snip.chars().count() <= SNIPPET_CHARS);
}

#[test]
fn approved_assistant_context_entry_snippet_labels_statement_and_assistant_words() {
    let context =
        "The user and the agent investigated drain stalls. The agent reported a token collision.";
    let quote = "The root cause is that parallel drains share a single auth token.";
    let mut e = noted("notes-1", context);
    e.author = Author::assistant();
    e.said = String::from("#scribe approve p-1");
    e.content_from = crate::entry::ContentFrom::Message {
        role: crate::entry::Speaker::Assistant,
        message: String::from("a-1"),
        at: T0,
    };
    e.quote = Some(quote.to_owned());
    e.stated_by = Some(Author::session_pass());

    // A query matching only the assistant's quote still ranks the entry
    // and renders the statement + assistant's words snippet.
    let found = run(&query("single auth token"), &[e], &[]);
    assert_eq!(cites(&found), ["notes-1"]);
    let snip = &found.hits[0].snippet;
    assert_eq!(
        snip,
        &format!(
            "scribe's statement of the assistant's turn: {context} | The assistant's words: \"{quote}\""
        )
    );
}
