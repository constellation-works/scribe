//! End-to-end tests of the instruction check (docs/design/scribe/2_design.md
//! §12.2, §12.5): `--session` and `--instruction` on `proposal settle` and
//! `entry retire`, given together or not at all, and each refusal code,
//! which changes nothing in the store.
//!
//! Every run is the built binary with a temp `HOME` and data root; the
//! transcripts are fixtures written relative to the real clock, since the
//! check measures an instruction's age against the time the command runs.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Output;
use support::{Fixture, make_private, snapshot, text};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
const CODEX_SESSION: &str = "11111111-2222-4333-8444-555555555555";

/// One transcript line: `(uuid, role, text, minutes before now)`.
type Turn<'a> = (&'a str, &'a str, &'a str, i64);

/// A Claude Code transcript under the fixture's home, every line carrying
/// `entrypoint` (or none, for `None`).
fn claude(fx: &Fixture, entrypoint: Option<&str>, turns: &[Turn<'_>]) -> PathBuf {
    let project = fx.home().join(".claude/projects/-work-scribe");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{SESSION}.jsonl"));
    let now = OffsetDateTime::now_utc();
    let mut out = String::new();
    for (uuid, role, body, ago) in turns {
        let content = if *role == "user" {
            json!(body)
        } else {
            json!([{"type": "text", "text": body}])
        };
        let at = (now - time::Duration::minutes(*ago))
            .format(&Rfc3339)
            .unwrap();
        let mut line = json!({"type": role, "uuid": uuid, "sessionId": SESSION,
            "timestamp": at, "cwd": "/work/scribe",
            "message": {"role": role, "content": content}});
        if let Some(entrypoint) = entrypoint {
            line["entrypoint"] = json!(entrypoint);
        }
        out.push_str(&format!("{line}\n"));
    }
    std::fs::write(&path, out).unwrap();
    make_private(&path);
    path
}

/// A Codex rollout recorded with `source`, holding Daniel's typed `turns`
/// `(id, text, minutes before now)`.
fn codex(fx: &Fixture, source: &str, turns: &[(&str, &str, i64)]) {
    let dir = fx.home().join(".codex/sessions/2026/11/21");
    fx.create_private_dir_all(&dir);
    let path = dir.join(format!("rollout-2026-11-21T00-00-00-{CODEX_SESSION}.jsonl"));
    let now = OffsetDateTime::now_utc();
    let mut out = format!(
        "{}\n",
        json!({"type": "session_meta", "payload": {"id": CODEX_SESSION, "source": source,
            "thread_source": "user", "cwd": fx.home()}})
    );
    for (id, body, ago) in turns {
        let at = (now - time::Duration::minutes(*ago))
            .format(&Rfc3339)
            .unwrap();
        out.push_str(&format!(
            "{}\n",
            json!({"type": "response_item", "timestamp": at, "payload": {"type": "message",
                "id": id, "role": "user", "content": [{"type": "input_text", "text": body}],
                "internal_chat_message_metadata_passthrough": {"turn_id": id,
                    "content_item_kinds": ["user.text"],
                    "content_item_metadata": [{"provenance": {"type": "user"}}]}}})
        ));
    }
    std::fs::write(&path, out).unwrap();
    make_private(&path);
}

/// Run `args` with the seeded store's `[redact]` env name set.
fn exec(fx: &Fixture, args: &[&str]) -> Output {
    fx.cmd(args)
        .env("GITHUB_TOKEN", "synthetic-fixture-token-value")
        .output()
        .unwrap()
}

/// [`exec`], requiring exit 0.
fn ok(fx: &Fixture, args: &[&str]) -> Output {
    let out = exec(fx, args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{args:?}: {}",
        text(&out.stderr)
    );
    out
}

/// Run `args` with `--json`; the exit code and the error code, if any.
fn run(fx: &Fixture, args: &[&str]) -> (Option<i32>, Option<String>) {
    let mut args = args.to_vec();
    args.push("--json");
    let out = exec(fx, &args);
    let code = serde_json::from_slice::<Value>(&out.stderr)
        .ok()
        .and_then(|err| err["code"].as_str().map(str::to_owned));
    (out.status.code(), code)
}

/// `entry retire <id>` on `instruction` in [`SESSION`].
fn retire<'a>(id: &'a str, instruction: &'a str) -> Vec<&'a str> {
    vec![
        "entry",
        "retire",
        id,
        "--author",
        "opus",
        "--reason",
        "Daniel asked",
        "--session",
        SESSION,
        "--instruction",
        instruction,
    ]
}

/// A seeded store with one agent entry, `preferences-2`.
fn store_with_agent_entry() -> Fixture {
    let fx = Fixture::new();
    fx.seed();
    ok(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "Use the stable API",
        ],
    );
    fx
}

