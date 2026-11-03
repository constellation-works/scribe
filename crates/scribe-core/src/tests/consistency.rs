use super::entry;
use crate::consistency::{self, Draft, Settle, SettleRefusal, Settled};
use crate::entry::{Cite, Entry, EntryId, EntryStatus};
use crate::models::{ConsistencyAnswer, JudgementAnswer};
use crate::proposal::{Proposal, ProposalId, ProposalKind, ProposalState, Quote};
use time::macros::{date, datetime};
use time::{Duration, OffsetDateTime};

const T0: OffsetDateTime = datetime!(2026-10-01 09:00 UTC);
const NOW: OffsetDateTime = datetime!(2026-11-02 06:00 UTC);

fn id(s: &str) -> EntryId {
    s.parse().unwrap()
}

fn pid(s: &str) -> ProposalId {
    s.parse().unwrap()
}

/// An active entry `id`, recorded `days` after T0, saying `content`.
fn with(id: &str, days: i64, content: &str) -> Entry {
    let mut e = entry(id, T0 + Duration::days(days));
    e.content = content.to_owned();
    e.said = format!("#scribe {}: {content}", e.tag);
    e
}

fn cite() -> Cite {
    Cite {
        session: "s-9".parse().unwrap(),
        message: None,
        at: NOW,
        prompt_sha256: "1".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    }
}

fn proposal(
    n: &str,
    kind: ProposalKind,
    older: &str,
    newer: Option<&str>,
    state: ProposalState,
) -> Proposal {
    Draft {
        kind,
        older: id(older),
        newer: newer.map(id),
        reason: String::from("why"),
        quotes: vec![Quote {
            entry: id(older),
            text: String::from("the older words"),
        }],
    }
    .into_proposal(pid(n), T0)
    .with_state(state)
}

trait WithState {
    fn with_state(self, state: ProposalState) -> Self;
}

impl WithState for Proposal {
    fn with_state(mut self, state: ProposalState) -> Self {
        self.state = state;
        self
    }
}

fn store() -> Vec<Entry> {
    vec![
        with("decisions-1", 0, "squash-merge agent PRs into agent-main"),
        with(
            "decisions-2",
            5,
            "rebase-merge agent PRs into agent-main, never squash",
        ),
        with("decisions-3", 6, "the release train leaves on Thursdays"),
        with("rituals-1", 1, "squash-merge agent PRs into agent-main"),
    ]
}

#[test]
fn candidates_pair_overlapping_entries_of_one_tag_only() {
    let found = consistency::candidates(&store(), &[], date!(2026 - 11 - 02), 40);
    let pairs: Vec<(String, String)> = found
        .pairs
        .iter()
        .map(|p| (p.older.id.to_string(), p.newer.id.to_string()))
        .collect();
    // rituals-1 says the same words as decisions-1 but in another tag;
    // decisions-3 shares no words with the others.
    assert_eq!(
        pairs,
        [(String::from("decisions-1"), String::from("decisions-2"))]
    );
    assert_eq!(found.over_cap, 0);
    assert!(found.stale.is_empty());
}

#[test]
fn candidates_pair_entries_sharing_a_topic_and_cap_newest_first() {
    let mut entries = store();
    let topic: crate::record::Topics = vec!["release".parse().unwrap()].try_into().unwrap();
    entries[1].topics = topic.clone();
    entries[2].topics = topic;
    let found = consistency::candidates(&entries, &[], date!(2026 - 11 - 02), 1);
    assert_eq!(found.pairs.len(), 1);
    assert_eq!(found.pairs[0].older.id, id("decisions-2"));
    assert_eq!(found.pairs[0].newer.id, id("decisions-3"));
    assert_eq!(
        found.over_cap, 1,
        "decisions-1/decisions-2 waits for the next scan"
    );
}

