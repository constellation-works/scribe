//! The redaction fixtures and field lists the redactor's tests share: the
//! store's (`tests/redact.rs`) and scribe-core's, which build the redactor
//! from a `Config`.

use crate::proposal::{Proposal, ProposalKind, ProposalState, Quote};
pub use crate::redact::{entry_fields, proposal_fields, session_fields};
use crate::session::{SessionRecord, Term};
use scribe_domain::fixtures::{T0, entry, session};

/// A configured secret's value.
pub const SECRET: &str = "sk-live-0123456789abcdef";

/// A proposal with every free-text field set, so all are listed.
pub fn proposal() -> Proposal {
    Proposal {
        id: "p-1".parse().unwrap(),
        kind: ProposalKind::Supersedes,
        older: "preferences-1".parse().unwrap(),
        newer: Some("preferences-2".parse().unwrap()),
        reason: String::new(),
        quotes: vec![
            Quote {
                entry: "preferences-1".parse().unwrap(),
                text: String::new(),
            },
            Quote {
                entry: "preferences-2".parse().unwrap(),
                text: String::new(),
            },
        ],
        state: ProposalState::Open,
        opened_at: T0,
        settled_at: None,
        settled_by: None,
        author: crate::record::Author::consistency(),
        updated_by: None,
        settled_author: None,
        closed_reason: None,
        instruction: Some(crate::Instruction {
            session: "s-1".parse().unwrap(),
            message: String::from("m-1"),
            at: T0,
            quote: Some(String::new()),
        }),
        candidate: Some(crate::EntryCandidate {
            tag: "preferences".parse().unwrap(),
            role: crate::Speaker::User,
            quote: String::new(),
            context: Some(String::new()),
            statement: Some(String::new()),
            subject: Some(String::new()),
            evidence: Some(String::new()),
            cite: entry("preferences-1", T0).cite.unwrap(),
            project: crate::Project::sanitized("scribe"),
            provider: crate::Label::sanitized("claude-code", "claude-code"),
            host: crate::Label::sanitized("host", "host"),
            admitted: Some(crate::Admitted {
                trigger: String::from("I prefer"),
                matched: String::new(),
                criteria: Some(String::new()),
            }),
        }),
    }
}

/// A session record with one term, so the term's fields are listed.
pub fn session_with_term() -> SessionRecord {
    let mut s = session("s-1", T0);
    s.active_change = Some(crate::session::ActiveChange {
        at: T0,
        by: crate::record::Author::user(),
        route: crate::session::ActiveRoute::Manual,
        reason: String::new(),
        cite: None,
    });
    s.terms.push(Term {
        term: String::new(),
        said: String::new(),
        message: String::from("m-1"),
    });
    s
}

/// An outcome with both free-text fields set, so both are listed. No
/// stored entry holds both (each state allows one); the redactor's field
/// list does not depend on the state.
pub fn outcome() -> crate::Outcome {
    crate::Outcome {
        state: crate::OutcomeState::Parked,
        reason: Some(String::new()),
        task: Some(String::new()),
    }
}

/// Set an entry's decision and open item, with every free-text field.
pub fn entry_records(entry: &mut crate::Entry) {
    let agent: crate::AuthorName = "0-chief".parse().unwrap();
    entry.decision = Some(crate::Decision {
        decided_by: agent.clone(),
        rationale: Some(String::new()),
        reason_quote: Some(String::new()),
        options: vec![String::new()],
        recommended: None,
        supersedes: None,
        settles: None,
    });
    entry.item = Some(crate::OpenItem {
        asked_by: agent.clone(),
        options: vec![String::new()],
        recommended: None,
        why: Some(String::new()),
        blocked: String::new(),
        answer_in: crate::AnswerIn::Any,
        answer: Some(crate::ItemAnswer {
            words: String::new(),
            session: "s-1".parse().unwrap(),
            at: T0,
            recorded_by: agent.clone(),
        }),
        closed: Some(crate::ItemClosure {
            at: T0,
            by: agent,
            decision: None,
            note: String::new(),
        }),
    });
}
