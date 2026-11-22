//! The session brief: which sessions and claims it picks, and its text.

use super::{T0, session};
use crate::brief::{self, Brief};
use crate::entry::{Cite, Speaker};
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
        settled_author: (state != ProposalState::Open).then(Author::daniel),
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
    let brief = brief::build(&[], &[], Some(&Project::sanitized("scribe")), None, 5);
    assert_eq!(brief.claim_count(), 0);
    assert_eq!(
        brief.text(),
        "# scribe brief\n\n## Recent sessions\n(none)\n\n## Open claims\n(none)\n"
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

    let all = brief::build(&records, &[], Some(&project), Some(&now), 5);
    assert_eq!(ids(&all), ["s-new", "s-mid", "s-old", "s-other"]);
    let capped = brief::build(&records, &[], Some(&project), Some(&now), 2);
    assert_eq!(ids(&capped), ["s-new", "s-mid"]);
    let anywhere = brief::build(&records, &[], None, None, 20);
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
    let brief = brief::build(&[], &proposals, None, None, 5);
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
    let brief = brief::build(&[], &[long], None, None, 5);
    let quote = &brief.claims[0].claims[0].quote;
    assert_eq!(quote.chars().count(), brief::QUOTE_CHARS);
    assert!(quote.ends_with('…'), "{quote}");
    let mut bare = supersedes(5, "decisions-1", "decisions-7");
    bare.quotes.clear();
    let brief = brief::build(&[], &[bare], None, None, 5);
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
    let mut brief: Brief = brief::build(&[record], &proposals, Some(&project), None, 5);
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
         ## Recent sessions\n\
         - s-1 · 2026-11-01 11:00Z · scribe · Ship the brief\n\
         \n\
         ## Open claims\n\
         - p-2 · 2026-11-01 10:00Z · decisions · \"squash-merge agent PRs\"\n\
         - p-1 · 2026-11-01 10:00Z · preferences · \"quote 1\"\n\
         Settle a claim by typing #scribe approve <id> or #scribe reject <id>; scribe proposal list --state open shows each in full.\n"
    );
}
