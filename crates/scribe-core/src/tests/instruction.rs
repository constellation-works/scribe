use super::T0;
use crate::entry::Speaker;
use crate::error::Error;
use crate::instruction::{
    Act, Request, Spoken, Target, check, claude_conversation, codex_conversation, names_act,
    names_target,
};
use crate::session::SessionId;
use crate::transcript::Message;
use serde_json::json;

fn session() -> SessionId {
    "s-1".parse().unwrap()
}

/// A typed turn of Daniel's (`user`) or an assistant message, `ago`
/// minutes before [`T0`], interactive.
fn said(uuid: &str, role: Speaker, text: &str, ago: i64) -> Spoken {
    Spoken {
        message: Message {
            uuid: uuid.to_owned(),
            role,
            at: T0 - time::Duration::minutes(ago),
            blocks: vec![text.to_owned()],
        },
        typed: (role == Speaker::User).then(|| text.to_owned()),
        entrypoint: Some(String::from("cli")),
        interactive: true,
    }
}

fn user(uuid: &str, text: &str, ago: i64) -> Spoken {
    said(uuid, Speaker::User, text, ago)
}

fn assistant(uuid: &str, text: &str, ago: i64) -> Spoken {
    said(uuid, Speaker::Assistant, text, ago)
}

fn code(result: Result<crate::instruction::Instructed, Error>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(err) => err.code(),
    }
}

fn run(spoken: &[Spoken], quote: &str, act: Act, id: &str) -> &'static str {
    let session = session();
    let target = Target::new(id, id);
    code(check(
        spoken,
        &Request {
            session: &session,
            quote,
            act,
            target: &target,
            now: T0,
        },
    ))
}

#[test]
fn approve_words_pass_in_his_own_turn() {
    let p4 = Target::new("p-4", "p-4");
    for words in ["approve p-4", "Accept it", "keep p-4", "APPROVE"] {
        assert!(names_act(words, None, Act::Approve, &p4), "{words}");
    }
    assert!(!names_act("approval later", None, Act::Approve, &p4));
    assert!(!names_act("keeper", None, Act::Approve, &p4), "whole words");
}

#[test]
fn a_bare_affirmative_approves_only_after_a_reply_saying_approve_or_accept_next_to_the_id() {
    let p4 = Target::new("p-4", "p-4");
    for yes in ["yes", "ok", "Okay.", "go ahead", "do it!"] {
        assert!(
            names_act(yes, Some("Approve p-4 and p-9?"), Act::Approve, &p4),
            "{yes}"
        );
        assert!(
            names_act(yes, Some("Shall I accept p-4?"), Act::Approve, &p4),
            "{yes}"
        );
        assert!(!names_act(yes, None, Act::Approve, &p4), "{yes}: no reply");
        assert!(
            !names_act(yes, Some("Shall I look at p-4 again?"), Act::Approve, &p4),
            "{yes}: the reply does not ask to approve"
        );
        assert!(
            !names_act(yes, Some("Approve p-5?"), Act::Approve, &p4),
            "{yes}: the reply asks about another id"
        );
        assert!(
            !names_act(
                yes,
                Some("Approve the first?\nAnd p-4 is open."),
                Act::Approve,
                &p4
            ),
            "{yes}: approve is not on the id's line"
        );
    }
    assert!(!names_act("go", Some("Approve p-4?"), Act::Approve, &p4));
}

