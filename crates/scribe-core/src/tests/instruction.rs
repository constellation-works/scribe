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
fn conservative_instruction_regressions() {
    for (text, act, id) in [
        ("don't retire preferences-2", Act::Retire, "preferences-2"),
        ("do not approve p-4", Act::Approve, "p-4"),
        ("never forget preferences-2", Act::Forget, "preferences-2"),
        (
            "Retire preferences-3, not preferences-2",
            Act::Retire,
            "preferences-2",
        ),
        (
            "Do not, under any circumstances, retire preferences-2",
            Act::Retire,
            "preferences-2",
        ),
        (
            "Please don't\nretire preferences-2",
            Act::Retire,
            "preferences-2",
        ),
        (
            "Don't retire preferences-2, preferences-3, or preferences-4.",
            Act::Retire,
            "preferences-3",
        ),
        (
            "retire preferences-2 but keep another",
            Act::Retire,
            "preferences-2",
        ),
        (
            "retire preferences-2 except that one",
            Act::Retire,
            "preferences-2",
        ),
        ("Don't forget to approve p-4", Act::Approve, "p-4"),
    ] {
        assert_eq!(
            run(&[user("u", text, 0)], text, act, id),
            "instruction_mismatch",
            "{text}"
        );
    }
    for (reply, answer) in [
        ("Approve p-4?", "Yes, but not p-4"),
        ("Don't approve p-4, but approve p-9?", "yes"),
    ] {
        assert_eq!(
            run(
                &[assistant("a", reply, 0), user("u", answer, 0)],
                answer,
                Act::Approve,
                "p-4"
            ),
            "instruction_mismatch"
        );
    }
    for answer in ["yes, approve it", "approve p-4"] {
        assert_eq!(
            run(
                &[assistant("a", "Approve p-4?", 0), user("u", answer, 0)],
                answer,
                Act::Approve,
                "p-4"
            ),
            "ok"
        );
    }
    assert_eq!(
        run(
            &[user("u", "retire preferences-2", 0)],
            "retire preferences-2",
            Act::Retire,
            "preferences-2"
        ),
        "ok"
    );
    for reply in ["approved p-4?", "accepted p-4?"] {
        assert_eq!(
            run(
                &[assistant("a", reply, 0), user("u", "yes", 0)],
                "yes",
                Act::Approve,
                "p-4"
            ),
            "instruction_mismatch"
        );
    }
    assert!(!names_target("approve anything", &Target::new("", "")));
}

#[test]
fn precise_freshness_boundaries() {
    let mut future = user("u", "approve p-4", 0);
    future.message.at = T0 + time::Duration::nanoseconds(1);
    assert_eq!(
        run(&[future], "approve p-4", Act::Approve, "p-4"),
        "instruction_stale"
    );
    assert_eq!(
        run(
            &[user("u", "approve p-4", 30)],
            "approve p-4",
            Act::Approve,
            "p-4"
        ),
        "ok"
    );
}

#[test]
fn origin_claims_fail_closed_and_ignore_nonconversation_lines() {
    for field in [
        "source",
        "promptSource",
        "prompt_source",
        "origin",
        "promptOrigin",
        "prompt_origin",
        "turnOrigin",
        "turn_origin",
    ] {
        for value in [json!(null), json!(42), json!({}), json!([]), json!(true)] {
            let mut extra = json!({"entrypoint":"cli"});
            extra[field] = value;
            let line = claude_line("u", "user", "approve p-4", extra);
            assert_eq!(
                run(
                    &claude_conversation(line.as_bytes()),
                    "approve p-4",
                    Act::Approve,
                    "p-4"
                ),
                "instruction_not_found",
                "{field}"
            );
        }
    }
    for origin in [
        json!({"kind": null}),
        json!({"kind": 42}),
        json!({"kind": ""}),
        json!({"kind": "  "}),
        json!({"kind": {}}),
    ] {
        let line = claude_line(
            "u",
            "user",
            "approve p-4",
            json!({"entrypoint": "claude-desktop", "origin": origin, "promptSource": "sdk", "turnOrigin": "human"}),
        );
        assert_eq!(
            run(
                &claude_conversation(line.as_bytes()),
                "approve p-4",
                Act::Approve,
                "p-4"
            ),
            "instruction_not_found",
            "{origin}"
        );
    }
    let non_message =
        json!({"type":"progress", "uuid":"u", "entrypoint":"sdk-cli", "origin":"agent"});
    let line = claude_line("u", "user", "approve p-4", json!({"entrypoint":"cli"}));
    assert_eq!(
        run(
            &claude_conversation(format!("{non_message}\n{line}").as_bytes()),
            "approve p-4",
            Act::Approve,
            "p-4"
        ),
        "ok"
    );
}

