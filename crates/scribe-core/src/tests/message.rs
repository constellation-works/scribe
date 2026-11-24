//! Which of a pass window's turns are kept in `scribe.db`, and the search
//! hits made of them.

use super::{T0, session};
use crate::entry::{Cite, Speaker};
use crate::message::{self, MessageMatch, MessageSearch};
use crate::models::PassMessage;
use crate::pass::PassClaim;
use crate::proposal::EntryCandidate;
use crate::record::{DateRange, Label, Project};
use crate::search::{self, SearchQuery};
use crate::transcript;
use std::num::NonZeroUsize;
use time::Duration;

fn said(uuid: &str, role: Speaker, text: &str) -> (PassMessage, transcript::Message) {
    (
        PassMessage {
            uuid: uuid.to_owned(),
            role,
            text: text.to_owned(),
            pasted: Vec::new(),
        },
        transcript::Message {
            uuid: uuid.to_owned(),
            role,
            at: T0 + Duration::minutes(1),
            blocks: vec![text.to_owned()],
        },
    )
}

fn claim(role: Speaker, message: &str) -> PassClaim {
    PassClaim {
        candidate: EntryCandidate {
            tag: "todo".parse().unwrap(),
            role,
            quote: String::from("quote"),
            context: None,
            statement: None,
            subject: None,
            evidence: None,
            cite: Cite {
                session: "s-1".parse().unwrap(),
                message: Some(message.to_owned()),
                at: T0,
                prompt_sha256: "0".repeat(64),
                line: 1,
                cwd: String::from("/work"),
            },
            project: Project::sanitized("scribe"),
            provider: Label::sanitized("claude-code", "claude-code"),
            host: Label::sanitized("host", "host"),
            admitted: None,
        },
        turn: String::new(),
        outcome: None,
        outcome_turn: None,
    }
}

#[test]
fn typed_text_drops_fenced_code_and_the_blank_lines_it_leaves() {
    assert_eq!(
        message::typed_text("Keep this.\n\n```rust\nfn x() {}\n```\n\nAnd this."),
        "Keep this.\n\nAnd this."
    );
    assert_eq!(
        message::typed_text("~~~~\ncode\n~~~\nstill code\n~~~~\nafter"),
        "after"
    );
    assert_eq!(message::typed_text("before\n```\nnever closed"), "before");
    assert_eq!(message::typed_text("```\nonly code\n```"), "");
    assert_eq!(
        message::typed_text("inline `code` stays"),
        "inline `code` stays"
    );
}

#[test]
fn a_window_keeps_typed_user_turns_and_only_the_assistant_turns_a_claim_cites() {
    let (u1, t1) = said("u-1", Speaker::User, "Keep the merge queue.");
    let (a1, ta1) = said("a-1", Speaker::Assistant, "TODO: drop the release train.");
    let (a2, ta2) = said("a-2", Speaker::Assistant, "Done.");
    let (u2, t2) = said("u-2", Speaker::User, "```\nonly code\n```");
    let (u3, _) = said("u-3", Speaker::User, "Not in the transcript.");
    let window = [u1, a1, a2, u2, u3];
    let transcript = [t1, ta1, ta2, t2];
    let session = "s-1".parse().unwrap();
    let kept = message::window(
        &session,
        &window,
        &transcript,
        &[
            claim(Speaker::Assistant, "a-1"),
            claim(Speaker::User, "u-1"),
        ],
    );
    let ids: Vec<(&str, Speaker)> = kept.iter().map(|m| (m.message.as_str(), m.role)).collect();
    assert_eq!(ids, [("u-1", Speaker::User), ("a-1", Speaker::Assistant)]);
    assert_eq!(kept[1].text, "TODO: drop the release train.");
    assert_eq!(kept[0].at, T0 + Duration::minutes(1));
}

fn query(terms: &str) -> SearchQuery {
    SearchQuery {
        terms: terms.to_owned(),
        kind: Some(crate::ask::RecordKind::Message),
        tag: None,
        project: None,
        topics: Vec::new(),
        dates: DateRange::default(),
        include_hidden: false,
        limit: NonZeroUsize::new(20).unwrap(),
    }
}

fn found(session: &str, id: &str, score: f64) -> MessageMatch {
    MessageMatch {
        session: session.parse().unwrap(),
        message: id.to_owned(),
        role: Speaker::User,
        at: T0,
        text: String::from("keep the merge queue"),
        score,
        group_project: None,
    }
}

#[test]
fn message_hits_filter_through_their_session_and_hide_an_inactive_one() {
    let active = session("s-1", T0);
    let mut inactive = session("s-2", T0);
    inactive.active = false;
    let mut other = session("s-3", T0);
    other.project = Project::sanitized("orbit");
    let stored = MessageSearch {
        matches: vec![
            found("s-1", "m-1", 2.5),
            found("s-2", "m-2", 2.0),
            found("s-3", "m-3", 1.5),
            found("s-9", "m-9", 1.0),
        ],
        read_only_copy: false,
    };
    let sessions = [active, inactive, other];
    let all = search::messages(&query("merge"), &stored, &sessions).unwrap();
    let ids: Vec<_> = all
        .hits
        .iter()
        .map(|h| h.message.clone().unwrap())
        .collect();
    assert_eq!(ids, ["m-1", "m-3"], "no record for s-9; s-2 hidden");
    assert_eq!(all.hidden_matches, 1);
    assert_eq!(all.hits[0].cite, "session:s-1");
    assert_eq!(all.hits[0].kind.as_str(), "message");

    let mut scoped = query("merge");
    scoped.project = Some(Project::sanitized("orbit"));
    scoped.include_hidden = true;
    let hits = search::messages(&scoped, &stored, &sessions).unwrap().hits;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message.as_deref(), Some("m-3"));
    assert!(search::messages(&query("the"), &stored, &sessions).is_err());
}

#[test]
fn a_turn_with_a_finance_or_sensitive_event_word_is_not_kept() {
    assert!(message::keeps("keep the merge queue"));
    assert!(!message::keeps(""));
    assert!(!message::keeps("my salary went up"));
    assert!(!message::keeps("my grandmother died last week"));
}