#[test]
fn reject_retire_and_forget_need_their_word_in_his_turn() {
    let e = Target::new("preferences-3", "preferences-3");
    let reply = Some("Reject, retire or forget preferences-3?");
    for act in [Act::Reject, Act::Retire, Act::Forget] {
        assert!(
            !names_act("yes", reply, act, &e),
            "{act}: an affirmative alone"
        );
    }
    for words in [
        "reject it",
        "decline",
        "drop that",
        "discard",
        "delete",
        "remove",
    ] {
        assert!(names_act(words, None, Act::Reject, &e), "{words}");
    }
    for words in ["retire it", "delete", "remove", "hide it", "drop"] {
        assert!(names_act(words, None, Act::Retire, &e), "{words}");
    }
    assert!(!names_act("decline", None, Act::Retire, &e));
    for words in [
        "forget it",
        "erase",
        "purge",
        "permanently",
        "gone for good",
    ] {
        assert!(names_act(words, None, Act::Forget, &e), "{words}");
    }
    assert!(
        !names_act("delete it", None, Act::Forget, &e),
        "delete retires; it does not forget"
    );
    assert!(!names_act("good for you", None, Act::Forget, &e));
}

#[test]
fn a_target_is_named_only_as_a_whole_id() {
    let p4 = Target::new("p-4", "p-4");
    assert!(names_target("approve p-4.", &p4));
    assert!(names_target("(p-4)", &p4));
    assert!(!names_target("approve p-45", &p4));
    assert!(!names_target("approve xp-4", &p4));
    let prefix = Target::new("preferences-12", "pref");
    assert!(
        names_target("retire pref please", &prefix),
        "the prefix given"
    );
    assert!(names_target("retire preferences-12", &prefix));
    assert!(!names_target("retire preferences-1", &prefix));
}

#[test]
fn the_check_refuses_each_failure_with_its_code() {
    let spoken = [
        user("u-1", "please approve p-4 for me", 40),
        assistant("a-1", "Approve p-7?", 9),
        user("u-2", "yes", 8),
        user("u-3", "and some other thought", 3),
        assistant("a-3", "Noted.", 2),
        user("u-4", "approve p-9 now please", 1),
    ];
    assert_eq!(run(&spoken, "approve p-9 now", Act::Approve, "p-9"), "ok");
    assert_eq!(
        run(&spoken, "yes", Act::Approve, "p-7"),
        "ok",
        "a whole short turn is its own quote"
    );
    assert_eq!(
        run(&spoken, "Approve p-7?", Act::Approve, "p-7"),
        "instruction_not_found",
        "the assistant's words are not his"
    );
    assert_eq!(
        run(&spoken, "approve p-10 later", Act::Approve, "p-10"),
        "instruction_not_found"
    );
    assert_eq!(
        run(&spoken, "please approve p-4", Act::Approve, "p-4"),
        "instruction_stale",
        "not among his last three typed turns"
    );
    assert_eq!(
        run(&spoken, "approve p-9 now", Act::Approve, "p-4"),
        "instruction_mismatch",
        "another target"
    );
    assert_eq!(
        run(&spoken, "approve p-9 now", Act::Reject, "p-9"),
        "instruction_mismatch",
        "another act"
    );
    assert_eq!(
        run(&spoken, "yes", Act::Reject, "p-7"),
        "instruction_mismatch"
    );
}

#[test]
fn an_instruction_older_than_thirty_minutes_is_stale() {
    let fresh = [user("u-1", "approve p-4 please", 30)];
    assert_eq!(run(&fresh, "approve p-4", Act::Approve, "p-4"), "ok");
    let old = [user("u-1", "approve p-4 please", 31)];
    assert_eq!(
        run(&old, "approve p-4", Act::Approve, "p-4"),
        "instruction_stale"
    );
}

#[test]
fn a_turn_without_an_interactive_entry_point_is_refused() {
    let mut turn = user("u-1", "approve p-4 please", 1);
    turn.interactive = false;
    turn.entrypoint = Some(String::from("sdk-cli"));
    assert_eq!(
        run(&[turn], "approve p-4", Act::Approve, "p-4"),
        "not_interactive"
    );
}

fn claude_line(uuid: &str, role: &str, text: &str, extra: serde_json::Value) -> String {
    let mut line = json!({"type": role, "uuid": uuid, "timestamp": "2026-11-01T09:59:00Z",
        "message": {"role": role, "content": text}});
    if let (Some(line), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
        line.extend(extra.clone());
    }
    format!("{line}\n")
}