/// The store's files, but the audit log and the lock file, which every
/// audited refusal appends to or rewrites.
fn state(fx: &Fixture) -> Vec<(PathBuf, Vec<u8>, u32)> {
    snapshot(&fx.root())
        .into_iter()
        .filter(|(path, _, _)| !path.ends_with("audit.jsonl") && !path.ends_with(".lock"))
        .collect()
}

#[test]
fn session_and_instruction_come_together_or_not_at_all() {
    let fx = store_with_agent_entry();
    let before = state(&fx);
    for args in [
        vec![
            "proposal",
            "settle",
            "p-1",
            "--approve",
            "--author",
            "opus",
            "--session",
            SESSION,
        ],
        vec![
            "proposal",
            "settle",
            "p-1",
            "--reject",
            "--author",
            "opus",
            "--instruction",
            "reject p-1 please",
        ],
        vec![
            "entry",
            "retire",
            "preferences-2",
            "--author",
            "opus",
            "--reason",
            "r",
            "--session",
            SESSION,
        ],
        vec![
            "entry",
            "retire",
            "preferences-2",
            "--author",
            "opus",
            "--reason",
            "r",
            "--instruction",
            "retire preferences-2",
        ],
    ] {
        let out = exec(&fx, &args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    assert_eq!(state(&fx), before, "a usage error changes nothing");
}

#[test]
fn each_check_refuses_with_its_code_and_changes_nothing() {
    let fx = store_with_agent_entry();
    let before = state(&fx);
    // No transcript of the session on this host.
    assert_eq!(
        run(&fx, &retire("preferences-2", "retire preferences-2 now")),
        (Some(1), Some(String::from("instruction_not_found")))
    );
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-1", "user", "please retire preferences-2 for me", 50),
            ("a-1", "assistant", "Retire preferences-2?", 49),
            (
                "u-2",
                "user",
                "> retire preferences-2 quoted\nlooks fine",
                8,
            ),
            ("a-2", "assistant", "Noted.", 7),
            (
                "u-3",
                "user",
                "<agent-message>retire preferences-2 now</agent-message>",
                6,
            ),
            ("u-4", "user", "remove preferences-1 instead", 5),
            ("a-4", "assistant", "Shall I go on?", 4),
            ("u-5", "user", "first turn of three", 3),
            ("u-6", "user", "second turn of three", 2),
            ("u-7", "user", "hide the thing we discussed", 1),
        ],
    );
    let cases = [
        // Not his words at all, an assistant's, a blockquote, an agent turn.
        ("never typed by anyone", "instruction_not_found"),
        ("Retire preferences-2?", "instruction_not_found"),
        ("retire preferences-2 quoted", "instruction_not_found"),
        ("retire preferences-2 now", "instruction_not_found"),
        // His, but older than his last three typed turns and 50 minutes old.
        ("please retire preferences-2", "instruction_stale"),
        // Recent, but naming another entry, or no retire word.
        ("hide the thing we discussed", "instruction_mismatch"),
        ("second turn of three", "instruction_mismatch"),
    ];
    for (instruction, code) in cases {
        assert_eq!(
            run(&fx, &retire("preferences-2", instruction)),
            (Some(1), Some(String::from(code))),
            "{instruction}"
        );
    }
    // Four typed turns since: u-4 is out of the last three.
    assert_eq!(
        run(
            &fx,
            &retire("preferences-1", "remove preferences-1 instead")
        ),
        (Some(1), Some(String::from("instruction_stale")))
    );
    assert_eq!(state(&fx), before, "every refusal changes nothing");
    let audit = std::fs::read_to_string(fx.root().join("audit.jsonl")).unwrap();
    for code in [
        "instruction_not_found",
        "instruction_stale",
        "instruction_mismatch",
    ] {
        assert!(
            audit.lines().any(|l| {
                let line: Value = serde_json::from_str(l).unwrap();
                line["error_code"] == code
            }),
            "{code} is audited"
        );
    }
    assert!(
        !audit.contains("please retire"),
        "the instruction's words are never audited"
    );
}

#[test]
fn a_fresh_instruction_naming_the_target_and_act_passes() {
    let fx = store_with_agent_entry();
    claude(
        &fx,
        Some("cli"),
        &[
            ("a-1", "assistant", "Shall I retire preferences-2?", 2),
            ("u-1", "user", "yes", 1),
            ("u-2", "user", "retire it, preferences-2 is wrong", 0),
        ],
    );
    assert_eq!(
        run(&fx, &retire("preferences-2", "yes")),
        (Some(1), Some(String::from("instruction_mismatch"))),
        "an affirmative alone does not retire"
    );
    let out = ok(
        &fx,
        &retire("preferences-2", "retire it, preferences-2 is wrong"),
    );
    assert!(out.status.success());
    let shown: Value =
        serde_json::from_slice(&ok(&fx, &["entry", "show", "preferences-2", "--json"]).stdout)
            .unwrap();
    assert_eq!(shown["status"], "retired");
}

