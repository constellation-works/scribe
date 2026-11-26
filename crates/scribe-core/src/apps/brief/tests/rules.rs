//! The session brief: which sessions and claims it picks, and its text.

use super::{T0, entry, session};
use crate::brief::{self, Brief};
use crate::entry::{Cite, Entry, EntryStatus, Speaker};
use crate::proposal::{EntryCandidate, Proposal, ProposalKind, ProposalState, Quote};
use crate::record::{Author, Label, Project};
use time::Duration;

fn candidate(n: u64, tag: &str, state: ProposalState, statement: Option<&str>) -> Proposal {
    Proposal {
        id: format!("p-{n}").parse().unwrap(),
        kind: ProposalKind::Candidate,
        older: format!("{tag}-{n}").parse().unwrap(),
        newer: None,
        candidate: Some(EntryCandidate {
            tag: tag.parse().unwrap(),
            role: Speaker::User,
            quote: format!("quote {n}"),
            context: None,
            statement: statement.map(str::to_owned),
            subject: statement.map(|_| format!("subject {n}")),
            evidence: None,
            cite: Cite {
                session: "s-1".parse().unwrap(),
                message: Some(format!("m-{n}")),
                at: T0,
                prompt_sha256: "0".repeat(64),
                line: 1,
                cwd: String::from("/work"),
            },
            project: Project::sanitized("scribe"),
            provider: Label::sanitized("claude-code", "claude-code"),
            host: Label::sanitized("host", "host"),
            admitted: None,
        }),
        reason: format!("candidate {n}"),
        quotes: Vec::new(),
        state,
        opened_at: T0,
        settled_at: (state != ProposalState::Open).then_some(T0),
        settled_by: None,
        author: Author::session_pass(),
        updated_by: None,
        settled_author: (state != ProposalState::Open).then(Author::user),
        closed_reason: None,
        instruction: None,
    }
}

fn supersedes(n: u64, older: &str, newer: &str) -> Proposal {
    Proposal {
        id: format!("p-{n}").parse().unwrap(),
        kind: ProposalKind::Supersedes,
        older: older.parse().unwrap(),
        newer: Some(newer.parse().unwrap()),
        candidate: None,
        reason: String::from("the newer entry replaces the older rule"),
        quotes: vec![Quote {
            entry: newer.parse().unwrap(),
            text: String::from("squash-merge agent PRs"),
        }],
        state: ProposalState::Open,
        opened_at: T0,
        settled_at: None,
        settled_by: None,
        author: Author::consistency(),
        updated_by: None,
        settled_author: None,
        closed_reason: None,
        instruction: None,
    }
}

#[test]
fn an_empty_store_has_both_sections_and_says_they_are_empty() {
    let brief = brief::build(&[], &[], &[], Some(&Project::sanitized("scribe")), None, 5);
    assert_eq!(brief.claim_count(), 0);
    assert_eq!(
        brief.text(),
        "# scribe brief\n\n## Open items\n(none)\n\n## Recent sessions\n(none)\n\n## Open claims\n(none)\n"
    );
}

#[test]
fn sessions_put_the_projects_first_newest_first_without_the_excluded_one() {
    let old = session("s-old", T0);
    let newest = session("s-new", T0 + Duration::days(2));
    let middle = session("s-mid", T0 + Duration::days(1));
    let mut inactive = session("s-off", T0 + Duration::days(3));
    inactive.active = false;
    let mut other = session("s-other", T0 + Duration::days(4));
    other.project = Project::sanitized("orbit");
    let current = session("s-now", T0 + Duration::days(5));
    let records = [old, newest, middle, inactive, other, current];
    let project = Project::sanitized("scribe");
    let ids = |b: &Brief| -> Vec<String> { b.sessions.iter().map(|s| s.id.to_string()).collect() };
    let now: crate::session::SessionId = "s-now".parse().unwrap();

    let all = brief::build(&records, &[], &[], Some(&project), Some(&now), 5);
    assert_eq!(ids(&all), ["s-new", "s-mid", "s-old", "s-other"]);
    let capped = brief::build(&records, &[], &[], Some(&project), Some(&now), 2);
    assert_eq!(ids(&capped), ["s-new", "s-mid"]);
    let anywhere = brief::build(&records, &[], &[], None, None, 20);
    assert_eq!(
        ids(&anywhere),
        ["s-now", "s-other", "s-new", "s-mid", "s-old"]
    );
}

