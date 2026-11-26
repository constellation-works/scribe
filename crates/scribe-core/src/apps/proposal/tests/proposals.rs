use crate::consistency::Draft;
use crate::proposal::{CandidateEntry, EntryCandidate, ProposalKind, ProposalState};
use crate::{Cite, Label, Speaker};
use time::macros::datetime;

#[test]
fn only_an_approved_candidate_can_materialize_an_entry() {
    let now = datetime!(2026-11-26 02:25 UTC);
    let mut proposal = Draft {
        kind: ProposalKind::Candidate,
        older: "preferences-1".parse().unwrap(),
        newer: None,
        reason: String::from("a preference"),
        quotes: Vec::new(),
    }
    .into_proposal("p-1".parse().unwrap(), now);
    proposal.settled_at = Some(now);
    proposal.candidate = Some(EntryCandidate {
        tag: "preferences".parse().unwrap(),
        role: Speaker::User,
        quote: String::from("Keep updates short."),
        context: None,
        statement: None,
        subject: None,
        evidence: None,
        cite: Cite {
            session: "s-1".parse().unwrap(),
            message: Some(String::from("m-1")),
            at: now,
            prompt_sha256: "1".repeat(64),
            line: 1,
            cwd: String::from("/work"),
        },
        project: "scribe".parse().unwrap(),
        provider: Label::sanitized("claude-code", "claude-code"),
        host: Label::sanitized("host", "host"),
        admitted: None,
    });

    assert!(proposal.candidate_entry(None).is_none());
    proposal.state = ProposalState::Rejected;
    assert!(proposal.candidate_entry(None).is_none());

    proposal.state = ProposalState::Approved;
    let entry = proposal.candidate_entry(None).unwrap();
    assert_eq!(entry.id, proposal.older);
    assert_eq!(entry.said, "#scribe approve p-1");

    proposal.kind = ProposalKind::Stale;
    assert!(proposal.candidate_entry(None).is_none());
}

#[test]
fn a_context_stands_alone_from_two_sentences_up_to_600_characters() {
    assert!(crate::context_stands_alone(
        "The user and the agent chose a store. The user asked for a log."
    ));
    assert!(crate::context_stands_alone(
        "They weighed it (twice). The user said \"keep the log.\""
    ));
    for line in [
        "",
        "The user prefers a log.",
        "The user prefers a log, e.g. one per session",
        "v1.2 is out and the user prefers it",
    ] {
        assert!(!crate::context_stands_alone(line), "{line}");
    }
    let long = format!("{}. {}.", "a".repeat(299), "b".repeat(299));
    assert_eq!(long.chars().count(), 601);
    assert!(!crate::context_stands_alone(&long));
}