#[test]
fn a_pair_or_stale_entry_with_a_proposal_in_any_state_is_not_offered_again() {
    let mut entries = store();
    entries[2].review_after = Some(date!(2026 - 11 - 01));
    for state in [
        ProposalState::Open,
        ProposalState::Approved,
        ProposalState::Rejected,
    ] {
        let proposals = [
            // Either order of the pair counts as the same subject.
            proposal(
                "p-1",
                ProposalKind::Supersedes,
                "decisions-2",
                Some("decisions-1"),
                state,
            ),
            proposal("p-2", ProposalKind::Stale, "decisions-3", None, state),
        ];
        let found = consistency::candidates(&entries, &proposals, date!(2026 - 11 - 02), 40);
        assert!(found.pairs.is_empty(), "{state:?}");
        assert!(found.stale.is_empty(), "{state:?}");
    }
}

#[test]
fn candidates_find_active_entries_due_for_review_and_skip_hidden_ones() {
    let mut entries = store();
    entries[2].review_after = Some(date!(2026 - 11 - 02));
    entries[3].review_after = Some(date!(2026 - 11 - 03));
    entries[0].review_after = Some(date!(2026 - 10 - 01));
    entries[0].status = EntryStatus::Retired;
    let found = consistency::candidates(&entries, &[], date!(2026 - 11 - 02), 40);
    let stale: Vec<String> = found.stale.iter().map(|e| e.id.to_string()).collect();
    assert_eq!(stale, ["decisions-3"]);
    assert!(found.pairs.is_empty(), "a retired entry pairs with nothing");
}

fn judgement(pair: &str, label: &str, older: &str, newer: &str) -> JudgementAnswer {
    JudgementAnswer {
        pair: pair.to_owned(),
        label: label.to_owned(),
        reason: format!("{label} because"),
        older_quote: older.to_owned(),
        newer_quote: newer.to_owned(),
    }
}

#[test]
fn judge_keeps_a_contradiction_only_with_both_quotes_verbatim() {
    let pairs = consistency::candidates(&store(), &[], date!(2026 - 11 - 02), 40).pairs;
    let request = consistency::request("a-model", &pairs);
    assert_eq!(request.pairs[0].pair, "1");
    assert_eq!(request.pairs[0].older.id, "decisions-1");

    let good = judgement(
        "1",
        "contradicts",
        "squash-merge agent PRs",
        "rebase-merge agent PRs",
    );
    let judged = consistency::judge(
        ConsistencyAnswer {
            judgements: vec![good.clone()],
        },
        &pairs,
    );
    assert_eq!(judged.drafts.len(), 1);
    let draft = &judged.drafts[0];
    assert_eq!(draft.kind, ProposalKind::Supersedes);
    assert_eq!(draft.older, id("decisions-1"));
    assert_eq!(draft.newer, Some(id("decisions-2")));
    assert_eq!(draft.quotes.len(), 2);

    for bad in [
        judgement(
            "1",
            "contradicts",
            "merge every PR by hand",
            "rebase-merge agent PRs",
        ),
        judgement(
            "1",
            "refines",
            "squash-merge agent PRs",
            "rebase only on Fridays",
        ),
        judgement(
            "2",
            "contradicts",
            "squash-merge agent PRs",
            "rebase-merge agent PRs",
        ),
        judgement(
            "1",
            "duplicates",
            "squash-merge agent PRs",
            "rebase-merge agent PRs",
        ),
    ] {
        let judged = consistency::judge(
            ConsistencyAnswer {
                judgements: vec![bad.clone()],
            },
            &pairs,
        );
        assert!(judged.drafts.is_empty(), "{bad:?}");
        assert_eq!(judged.dropped, 1, "{bad:?}");
    }

    // A second label for the same pair is dropped; unrelated is counted.
    let judged = consistency::judge(
        ConsistencyAnswer {
            judgements: vec![judgement("1", "unrelated", "", ""), good],
        },
        &pairs,
    );
    assert_eq!(
        (judged.drafts.len(), judged.unrelated, judged.dropped),
        (0, 1, 1)
    );
}