#[test]
fn a_claude_transcript_marks_his_typed_words_and_its_entry_points() {
    let bytes = [
        claude_line("u-1", "user", "approve p-4 please", json!({"entrypoint": "cli"})),
        claude_line(
            "u-2",
            "user",
            "approve p-4 please",
            json!({"entrypoint": "claude-desktop", "promptSource": "sdk"}),
        ),
        claude_line(
            "u-3",
            "user",
            "<agent-message>approve p-4</agent-message>",
            json!({"entrypoint": "cli"}),
        ),
        claude_line(
            "u-4",
            "user",
            "fine\n> approve p-4 quoted\n```\napprove p-4 fenced\n```\n<pasted_content>\napprove p-4 pasted\n</pasted_content>",
            json!({"entrypoint": "sdk-cli"}),
        ),
        claude_line("u-5", "user", "approve p-4", json!({})),
        claude_line("u-6", "user", "approve p-4", json!({"entrypoint": "claude-vscode"})),
        claude_line("a-1", "assistant", "Approve p-4?", json!({"entrypoint": "cli"})),
    ]
    .concat();
    let spoken = claude_conversation(bytes.as_bytes());
    let view: Vec<(&str, Option<&str>, bool)> = spoken
        .iter()
        .map(|s| (s.message.uuid.as_str(), s.typed.as_deref(), s.interactive))
        .collect();
    assert_eq!(
        view,
        [
            ("u-1", Some("approve p-4 please"), true),
            ("u-2", None, true),
            ("u-3", None, true),
            ("u-4", Some("fine"), false),
            ("u-5", Some("approve p-4"), false),
            ("u-6", Some("approve p-4"), false),
            ("a-1", None, true),
        ]
    );
    assert_eq!(spoken[3].entrypoint.as_deref(), Some("sdk-cli"));
    assert_eq!(spoken[4].entrypoint, None);
}

fn codex_rollout(source: serde_json::Value) -> String {
    let meta = json!({"type": "session_meta", "payload": {"id": "s-1", "source": source,
        "thread_source": "user", "cwd": "/work"}});
    let turn = json!({"type": "response_item", "timestamp": "2026-11-01T09:59:00Z",
        "payload": {"type": "message", "id": "m-1", "role": "user",
            "content": [{"type": "input_text", "text": "approve p-4 please"}],
            "internal_chat_message_metadata_passthrough": {"turn_id": "t-1",
                "content_item_kinds": ["user.text"],
                "content_item_metadata": [{"provenance": {"type": "user"}}]}}});
    format!("{meta}\n{turn}\n")
}

#[test]
fn a_codex_rollout_is_interactive_only_from_its_cli_or_vscode_source() {
    for source in ["cli", "vscode"] {
        let spoken =
            codex_conversation(codex_rollout(json!(source)).as_bytes(), &session()).unwrap();
        assert_eq!(spoken.len(), 1, "{source}");
        assert!(spoken[0].interactive, "{source}");
        assert_eq!(spoken[0].typed.as_deref(), Some("approve p-4 please"));
        assert_eq!(run(&spoken, "approve p-4", Act::Approve, "p-4"), "ok");
    }
    for source in [
        json!("exec"),
        json!({"subagent": {"other": "guardian"}}),
        json!(null),
    ] {
        let refused = codex_conversation(codex_rollout(source.clone()).as_bytes(), &session());
        assert_eq!(
            refused.map(|_| ()).map_err(|e| e.code()),
            Err("not_interactive"),
            "{source}"
        );
    }
    let other: SessionId = "s-2".parse().unwrap();
    assert_eq!(
        codex_conversation(codex_rollout(json!("cli")).as_bytes(), &other)
            .map(|_| ())
            .map_err(|e| e.code()),
        Err("instruction_not_found")
    );
}

