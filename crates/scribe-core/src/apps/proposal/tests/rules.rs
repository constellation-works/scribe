use super::entry;
use crate::consistency::{self, Draft, Settle, SettleRefusal, Settled};
use crate::entry::{Cite, Entry, EntryId, EntryStatus};
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
        context: None,
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
            | Settled::SessionForgotten(_)
            | Settled::SessionForgetRepaired { .. }
            | Settled::Covered { .. }
            | Settled::Already(_)
            | Settled::Recorded { .. } => None,
        }
    }
}