#[test]
fn a_stale_draft_quotes_the_start_of_the_entry() {
    let mut e = with("decisions-3", 6, "the release train leaves on Thursdays");
    e.review_after = Some(date!(2026 - 11 - 01));
    let draft = consistency::stale(&e);
    assert_eq!(draft.kind, ProposalKind::Stale);
    assert_eq!(draft.newer, None);
    assert_eq!(
        draft.quotes[0].text,
        "the release train leaves on Thursdays"
    );
    assert!(draft.reason.contains("2026-11-01"), "{}", draft.reason);
}

#[test]
fn approve_supersedes_the_older_entry_and_is_idempotent() {
    let mut entries = store();
    let mut proposals = vec![proposal(
        "p-1",
        ProposalKind::Supersedes,
        "decisions-1",
        Some("decisions-2"),
        ProposalState::Open,
    )];
    let done = consistency::settle(
        &Settle::Approve(pid("p-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(
        done,
        Settled::Approved {
            proposal: pid("p-1"),
            entry: id("decisions-1"),
            status: EntryStatus::Superseded,
            by: Some(id("decisions-2")),
        }
    );
    assert_eq!(proposals[0].state, ProposalState::Approved);
    assert_eq!(proposals[0].settled_at, Some(NOW));
    assert_eq!(proposals[0].settled_by, Some(cite()));
    assert_eq!(entries[0].status, EntryStatus::Superseded);
    assert_eq!(entries[0].superseded_by, Some(id("decisions-2")));
    assert_eq!(entries[0].updated, NOW);
    assert_eq!(entries[1].status, EntryStatus::Active);

    let again = consistency::settle(
        &Settle::Approve(pid("p-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert!(matches!(again, Settled::Already(_)), "{again:?}");
    let reject = consistency::settle(
        &Settle::Reject(pid("p-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(reject.refusal_code(), Some("proposal_settled"));
}

#[test]
fn approving_a_stale_proposal_retires_its_entry() {
    let mut entries = store();
    let mut proposals = vec![proposal(
        "p-3",
        ProposalKind::Stale,
        "decisions-3",
        None,
        ProposalState::Open,
    )];
    consistency::settle(
        &Settle::Approve(pid("p-3")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(entries[2].status, EntryStatus::Retired);
    assert_eq!(entries[2].superseded_by, None);
}

#[test]
fn approve_refuses_when_the_older_entry_is_no_longer_active() {
    let mut entries = store();
    entries[0].status = EntryStatus::Retired;
    let mut proposals = vec![proposal(
        "p-1",
        ProposalKind::Supersedes,
        "decisions-1",
        Some("decisions-2"),
        ProposalState::Open,
    )];
    let done = consistency::settle(
        &Settle::Approve(pid("p-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(done.refusal_code(), Some("entry_not_active"));
    assert_eq!(proposals[0].state, ProposalState::Open, "nothing changed");
    let missing = consistency::settle(
        &Settle::Approve(pid("p-9")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(missing.refusal_code(), Some("proposal_not_found"));
}

#[test]
fn reject_closes_the_proposal_and_leaves_the_entries() {
    let mut entries = store();
    let before = entries.clone();
    let mut proposals = vec![proposal(
        "p-1",
        ProposalKind::Supersedes,
        "decisions-1",
        Some("decisions-2"),
        ProposalState::Open,
    )];
    let done = consistency::settle(
        &Settle::Reject(pid("p-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(
        done,
        Settled::Rejected {
            proposal: pid("p-1")
        }
    );
    assert_eq!(proposals[0].state, ProposalState::Rejected);
    assert_eq!(entries, before);
    let approve = consistency::settle(
        &Settle::Approve(pid("p-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(approve.refusal_code(), Some("proposal_settled"));
}

#[test]
fn retire_hides_an_active_entry_only() {
    let mut entries = store();
    let mut proposals = Vec::new();
    let done = consistency::settle(
        &Settle::Retire(id("decisions-3")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(done, Settled::Retired(id("decisions-3")));
    assert_eq!(entries[2].status, EntryStatus::Retired);
    assert_eq!(
        entries[2].content, "the release train leaves on Thursdays",
        "the text stays"
    );
    let again = consistency::settle(
        &Settle::Retire(id("decisions-3")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert!(matches!(again, Settled::Already(_)));
    entries[0].status = EntryStatus::Superseded;
    let superseded = consistency::settle(
        &Settle::Retire(id("decisions-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(superseded.refusal_code(), Some("entry_not_active"));
    let missing = consistency::settle(
        &Settle::Retire(id("decisions-9")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(missing.refusal_code(), Some("entry_not_found"));
}

#[test]
fn forget_blanks_the_entry_and_every_proposal_text_about_it() {
    let mut entries = store();
    let mut proposals = vec![
        proposal(
            "p-1",
            ProposalKind::Supersedes,
            "decisions-1",
            Some("decisions-2"),
            ProposalState::Open,
        ),
        proposal(
            "p-2",
            ProposalKind::Stale,
            "decisions-3",
            None,
            ProposalState::Open,
        ),
    ];
    let done = consistency::settle(
        &Settle::Forget(id("decisions-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(done, Settled::Forgotten(id("decisions-1")));
    assert_eq!(entries[0].status, EntryStatus::Forgotten);
    assert_eq!(
        (entries[0].said.as_str(), entries[0].content.as_str()),
        ("", "")
    );
    assert_eq!(entries[0].cite, store()[0].cite, "the cite stays");
    assert_eq!(proposals[0].reason, "");
    assert_eq!(proposals[0].quotes[0].text, "");
    assert_eq!(
        proposals[1].reason, "why",
        "a proposal about another entry keeps its text"
    );
    let again = consistency::settle(
        &Settle::Forget(id("decisions-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert!(matches!(again, Settled::Already(_)));
}

#[test]
fn reapply_finishes_an_approval_whose_entry_is_still_active() {
    let mut entries = store();
    let proposals = vec![
        proposal(
            "p-1",
            ProposalKind::Supersedes,
            "decisions-1",
            Some("decisions-2"),
            ProposalState::Approved,
        ),
        proposal(
            "p-2",
            ProposalKind::Stale,
            "decisions-3",
            None,
            ProposalState::Rejected,
        ),
    ];
    let done = consistency::reapply(&proposals, &mut entries, NOW);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].proposal, pid("p-1"));
    assert_eq!(entries[0].status, EntryStatus::Superseded);
    assert_eq!(entries[0].superseded_by, Some(id("decisions-2")));
    assert_eq!(
        entries[2].status,
        EntryStatus::Active,
        "a rejected proposal changes nothing"
    );
    assert!(
        consistency::reapply(&proposals, &mut entries, NOW).is_empty(),
        "once"
    );
}

#[test]
fn a_draft_is_dropped_once_its_entries_are_not_both_active() {
    let mut entries = store();
    let draft = Draft {
        kind: ProposalKind::Supersedes,
        older: id("decisions-1"),
        newer: Some(id("decisions-2")),
        reason: String::new(),
        quotes: Vec::new(),
    };
    assert!(!consistency::is_settled_or_gone(&draft, &[], &entries));
    entries[1].status = EntryStatus::Forgotten;
    assert!(consistency::is_settled_or_gone(&draft, &[], &entries));
}

trait RefusalCode {
    fn refusal_code(&self) -> Option<&'static str>;
}

impl RefusalCode for Settled {
    fn refusal_code(&self) -> Option<&'static str> {
        match self {
            Settled::Refused(why) => Some(SettleRefusal::code(why)),
            Settled::Approved { .. }
            | Settled::Rejected { .. }
            | Settled::Retired(_)
            | Settled::Forgotten(_)
            | Settled::Already(_) => None,
        }
    }
}
