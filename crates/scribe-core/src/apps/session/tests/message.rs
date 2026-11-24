//! Which of a pass window's turns are kept in `scribe.db`, and the search
//! hits made of them.

use super::T0;
use crate::claim::PassClaim;
use crate::entry::{Cite, Speaker};
use crate::message;
use crate::models::PassMessage;
use crate::proposal::EntryCandidate;
use crate::record::{Label, Project};
use crate::transcript;
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
