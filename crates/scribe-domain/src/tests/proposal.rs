use crate::proposal::{ClosedReason, ProposalId};

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
fn closed_reason_instructed_round_trips() {
    assert_eq!(ClosedReason::TagRetired.as_str(), "tag_retired");
    assert_eq!(ClosedReason::Instructed.as_str(), "instructed");
    let json = serde_json::to_string(&ClosedReason::Instructed).unwrap();
    assert_eq!(json, "\"instructed\"");
    let back: ClosedReason = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ClosedReason::Instructed);
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
