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
        with("preferences-1", 0, "squash-merge agent PRs into agent-main"),
        with(
            "preferences-2",
            5,
            "rebase-merge agent PRs into agent-main, never squash",
        ),
        with("preferences-3", 6, "the release train leaves on Thursdays"),
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
    // rituals-1 says the same words as preferences-1 but in another tag;
    // preferences-3 shares no words with the others.
    assert_eq!(
        pairs,
        [(String::from("preferences-1"), String::from("preferences-2"))]
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
    assert_eq!(found.pairs[0].older.id, id("preferences-2"));
    assert_eq!(found.pairs[0].newer.id, id("preferences-3"));
    assert_eq!(
        found.over_cap, 1,
        "preferences-1/preferences-2 waits for the next scan"
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
                "preferences-2",
                Some("preferences-1"),
                state,
            ),
            proposal("p-2", ProposalKind::Stale, "preferences-3", None, state),
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
    assert_eq!(stale, ["preferences-3"]);
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
    let request = consistency::request("a-model", &pairs, crate::voice::Voice::default());
    assert_eq!(request.pairs[0].pair, "1");
    assert_eq!(request.pairs[0].older.id, "preferences-1");

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
    assert_eq!(draft.older, id("preferences-1"));
    assert_eq!(draft.newer, Some(id("preferences-2")));
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
    let mut e = with("preferences-3", 6, "the release train leaves on Thursdays");
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
        "preferences-1",
        Some("preferences-2"),
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
            entry: id("preferences-1"),
            status: EntryStatus::Superseded,
            by: Some(id("preferences-2")),
        }
    );
    assert_eq!(proposals[0].state, ProposalState::Approved);
    assert_eq!(proposals[0].settled_at, Some(NOW));
    assert_eq!(proposals[0].settled_by, Some(cite()));
    assert_eq!(entries[0].status, EntryStatus::Superseded);
    assert_eq!(entries[0].superseded_by, Some(id("preferences-2")));
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
        "preferences-3",
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
        "preferences-1",
        Some("preferences-2"),
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
        "preferences-1",
        Some("preferences-2"),
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
        &Settle::Retire(id("preferences-3")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(done, Settled::Retired(id("preferences-3")));
    assert_eq!(entries[2].status, EntryStatus::Retired);
    assert_eq!(
        entries[2].content, "the release train leaves on Thursdays",
        "the text stays"
    );
    let again = consistency::settle(
        &Settle::Retire(id("preferences-3")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert!(matches!(again, Settled::Already(_)));
    entries[0].status = EntryStatus::Superseded;
    let superseded = consistency::settle(
        &Settle::Retire(id("preferences-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(superseded.refusal_code(), Some("entry_not_active"));
    let missing = consistency::settle(
        &Settle::Retire(id("preferences-9")),
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
            "preferences-1",
            Some("preferences-2"),
            ProposalState::Open,
        ),
        proposal(
            "p-2",
            ProposalKind::Stale,
            "preferences-3",
            None,
            ProposalState::Open,
        ),
    ];
    let done = consistency::settle(
        &Settle::Forget(id("preferences-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(done, Settled::Forgotten(id("preferences-1")));
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
        &Settle::Forget(id("preferences-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert!(matches!(again, Settled::Already(_)));
}

#[test]
fn forget_clears_every_free_text_field_of_the_entry_and_its_candidates() {
    use crate::entry::{Speaker, Window, WindowTurn};
    use crate::proposal::{Admitted, EntryCandidate};
    use crate::record::{Author, Label, Project};
    let mut entries = store();
    {
        let e = &mut entries[0];
        e.evidence = Some(String::from("task:ORB-1"));
        e.as_of = Some(date!(2026 - 11 - 01));
        e.quote = Some(String::from("quoted correction"));
        e.retirement_reason = Some(String::from("because of the quoted words"));
        e.window = Some(Window {
            turns: vec![WindowTurn {
                role: Speaker::User,
                message: None,
                at: T0,
                text: String::from("#scribe standard: the forgotten words"),
                cut: false,
            }],
            truncated: false,
            omitted: 0,
        });
    }
    let mut about = proposal(
        "p-1",
        ProposalKind::Candidate,
        "preferences-1",
        None,
        ProposalState::Approved,
    );
    about.candidate = Some(EntryCandidate {
        tag: "preferences".parse().unwrap(),
        role: Speaker::User,
        quote: String::from("the forgotten words"),
        statement: None,
        subject: None,
        evidence: Some(String::from("task:ORB-1")),
        cite: cite(),
        project: Project::sanitized("scribe"),
        provider: Label::sanitized("claude-code", "claude-code"),
        host: Label::sanitized("host", "host"),
        admitted: Some(Admitted {
            trigger: String::from("from now on"),
            matched: String::from("from now on, the forgotten words"),
            criteria: None,
        }),
    });
    about.author = Author::session_pass();
    let mut proposals = vec![about];
    let done = consistency::settle(
        &Settle::Forget(id("preferences-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(done, Settled::Forgotten(id("preferences-1")));
    let e = &entries[0];
    assert_eq!(
        (
            &e.evidence,
            &e.as_of,
            &e.quote,
            &e.retirement_reason,
            &e.window
        ),
        (&None, &None, &None, &None, &None),
        "the entry keeps no forgotten words, evidence date, quote, window or reason"
    );
    let candidate = proposals[0].candidate.as_ref().unwrap();
    assert_eq!((candidate.quote.as_str(), &candidate.evidence), ("", &None));
    let admitted = candidate.admitted.as_ref().unwrap();
    assert_eq!(admitted.matched, "", "the matched words are cleared");
    assert_eq!(
        admitted.trigger, "from now on",
        "the configured trigger stays"
    );
}

#[test]
fn forget_clears_instruction_quotes_while_keeping_metadata() {
    let mut entries = store();
    let inst = crate::Instruction {
        session: "s-9".parse().unwrap(),
        message: String::from("m-9"),
        at: NOW,
        quote: Some(String::from("approve p-1, secret preference words")),
    };
    entries[0].approved_by = Some(crate::Author::agent("aide".parse().unwrap()));
    entries[0].instruction = Some(inst.clone());

    let mut p_about = proposal(
        "p-1",
        ProposalKind::Candidate,
        "preferences-1",
        None,
        ProposalState::Approved,
    );
    p_about.instruction = Some(inst.clone());

    let mut p_other = proposal(
        "p-2",
        ProposalKind::Candidate,
        "preferences-3",
        None,
        ProposalState::Approved,
    );
    p_other.instruction = Some(crate::Instruction {
        quote: Some(String::from("approve p-2")),
        ..inst.clone()
    });

    let mut proposals = vec![p_about, p_other];
    let done = consistency::settle(
        &Settle::Forget(id("preferences-1")),
        &mut proposals,
        &mut entries,
        &cite(),
        NOW,
    );
    assert_eq!(done, Settled::Forgotten(id("preferences-1")));

    let e_inst = entries[0].instruction.as_ref().unwrap();
    assert_eq!(e_inst.session, inst.session);
    assert_eq!(e_inst.message, inst.message);
    assert_eq!(e_inst.at, inst.at);
    assert_eq!(e_inst.quote, None);

    let p_inst = proposals[0].instruction.as_ref().unwrap();
    assert_eq!(p_inst.session, inst.session);
    assert_eq!(p_inst.message, inst.message);
    assert_eq!(p_inst.at, inst.at);
    assert_eq!(p_inst.quote, None);

    assert_eq!(
        proposals[1].instruction.as_ref().unwrap().quote.as_deref(),
        Some("approve p-2"),
        "unrelated proposal keeps its instruction quote"
    );
}

#[test]
fn reapply_finishes_an_approval_whose_entry_is_still_active() {
    let mut entries = store();
    let proposals = vec![
        proposal(
            "p-1",
            ProposalKind::Supersedes,
            "preferences-1",
            Some("preferences-2"),
            ProposalState::Approved,
        ),
        proposal(
            "p-2",
            ProposalKind::Stale,
            "preferences-3",
            None,
            ProposalState::Rejected,
        ),
    ];
    let done = consistency::reapply(&proposals, &mut entries, NOW);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].proposal, pid("p-1"));
    assert_eq!(entries[0].status, EntryStatus::Superseded);
    assert_eq!(entries[0].superseded_by, Some(id("preferences-2")));
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
        older: id("preferences-1"),
        newer: Some(id("preferences-2")),
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
            | Settled::Covered { .. }
            | Settled::Already(_)
            | Settled::Recorded { .. } => None,
        }
    }
}

// Removed-feature sessions (S10).

use crate::consistency::{
    LaterRecord, RemovalPair, has_removal_cue, judge_removals, removal_candidates,
};
use crate::models::{RemovalAnswer, RemovalJudgementAnswer};
use crate::session::{SegmentState, SessionRecord};

const REMOVAL: &str = "The digest feature was ripped out and replaced by the weekly report.";

/// A session about `title`, whose one segment ends an hour after
/// `days_after_t0`, updated then too.
fn about(id: &str, days_after_t0: i64, title: &str) -> SessionRecord {
    let at = T0 + Duration::days(days_after_t0);
    let mut s = super::session(id, at);
    s.title = title.to_owned();
    s.summary = format!("Built the {title}.");
    s.goals = String::new();
    s.next = String::new();
    s.log = vec![super::segment(1, SegmentState::Sealed, at, "work")];
    s
}

fn removal_answer(pair: &str, label: &str, quote: &str) -> RemovalAnswer {
    RemovalAnswer {
        judgements: vec![RemovalJudgementAnswer {
            pair: pair.to_owned(),
            label: label.to_owned(),
            feature: String::from("the digest"),
            reason: String::from("The digest was replaced by the weekly report."),
            quote: quote.to_owned(),
        }],
    }
}

#[test]
fn a_removal_cue_is_a_whole_word_or_phrase() {
    assert!(has_removal_cue("we Removed it"));
    assert!(has_removal_cue("it was ripped out."));
    assert!(has_removal_cue("sunset"));
    assert!(!has_removal_cue("unremovedness and sunsets"));
    assert!(!has_removal_cue("we kept it"));
}

#[test]
fn removal_candidates_pair_an_active_session_with_a_recent_later_record_that_has_a_cue() {
    let now = T0 + Duration::days(40);
    let digest = about("s-digest", 1, "digest feature");
    // A model-written segment claiming removal is never evidence.
    let mut later = about("s-later", 35, "weekly report");
    later.log[0].text = REMOVAL.to_owned();
    // A Daniel-authored entry saying the same, recent and related.
    let mut said = with("preferences-7", 36, "The digest feature was removed.");
    said.updated = said.created;
    // Unrelated cue, an old record, a pinned and an inactive session.
    let other = with("preferences-8", 37, "The flaky lint was removed.");
    let mut old = about("s-old", 2, "old digest feature");
    old.log[0].text = REMOVAL.to_owned();
    let mut pinned = about("s-pinned", 1, "digest feature");
    pinned.keep_active = true;
    let mut inactive = about("s-inactive", 1, "digest feature");
    inactive.active = false;

    let sessions = vec![digest, later, old, pinned, inactive];
    let found = removal_candidates(&sessions, &[said, other], now, 20);

    let pairs: Vec<(String, String)> = found
        .pairs
        .iter()
        .map(|p| (p.session.to_string(), p.later.cite.clone()))
        .collect();
    assert_eq!(
        pairs,
        [
            (String::from("s-digest"), String::from("preferences-7")),
            (String::from("s-old"), String::from("preferences-7")),
        ],
        "only the Daniel-authored entry pairs; never a pinned or inactive \
         session, never a model-written segment, never a record about \
         something else"
    );
    assert_eq!(found.over_cap, 0);

    let capped = removal_candidates(&sessions, &[], now, 0);
    assert!(capped.pairs.is_empty());
    assert_eq!(capped.over_cap, 0, "no entries means no removal pairs");
}

#[test]
fn a_record_before_the_sessions_last_message_is_not_a_candidate() {
    let now = T0 + Duration::days(40);
    let digest = about("s-digest", 36, "digest feature");
    let said = with("preferences-7", 35, "The digest feature was removed.");
    assert!(
        removal_candidates(&[digest], &[said], now, 20)
            .pairs
            .is_empty()
    );
}

#[test]
fn a_removed_label_survives_only_with_a_verbatim_quote_of_the_later_record() {
    let now = T0 + Duration::days(40);
    let digest = about("s-digest", 1, "digest feature");
    let mut said = with("preferences-7", 36, REMOVAL);
    said.updated = said.created;
    let pairs: Vec<RemovalPair> = removal_candidates(&[digest], &[said], now, 20).pairs;
    assert_eq!(pairs.len(), 1);

    let kept = judge_removals(
        removal_answer("1", "removed", "ripped out and replaced"),
        &pairs,
    );
    assert_eq!(kept.dropped, 0);
    assert_eq!(kept.removals.len(), 1);
    let removal = &kept.removals[0];
    assert_eq!(removal.session.as_str(), "s-digest");
    assert_eq!(removal.cite, "preferences-7");
    assert_eq!(removal.quote, "ripped out and replaced");
    assert_eq!(
        removal.reason,
        "The digest was replaced by the weekly report."
    );

    let invented = judge_removals(removal_answer("1", "removed", "the digest is gone"), &pairs);
    assert!(invented.removals.is_empty());
    assert_eq!(
        invented.dropped, 1,
        "a quote not in the later record is dropped"
    );

    let unknown = judge_removals(removal_answer("2", "removed", "ripped out"), &pairs);
    assert_eq!(unknown.dropped, 1);
    let current = judge_removals(removal_answer("1", "still_current", ""), &pairs);
    assert_eq!((current.kept, current.dropped), (1, 0));
    let bad = judge_removals(removal_answer("1", "obsolete", "ripped out"), &pairs);
    assert_eq!(bad.dropped, 1);
}

#[test]
fn agent_note_and_model_segment_claiming_removal_deactivate_nothing() {
    let now = T0 + Duration::days(40);
    let digest = about("s-digest", 1, "digest feature");
    // A model-written segment of a later session claiming removal.
    let mut later = about("s-later", 35, "weekly report");
    later.log[0].text = REMOVAL.to_owned();
    // An agent note claiming the same removal.
    let mut note = with("preferences-9", 36, "The digest feature was removed.");
    note.updated = note.created;
    note.author = crate::record::Author::agent("opus".parse().unwrap());
    note.cite = None;

    let found = removal_candidates(&[digest.clone(), later], &[note], now, 20);
    assert!(
        found.pairs.is_empty(),
        "neither an agent note nor a model-written segment is removal evidence"
    );

    // Daniel's own entry with the same words still is.
    let mut said = with("preferences-10", 36, "The digest feature was removed.");
    said.updated = said.created;
    let found = removal_candidates(&[digest], &[said], now, 20);
    assert_eq!(found.pairs.len(), 1);
    assert_eq!(found.pairs[0].later.cite, "preferences-10");
}

#[test]
fn a_removed_label_citing_a_session_is_dropped_even_with_a_verbatim_quote() {
    // removal_candidates never pairs session text, so this pair cannot come
    // from a scan; judge_removals must still refuse to deactivate on it.
    let digest = about("s-digest", 1, "digest feature");
    let pair = RemovalPair {
        session: digest.id.clone(),
        last_message: T0 + Duration::days(1),
        header: String::from("digest feature"),
        later: LaterRecord {
            cite: String::from("session:s-later#1"),
            record: String::from("s-later"),
            date: T0 + Duration::days(35),
            text: REMOVAL.to_owned(),
            topics: Vec::new(),
        },
    };
    let judged = judge_removals(
        removal_answer("1", "removed", "ripped out and replaced"),
        &[pair],
    );
    assert!(
        judged.removals.is_empty(),
        "no deactivation on a session cite"
    );
    assert_eq!(judged.dropped, 1);
}