#[test]
fn approve_takes_a_bare_yes_only_after_a_reply_asking_to_approve_the_id() {
    let fx = store_with_agent_entry();
    let settle = |instruction: &'static str| {
        vec![
            "proposal",
            "settle",
            "p-1",
            "--approve",
            "--author",
            "opus",
            "--session",
            SESSION,
            "--instruction",
            instruction,
        ]
    };
    claude(
        &fx,
        Some("claude-desktop"),
        &[
            ("a-1", "assistant", "Approve p-1 and p-9?", 2),
            ("u-1", "user", "yes", 1),
        ],
    );
    // The check passes; settle then refuses as built today, since an
    // instruction does not yet widen what it accepts.
    assert_eq!(
        run(&fx, &settle("yes")),
        (Some(1), Some(String::from("daniel_only")))
    );

    claude(
        &fx,
        Some("claude-desktop"),
        &[
            ("a-1", "assistant", "Shall I look at p-1 again?", 2),
            ("u-1", "user", "yes", 1),
        ],
    );
    assert_eq!(
        run(&fx, &settle("yes")),
        (Some(1), Some(String::from("instruction_mismatch")))
    );
}

#[test]
fn only_cli_and_claude_desktop_entry_points_are_interactive() {
    let turns: &[Turn<'_>] = &[("u-1", "user", "retire preferences-2 please", 1)];
    for (entrypoint, passes) in [
        (Some("cli"), true),
        (Some("claude-desktop"), true),
        (Some("sdk-cli"), false),
        (Some("sdk-ts"), false),
        (None, false),
    ] {
        let fx = store_with_agent_entry();
        claude(&fx, entrypoint, turns);
        let before = state(&fx);
        let result = run(&fx, &retire("preferences-2", "retire preferences-2 please"));
        if passes {
            assert_eq!(result, (Some(0), None), "{entrypoint:?}");
        } else {
            assert_eq!(
                result,
                (Some(1), Some(String::from("not_interactive"))),
                "{entrypoint:?}"
            );
            assert_eq!(state(&fx), before, "{entrypoint:?} changes nothing");
        }
    }
}

#[test]
fn codex_cli_and_vscode_sessions_are_interactive_and_exec_is_not() {
    for (source, passes) in [("cli", true), ("vscode", true), ("exec", false)] {
        let fx = store_with_agent_entry();
        codex(&fx, source, &[("m-1", "retire preferences-2 please", 1)]);
        let before = state(&fx);
        let out = exec(
            &fx,
            &[
                "--provider",
                "codex",
                "entry",
                "retire",
                "preferences-2",
                "--author",
                "opus",
                "--reason",
                "Daniel asked",
                "--session",
                CODEX_SESSION,
                "--instruction",
                "retire preferences-2 please",
                "--json",
            ],
        );
        if passes {
            assert_eq!(
                out.status.code(),
                Some(0),
                "{source}: {}",
                text(&out.stderr)
            );
        } else {
            assert_eq!(out.status.code(), Some(1), "{source}");
            let err: Value = serde_json::from_slice(&out.stderr).unwrap();
            assert_eq!(err["code"], "not_interactive", "{source}: {err}");
            assert_eq!(state(&fx), before, "{source} changes nothing");
        }
    }
}

#[test]
fn a_negated_instruction_is_refused_and_a_plain_one_still_passes() {
    let fx = store_with_agent_entry();
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-1", "user", "don't retire preferences-2", 3),
            ("u-2", "user", "do not approve p-1", 2),
            ("u-3", "user", "retire preferences-2", 1),
        ],
    );
    let before = state(&fx);
    assert_eq!(
        run(&fx, &retire("preferences-2", "don't retire preferences-2")),
        (Some(1), Some(String::from("instruction_negated")))
    );
    assert_eq!(
        run(
            &fx,
            &[
                "proposal",
                "settle",
                "p-1",
                "--approve",
                "--author",
                "opus",
                "--session",
                SESSION,
                "--instruction",
                "do not approve p-1",
            ],
        ),
        (Some(1), Some(String::from("instruction_negated")))
    );
    assert_eq!(state(&fx), before, "a negated instruction changes nothing");
    assert!(
        ok(&fx, &retire("preferences-2", "retire preferences-2"))
            .status
            .success()
    );
}

#[test]
fn codex_scribe_lines_do_not_push_the_instruction_out_of_the_window() {
    let fx = store_with_agent_entry();
    codex(
        &fx,
        "cli",
        &[
            ("m-1", "retire preferences-2 please", 4),
            ("m-2", "/scribe on", 3),
            ("m-3", "second real turn", 2),
            ("m-4", "third real turn", 1),
        ],
    );
    let out = exec(
        &fx,
        &[
            "--provider",
            "codex",
            "entry",
            "retire",
            "preferences-2",
            "--author",
            "opus",
            "--reason",
            "Daniel asked",
            "--session",
            CODEX_SESSION,
            "--instruction",
            "retire preferences-2 please",
            "--json",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "control line counted: {}",
        text(&out.stderr)
    );
}