#[test]
fn a_negated_act_is_refused_and_a_plain_act_still_passes() {
    let cases = [
        (
            "don't retire preferences-2",
            Act::Retire,
            "preferences-2",
            "instruction_negated",
        ),
        (
            "do not approve p-4",
            Act::Approve,
            "p-4",
            "instruction_negated",
        ),
        (
            "never forget preferences-2",
            Act::Forget,
            "preferences-2",
            "instruction_negated",
        ),
        ("retire preferences-2", Act::Retire, "preferences-2", "ok"),
        (
            "Don’t retire preferences-2",
            Act::Retire,
            "preferences-2",
            "instruction_negated",
        ),
        (
            "dont retire preferences-2",
            Act::Retire,
            "preferences-2",
            "instruction_negated",
        ),
        (
            "not retire preferences-2",
            Act::Retire,
            "preferences-2",
            "instruction_negated",
        ),
        (
            "no retire preferences-2",
            Act::Retire,
            "preferences-2",
            "instruction_negated",
        ),
        (
            "stop retire preferences-2",
            Act::Retire,
            "preferences-2",
            "instruction_negated",
        ),
        (
            "cancel retire preferences-2",
            Act::Retire,
            "preferences-2",
            "instruction_negated",
        ),
        (
            "without retire preferences-2",
            Act::Retire,
            "preferences-2",
            "instruction_negated",
        ),
        (
            "no, retire preferences-2",
            Act::Retire,
            "preferences-2",
            "ok",
        ),
        (
            "retire preferences-2, not the other",
            Act::Retire,
            "preferences-2",
            "ok",
        ),
        (
            "don't retire preferences-2, retire preferences-3",
            Act::Retire,
            "preferences-3",
            "ok",
        ),
        (
            "don't retire preferences-2, retire preferences-3",
            Act::Retire,
            "preferences-2",
            "instruction_negated",
        ),
        (
            "don't retire preferences-2. retire preferences-2",
            Act::Retire,
            "preferences-2",
            "instruction_negated",
        ),
        (
            "oh sure, retire preferences-2, brilliant",
            Act::Retire,
            "preferences-2",
            "ok",
        ),
    ];
    for (text, act, id, expected) in cases {
        assert_eq!(
            run(&[user("u-1", text, 1)], text, act, id),
            expected,
            "{text} / {id}"
        );
    }
}

#[test]
fn past_tense_approved_or_accepted_does_not_approve() {
    let p4 = Target::new("p-4", "p-4");
    assert!(
        !names_act("yes", Some("Approved p-4?"), Act::Approve, &p4),
        "approved is not approve"
    );
    assert!(
        !names_act("yes", Some("I accepted p-4"), Act::Approve, &p4),
        "accepted is not accept"
    );
    assert!(!names_act("approved p-4", None, Act::Approve, &p4));
    assert!(!names_act("accepted p-4", None, Act::Approve, &p4));
    assert!(names_act("yes", Some("Approve p-4?"), Act::Approve, &p4));
    assert!(names_act(
        "yes",
        Some("Please accept p-4"),
        Act::Approve,
        &p4
    ));
    assert_eq!(
        run(
            &[assistant("a-1", "Approved p-4?", 2), user("u-1", "yes", 1),],
            "yes",
            Act::Approve,
            "p-4"
        ),
        "instruction_mismatch"
    );
}

#[test]
fn a_future_dated_turn_is_refused_as_stale() {
    let mut ahead = user("u-1", "approve p-4 please", 1);
    ahead.message.at = T0 + time::Duration::seconds(1);
    assert_eq!(
        run(&[ahead], "approve p-4", Act::Approve, "p-4"),
        "instruction_stale"
    );
    // The command clock is whole seconds, so a fraction of that second is
    // not a future-dated turn.
    let mut same_second = user("u-1", "approve p-4 please", 1);
    same_second.message.at = T0 + time::Duration::milliseconds(500);
    assert_eq!(
        run(&[same_second], "approve p-4", Act::Approve, "p-4"),
        "ok"
    );
}