#[test]
fn claims_are_the_open_proposals_grouped_by_tag_with_their_ids() {
    let proposals = [
        candidate(
            3,
            "preferences",
            ProposalState::Open,
            Some("Prefer small PRs."),
        ),
        candidate(1, "preferences", ProposalState::Open, None),
        candidate(2, "decisions", ProposalState::Approved, Some("settled")),
        supersedes(4, "decisions-1", "decisions-7"),
    ];
    let brief = brief::build(&[], &[], &proposals, None, None, 5);
    let groups: Vec<(String, Vec<String>)> = brief
        .claims
        .iter()
        .map(|g| {
            (
                g.tag.to_string(),
                g.claims.iter().map(|c| c.id.to_string()).collect(),
            )
        })
        .collect();
    assert_eq!(
        groups,
        [
            (String::from("decisions"), vec![String::from("p-4")]),
            (
                String::from("preferences"),
                vec![String::from("p-1"), String::from("p-3")]
            ),
        ]
    );
    // A candidate with no statement is stated by its quote; a consistency
    // proposal quotes its first span, else its reason.
    assert_eq!(brief.claims[1].claims[0].statement, "quote 1");
    assert_eq!(brief.claims[0].claims[0].quote, "squash-merge agent PRs");
    let mut long = candidate(6, "idea", ProposalState::Open, None);
    if let Some(c) = long.candidate.as_mut() {
        c.quote = "word ".repeat(60);
    }
    let brief = brief::build(&[], &[], &[long], None, None, 5);
    let quote = &brief.claims[0].claims[0].quote;
    assert_eq!(quote.chars().count(), brief::QUOTE_CHARS);
    assert!(quote.ends_with('…'), "{quote}");
    let mut bare = supersedes(5, "decisions-1", "decisions-7");
    bare.quotes.clear();
    let brief = brief::build(&[], &[], &[bare], None, None, 5);
    assert_eq!(
        brief.claims[0].claims[0].quote,
        "the newer entry replaces the older rule"
    );
}

#[test]
fn the_text_puts_the_memory_block_first_then_dated_sessions_and_claims() {
    let mut record = session("s-1", T0);
    record.title = String::from("Ship the brief");
    let proposals = [
        candidate(
            1,
            "preferences",
            ProposalState::Open,
            Some("Prefer small PRs."),
        ),
        supersedes(2, "decisions-1", "decisions-7"),
    ];
    let project = Project::sanitized("scribe");
    let mut brief: Brief = brief::build(&[record], &[], &proposals, Some(&project), None, 5);
    brief.memory = Some(String::from(
        "scribe memory — page\nNow:\n- an item [m-1]\n",
    ));
    assert_eq!(
        brief.text(),
        "# scribe brief\n\
         \n\
         scribe memory — page\n\
         Now:\n\
         - an item [m-1]\n\
         \n\
         ## Open items\n\
         (none)\n\
         \n\
         ## Recent sessions\n\
         - s-1 · 2026-11-01 11:00Z · scribe · Ship the brief\n\
         \n\
         ## Open claims\n\
         - p-2 · 2026-11-01 10:00Z · decisions · \"squash-merge agent PRs\"\n\
         - p-1 · 2026-11-01 10:00Z · preferences · \"quote 1\"\n\
         Settle a claim by typing #scribe approve <id> or #scribe reject <id>; scribe proposal show <id> shows one in full.\n"
    );
}

fn handoff(id: &str, session_id: &str, created: time::OffsetDateTime) -> Entry {
    let mut e = entry(id, created);
    e.author = Author::agent("opus".parse().unwrap());
    e.related = vec![format!("session:{session_id}").parse().unwrap()];
    e.cite = None;
    e
}