#[test]
fn claude_desktop_origin_shapes_accept_human_and_refuse_synthetic_turns() {
    let human = claude_line(
        "u-human",
        "user",
        "approve p-4 please",
        json!({
            "entrypoint": "claude-desktop",
            "origin": {"kind": "human"},
            "promptSource": "sdk",
            "turnOrigin": "human"
        }),
    );
    let spoken = claude_conversation(human.as_bytes());
    assert_eq!(spoken.len(), 1);
    assert!(spoken[0].interactive);
    assert_eq!(spoken[0].typed.as_deref(), Some("approve p-4 please"));
    assert_eq!(run(&spoken, "approve p-4", Act::Approve, "p-4"), "ok");

    let peer_meta = claude_line(
        "u-peer",
        "user",
        "approve p-4 please",
        json!({
            "entrypoint": "claude-desktop",
            "origin": {
                "kind": "peer",
                "from": "local_1",
                "name": "peer",
                "body": "approve p-4 please",
                "hostInjected": true
            },
            "promptSource": "system",
            "turnOrigin": "peer",
            "isMeta": true
        }),
    );
    let peer_no_meta = claude_line(
        "u-peer-no-meta",
        "user",
        "approve p-4 please",
        json!({
            "entrypoint": "claude-desktop",
            "origin": {"kind": "peer"},
            "promptSource": "system",
            "turnOrigin": "peer"
        }),
    );
    let task_notification = claude_line(
        "u-task",
        "user",
        "approve p-4 please",
        json!({
            "entrypoint": "claude-desktop",
            "origin": {
                "kind": "task-notification",
                "producer": "session-task",
                "runId": "r-1"
            },
            "promptSource": "system",
            "turnOrigin": "task_notification"
        }),
    );
    let host_synthetic = claude_line(
        "u-synth",
        "user",
        "approve p-4 please",
        json!({
            "entrypoint": "claude-desktop",
            "promptSource": "sdk",
            "turnOrigin": "host_synthetic"
        }),
    );
    for (name, line) in [
        ("peer_meta", peer_meta),
        ("peer_no_meta", peer_no_meta),
        ("task_notification", task_notification),
        ("host_synthetic", host_synthetic),
    ] {
        assert_eq!(
            run(
                &claude_conversation(line.as_bytes()),
                "approve p-4",
                Act::Approve,
                "p-4"
            ),
            "instruction_not_found",
            "{name}"
        );
    }
}

#[test]
fn prompt_source_sdk_requires_plain_turn_origin_or_origin_kind() {
    for (name, extra, expected) in [
        (
            "origin_kind_only",
            json!({"entrypoint": "claude-desktop", "origin": {"kind": "human"}, "promptSource": "sdk"}),
            "ok",
        ),
        (
            "turn_origin_only",
            json!({"entrypoint": "claude-desktop", "promptSource": "sdk", "turnOrigin": "human"}),
            "ok",
        ),
        (
            "bare_sdk",
            json!({"entrypoint": "claude-desktop", "promptSource": "sdk"}),
            "instruction_not_found",
        ),
        (
            "sdk_with_empty_turn_origin",
            json!({"entrypoint": "claude-desktop", "promptSource": "sdk", "turnOrigin": ""}),
            "instruction_not_found",
        ),
        (
            "sdk_with_source_human_only",
            json!({"entrypoint": "claude-desktop", "source": "human", "promptSource": "sdk"}),
            "instruction_not_found",
        ),
        (
            "source_sdk_with_turn_origin_human",
            json!({"entrypoint": "claude-desktop", "source": "sdk", "turnOrigin": "human"}),
            "instruction_not_found",
        ),
        (
            "conflicting_turn_origin",
            json!({"entrypoint": "claude-desktop", "origin": {"kind": "human"}, "promptSource": "sdk", "turnOrigin": "host_synthetic"}),
            "instruction_not_found",
        ),
        (
            "system_with_human_origin",
            json!({"entrypoint": "claude-desktop", "origin": {"kind": "human"}, "promptSource": "system", "turnOrigin": "human"}),
            "instruction_not_found",
        ),
    ] {
        let line = claude_line("u", "user", "approve p-4 please", extra);
        assert_eq!(
            run(
                &claude_conversation(line.as_bytes()),
                "approve p-4",
                Act::Approve,
                "p-4"
            ),
            expected,
            "{name}"
        );
    }
}

#[test]
fn codex_control_lines_do_not_age_out_the_instruction() {
    let mut rollout = codex_rollout(json!("cli"));
    for i in 0..3 {
        let row = json!({"type":"response_item", "timestamp":"2026-11-01T09:59:00Z", "payload":{"type":"message", "id":format!("control-{i}"), "role":"user", "content":[{"type":"input_text", "text":"/scribe status"}], "internal_chat_message_metadata_passthrough":{"turn_id":format!("t-{i}"), "content_item_kinds":["user.text"], "content_item_metadata":[{"provenance":{"type":"user"}}]}}});
        rollout.push_str(&format!("{row}\n"));
    }
    let spoken = codex_conversation(rollout.as_bytes(), &session()).unwrap();
    assert_eq!(run(&spoken, "approve p-4", Act::Approve, "p-4"), "ok");
}
