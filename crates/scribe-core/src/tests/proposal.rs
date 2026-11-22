use crate::entry::{Cite, Speaker};
use crate::proposal::{
    ClosedReason, EntryCandidate, Proposal, ProposalId, ProposalKind, ProposalState,
    close_retired_tag_candidates,
};
use crate::record::{Author, AuthorKind, Label, Project};
use crate::tag::TagTable;
use time::macros::datetime;

#[test]
fn proposal_ids_are_p_dash_number() {
    let id: ProposalId = "p-4".parse().unwrap();
    assert_eq!(id.number(), 4);
    assert_eq!(id.to_string(), "p-4");
    for bad in ["p-0", "p-04", "p4", "q-4", "p-", "p--4"] {
        assert!(bad.parse::<ProposalId>().is_err(), "{bad:?}");
    }
}

#[test]
fn proposals_sort_by_number_not_by_text() {
    let mut ids: Vec<ProposalId> = ["p-10", "p-2", "p-1"]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
    ids.sort();
    let text: Vec<String> = ids.iter().map(ToString::to_string).collect();
    assert_eq!(text, ["p-1", "p-2", "p-10"]);
}

#[test]
fn close_retired_tag_candidates_closes_only_open_candidates_outside_the_tag_table() {
    let t0 = datetime!(2026-11-15 00:00 UTC);
    let t1 = datetime!(2026-11-21 06:00 UTC);
    let candidate = |n: u64, tag: &str, state: ProposalState| Proposal {
        id: format!("p-{n}").parse().unwrap(),
        kind: ProposalKind::Candidate,
        older: format!("{tag}-{n}").parse().unwrap(),
        newer: None,
        candidate: Some(EntryCandidate {
            tag: tag.parse().unwrap(),
            role: Speaker::User,
            quote: format!("quote {n}"),
            statement: None,
            subject: None,
            evidence: None,
            cite: Cite {
                session: "s-1".parse().unwrap(),
                message: Some(format!("m-{n}")),
                at: t0,
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
        opened_at: t0,
        settled_at: (state != ProposalState::Open).then_some(t0),
        settled_by: None,
        author: Author::session_pass(),
        updated_by: None,
        settled_author: (state != ProposalState::Open).then(Author::user),
        closed_reason: None,
        instruction: None,
    };
    let stale = Proposal {
        id: "p-5".parse().unwrap(),
        kind: ProposalKind::Stale,
        older: "decisions-1".parse().unwrap(),
        newer: None,
        candidate: None,
        reason: String::from("stale"),
        quotes: Vec::new(),
        state: ProposalState::Open,
        opened_at: t0,
        settled_at: None,
        settled_by: None,
        author: Author::consistency(),
        updated_by: None,
        settled_author: None,
        closed_reason: None,
        instruction: None,
    };
    let mut proposals = vec![
        candidate(1, "decisions", ProposalState::Open),
        candidate(2, "preferences", ProposalState::Open),
        candidate(3, "idea", ProposalState::Open),
        candidate(4, "task", ProposalState::Approved),
        stale,
    ];
    let tags = TagTable::defaults();
    let closed = close_retired_tag_candidates(&mut proposals, &tags, t1);
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].id.to_string(), "p-1");
    assert_eq!(proposals[0].state, ProposalState::Rejected);
    assert_eq!(proposals[0].settled_at, Some(t1));
    assert_eq!(proposals[0].settled_by, None);
    assert_eq!(
        proposals[0].settled_author,
        Some(Author {
            kind: AuthorKind::Scribe,
            name: "tag-retired".parse().unwrap(),
        })
    );
    assert_eq!(proposals[0].closed_reason, Some(ClosedReason::TagRetired));
    assert_eq!(proposals[1].state, ProposalState::Open);
    assert_eq!(proposals[2].state, ProposalState::Open);
    assert_eq!(proposals[3].state, ProposalState::Approved);
    assert_eq!(proposals[3].closed_reason, None);
    assert_eq!(proposals[4].state, ProposalState::Open);
    assert!(close_retired_tag_candidates(&mut proposals, &tags, t1).is_empty());
}

#[test]
fn closed_reason_instructed_round_trips() {
    assert_eq!(ClosedReason::TagRetired.as_str(), "tag_retired");
    assert_eq!(ClosedReason::Instructed.as_str(), "instructed");
    let json = serde_json::to_string(&ClosedReason::Instructed).unwrap();
    assert_eq!(json, "\"instructed\"");
    let back: ClosedReason = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ClosedReason::Instructed);
}