#[test]
fn brief_marks_recorded_sessions_with_newest_handoff_and_lists_unrecorded_ones() {
    let mut recorded = session("s-rec", T0);
    recorded.title = String::from("Recorded work");
    let mut inactive = session("s-inactive", T0 + Duration::hours(5));
    inactive.active = false;
    let h_old = handoff("handoff-1", "s-rec", T0 + Duration::minutes(30));
    let h_new = handoff("handoff-2", "s-rec", T0 + Duration::hours(2));
    let mut h_retired = handoff("handoff-3", "s-rec", T0 + Duration::hours(3));
    h_retired.status = EntryStatus::Retired;
    let h_unrecorded = handoff("handoff-4", "s-off", T0 + Duration::hours(4));
    let h_inactive = handoff("handoff-5", "s-inactive", T0 + Duration::hours(6));
    let project = Project::sanitized("scribe");

    let brief = brief::build(
        &[recorded, inactive],
        &[h_old, h_new, h_retired, h_unrecorded, h_inactive],
        &[],
        Some(&project),
        None,
        5,
    );
    let ids: Vec<&str> = brief.sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["s-off", "s-rec"]);
    assert!(!brief.sessions[0].has_record);
    assert_eq!(
        brief.sessions[0].handoff.as_ref().map(ToString::to_string),
        Some(String::from("handoff-4"))
    );
    assert_eq!(brief.sessions[0].handoffs, 1);
    assert!(brief.sessions[1].has_record);
    assert_eq!(
        brief.sessions[1].handoff.as_ref().map(ToString::to_string),
        Some(String::from("handoff-2"))
    );
    assert_eq!(brief.sessions[1].handoffs, 2);
    assert!(
        brief.text().contains(
            "- s-off · 2026-11-01 14:00Z · scribe · no session record [handoff: handoff-4]"
        )
    );
    assert!(
        brief
            .text()
            .contains("- s-rec · 2026-11-01 12:00Z · scribe · Recorded work [handoff: handoff-2]")
    );
}

#[test]
fn open_items_with_session_answer_in_use_short_ids() {
    use crate::ShortIds;
    use crate::entry::{AnswerIn, OpenItem};
    let session_full = "8dc9df2b-5808-47cf-8be4-190c9c6e0711";
    let mut item = entry("open-item-1", T0);
    item.item = Some(OpenItem {
        asked_by: "captain".parse().unwrap(),
        options: vec![],
        recommended: None,
        why: None,
        blocked: "nothing".into(),
        answer_in: AnswerIn::Session(session_full.parse().unwrap()),
        answer: None,
        closed: None,
    });
    let mut page = brief::build(&[], &[], &[], None, None, 5);
    page.open_items = brief::open_items(std::slice::from_ref(&item));
    assert_eq!(
        page.open_items[0].answer_in,
        format!("session:{session_full}")
    );

    // A cataloged session prints as its `s-<n>` alias.
    let mut ids = ShortIds::new([session_full.to_owned()]);
    ids.set_aliases([(session_full.to_owned(), String::from("s-3"))]);
    assert!(
        page.text_with_ids(&ids)
            .contains("- open-item-1 · open · captain · - · session:s-3 · something\n"),
        "{}",
        page.text_with_ids(&ids)
    );

    // An uncataloged session prints as its shortest unique prefix.
    let other = "8dc9df2c-5808-47cf-8be4-190c9c6e0711";
    let prefixed = ShortIds::new([session_full.to_owned(), other.to_owned()]);
    let line = page
        .text_with_ids(&prefixed)
        .lines()
        .find(|line| line.starts_with("- open-item-1 "))
        .unwrap()
        .to_owned();
    assert!(line.contains("session:8dc9df2b · "), "{line}");
    assert!(!line.contains(session_full), "{line}");
}

#[test]
fn open_items_are_ordered_capped_and_include_legacy_records() {
    use crate::entry::{AnswerIn, ItemAnswer, OpenItem};
    let mut answered = entry("open-item-1", T0);
    answered.item = Some(OpenItem {
        asked_by: "captain".parse().unwrap(),
        options: vec![],
        recommended: None,
        why: None,
        blocked: "nothing".into(),
        answer_in: AnswerIn::Hand,
        answer: Some(ItemAnswer {
            words: "yes".into(),
            session: "s-1".parse().unwrap(),
            at: T0,
            recorded_by: "captain".parse().unwrap(),
        }),
        closed: None,
    });
    let mut records: Vec<_> = (2..=12)
        .map(|n| entry(&format!("open-item-{n}"), T0 + Duration::days(n)))
        .collect();
    records.push(answered);
    let mut retired = entry("open-item-13", T0);
    retired.status = EntryStatus::Retired;
    records.push(retired);
    let mut page = brief::build(&[], &[], &[], None, None, 5);
    page.open_items = brief::open_items(&records);
    assert_eq!(page.open_items.len(), 12);
    assert_eq!(page.open_items[0].id, "open-item-2");
    assert_eq!(page.open_items[11].state, "answered");
    assert!(
        page.text()
            .contains("(+2 more: scribe entry list --tag open-item --state open --state answered)")
    );
}