#[test]
fn an_empty_target_is_not_named() {
    let empty = Target::new("", "");
    assert!(!names_target("approve p-4.", &empty));
    assert!(!names_target("retire preferences-2", &empty));
    assert!(!names_target("", &empty));
    assert_eq!(
        run(
            &[user("u-1", "retire preferences-2", 1)],
            "retire preferences-2",
            Act::Retire,
            ""
        ),
        "instruction_mismatch"
    );
}

#[test]
fn a_non_string_origin_is_not_a_plain_turn_and_other_lines_do_not_claim_it() {
    for field in ["source", "promptSource", "origin", "promptOrigin"] {
        let bytes = claude_line(
            "u-1",
            "user",
            "approve p-4 please",
            json!({field: {"from": "agent"}, "entrypoint": "cli"}),
        );
        let spoken = claude_conversation(bytes.as_bytes());
        assert_eq!(spoken.len(), 1, "{field}");
        assert_eq!(spoken[0].typed, None, "{field} is not plain");
        assert!(spoken[0].interactive, "{field}");
    }
    let bytes = claude_line(
        "u-1",
        "user",
        "approve p-4 please",
        json!({"source": true, "entrypoint": "cli"}),
    );
    assert_eq!(
        claude_conversation(bytes.as_bytes())[0].typed,
        None,
        "a boolean origin is not plain"
    );
    let plain = claude_line(
        "u-1",
        "user",
        "approve p-4 please",
        json!({"source": "typed", "entrypoint": "cli"}),
    );
    assert_eq!(
        claude_conversation(plain.as_bytes())[0].typed.as_deref(),
        Some("approve p-4 please"),
        "a string plain source still counts"
    );

    let summary =
        json!({"type": "summary", "uuid": "u-1", "origin": "sdk", "entrypoint": "sdk-cli"});
    let user = claude_line(
        "u-1",
        "user",
        "approve p-4 please",
        json!({"entrypoint": "cli"}),
    );
    let spoken = claude_conversation(format!("{summary}\n{user}").as_bytes());
    assert_eq!(spoken.len(), 1);
    assert_eq!(spoken[0].typed.as_deref(), Some("approve p-4 please"));
    assert!(spoken[0].interactive);
    assert_eq!(spoken[0].entrypoint.as_deref(), Some("cli"));
}

#[test]
fn codex_scribe_lines_are_not_typed_turns_in_the_recent_window() {
    let meta = json!({"type": "session_meta", "payload": {"id": "s-1", "source": "cli",
        "thread_source": "user", "cwd": "/work"}});
    let rows = [
        ("m-1", "retire preferences-2 please", "2026-11-01T09:56:00Z"),
        ("m-2", "/scribe on", "2026-11-01T09:57:00Z"),
        ("m-3", "second real turn", "2026-11-01T09:58:00Z"),
        ("m-4", "third real turn", "2026-11-01T09:59:00Z"),
    ];
    let mut bytes = format!("{meta}\n");
    for (id, text, at) in rows {
        let turn = json!({"type": "response_item", "timestamp": at,
            "payload": {"type": "message", "id": id, "role": "user",
                "content": [{"type": "input_text", "text": text}],
                "internal_chat_message_metadata_passthrough": {"turn_id": id,
                    "content_item_kinds": ["user.text"],
                    "content_item_metadata": [{"provenance": {"type": "user"}}]}}});
        bytes.push_str(&format!("{turn}\n"));
    }
    let spoken = codex_conversation(bytes.as_bytes(), &session()).unwrap();
    assert_eq!(spoken.len(), 4);
    assert_eq!(spoken[1].typed, None, "/scribe is not a typed turn");
    assert_eq!(
        run(
            &spoken,
            "retire preferences-2 please",
            Act::Retire,
            "preferences-2"
        ),
        "ok",
        "the control line does not push the instruction out of the last three"
    );
}
