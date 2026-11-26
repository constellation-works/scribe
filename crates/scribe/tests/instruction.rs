//! End-to-end tests of the instruction check (docs/design/scribe/2_design.md
//! §12.2, §12.5): `--session` and `--instruction` on `proposal settle` and
//! `entry retire`, given together or not at all, and each refusal code,
//! which changes nothing in the store; then what a passing check allows
//! (§12.1, §12.4): `entry retire` of Daniel's entry, and `entry forget`,
//! which leaves no store file holding the words forgotten.
//!
//! Every run is the built binary with a temp `HOME` and data root; the
//! transcripts are fixtures written relative to the real clock, since the
//! check measures an instruction's age against the time the command runs.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/instruction.rs"]
mod subject;
mod support;

use crate::support::command::Exit::{Capture, Success};
use crate::support::command::Input::Cli;
use serde_json::{Value, json};
use std::path::PathBuf;
use subject::*;
use support::{Fixture, make_private, text};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

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
        let out = fx.check(&args, Capture);
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
        (
            Some(
                i32::try_from(
                    support::session_id_outcome("instruction_missing")["exit"]
                        .as_i64()
                        .unwrap()
                )
                .unwrap()
            ),
            support::session_id_outcome("instruction_missing")["code"]
                .as_str()
                .map(str::to_owned)
        )
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
    let audit = support::audit_text(&fx.root());
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
    let out = fx.check(
        &retire("preferences-2", "retire it, preferences-2 is wrong"),
        Success,
    );
    assert!(out.status.success());
    let shown: Value = serde_json::from_slice(
        &fx.check(&["entry", "show", "preferences-2", "--json"], Success)
            .stdout,
    )
    .unwrap();
    assert_eq!(shown["status"], "retired");
}

#[test]
fn a_retry_answering_a_reply_that_echoed_a_refusal_code_passes() {
    // The skill has the agent echo a refusal's code; the code's `not` must
    // not refuse the human's corrected instruction that answers that reply.
    let fx = store_with_agent_entry();
    claude(
        &fx,
        Some("cli"),
        &[
            (
                "a-1",
                "assistant",
                "preferences-2: instruction_not_found. Name the id and say the act again.",
                1,
            ),
            ("u-1", "user", "retire preferences-2", 0),
        ],
    );
    let out = fx.check(&retire("preferences-2", "retire preferences-2"), Success);
    assert!(out.status.success());
    let shown: Value = serde_json::from_slice(
        &fx.check(&["entry", "show", "preferences-2", "--json"], Success)
            .stdout,
    )
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
        (Some(1), Some(String::from("user_only")))
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
        let out = fx.check(
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
            Capture,
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
fn a_group_writable_codex_exec_rollout_is_refused_before_its_source_is_classified() {
    use std::os::unix::fs::PermissionsExt;
    // Provenance is checked before the Codex source: the same `exec` rollout
    // is `instruction_not_found` while another user can write it, and
    // `not_interactive` once it is private to this user.
    let fx = store_with_agent_entry();
    let words = "retire preferences-2 please";
    let rollout = codex(&fx, "exec", &[("m-1", words, 1)]);
    let args = [
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
        words,
    ];
    std::fs::set_permissions(&rollout, std::fs::Permissions::from_mode(0o664)).unwrap();
    let before = state(&fx);
    let (code, err) = refusal(&fx, &args);
    assert_eq!(code, Some(1), "{err}");
    assert_eq!(err["code"], "instruction_not_found", "{err}");
    assert!(
        err["error"]
            .as_str()
            .unwrap()
            .contains("writable by another user"),
        "the provenance check refused it: {err}"
    );
    assert_eq!(state(&fx), before, "a refusal changes nothing");

    std::fs::set_permissions(&rollout, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (code, err) = refusal(&fx, &args);
    assert_eq!(code, Some(1), "{err}");
    assert_eq!(err["code"], "not_interactive", "{err}");
    assert_eq!(state(&fx), before, "a refusal changes nothing");
}

#[test]
fn negated_instruction_refuses_without_mutating_the_entry() {
    let fx = store_with_agent_entry();
    let text = "please don't retire preferences-2";
    claude(&fx, Some("cli"), &[("u-1", "user", text, 0)]);
    let before = state(&fx);
    assert_eq!(
        run(&fx, &retire("preferences-2", text)),
        (Some(1), Some(String::from("instruction_mismatch")))
    );
    assert_eq!(state(&fx), before);
}

#[test]
fn an_instruction_typed_this_instant_is_not_stale() {
    // The check runs on the sub-second clock: a whole-second `now` would put
    // a turn typed in the current second in the future and refuse it stale.
    for _ in 0..3 {
        let fx = store_with_agent_entry();
        let text = "retire preferences-2";
        claude(&fx, Some("cli"), &[("u-1", "user", text, 0)]);
        assert_eq!(
            run(&fx, &retire("preferences-2", text)),
            (Some(0), None),
            "a turn typed just now passes"
        );
    }
}

#[test]
fn instructed_retire_hides_daniels_entry_and_keeps_its_text() {
    let fx = Fixture::seeded();
    let words = "please retire preferences-1, it is outdated";
    claude(&fx, Some("claude-desktop"), &[("u-1", "user", words, 1)]);
    // Without the instruction his entry is still his to retire.
    assert_eq!(
        run(
            &fx,
            &[
                "entry",
                "retire",
                "preferences-1",
                "--author",
                "opus",
                "--reason",
                "Daniel asked"
            ]
        ),
        (Some(1), Some(String::from("user_only")))
    );
    let out = fx.check(&retire("preferences-1", words), Success);
    assert_eq!(text(&out.stderr).trim(), "", "it changed the entry");

    let shown: Value = serde_json::from_slice(
        &fx.check(&["entry", "show", "preferences-1", "--json"], Success)
            .stdout,
    )
    .unwrap();
    assert_eq!(shown["status"], "retired");
    assert_eq!(shown["retirement_reason"], "Daniel asked");
    assert_eq!(shown["content"], PREFERENCE, "retiring keeps the text");
    assert_eq!(shown["author"]["kind"], "user", "it stays his entry");
    assert_eq!(shown["updated_by"]["name"], "opus");
    assert_eq!(shown["removal_instruction"]["session"], SESSION);
    assert_eq!(shown["removal_instruction"]["message"], "u-1");
    assert_eq!(shown["removal_instruction"]["quote"], words);
    assert_eq!(shown["instruction"], Value::Null, "no agent approved it");

    let active: Value = serde_json::from_slice(
        &fx.check(&["entry", "list", "--status", "active", "--json"], Success)
            .stdout,
    )
    .unwrap();
    let ids: Vec<&str> = active["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&"preferences-1"), "{ids:?}");
    assert!(ids.contains(&"decisions-2"), "{ids:?}");

    let lines = audit_lines(&fx, "entry retire");
    assert_eq!(lines.len(), 2, "one refusal and one change: {lines:?}");
    assert_eq!(lines[0]["error_code"], "user_only");
    assert_eq!(lines[1]["status"], "success");
    assert_eq!(lines[1]["target"], "preferences-1");
    let audit = support::audit_text(&fx.root());
    assert!(
        !audit.contains(words),
        "the instruction's words are not audited"
    );
}

#[test]
fn forget_without_confirm_or_an_instruction_is_refused_and_changes_nothing() {
    let fx = Fixture::seeded();
    let words = "forget decisions-1 for good";
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-1", "user", "retire decisions-1 please", 2),
            ("u-2", "user", words, 1),
        ],
    );
    let before = state(&fx);
    let bare = ["entry", "forget", "decisions-1", "--author", "opus"];
    let cases: [(Vec<&str>, i32, &str); 4] = [
        // No instruction at all: the verb exists only with one.
        (bare.to_vec(), 2, "usage_error"),
        ([&bare[..], &["--confirm"]].concat(), 2, "usage_error"),
        // An instruction, but no --confirm.
        (
            [&bare[..], &["--session", SESSION, "--instruction", words]].concat(),
            1,
            "confirm_required",
        ),
        // Confirmed, but his words say retire, not forget.
        (
            forget("decisions-1", "retire decisions-1 please"),
            1,
            "instruction_mismatch",
        ),
    ];
    for (args, exit, code) in &cases {
        assert_eq!(
            run(&fx, args),
            (Some(*exit), Some((*code).to_owned())),
            "{args:?}"
        );
    }
    // Half an instruction is a parse error, as for every instructed verb.
    let out = fx.check(
        &[&bare[..], &["--session", SESSION, "--confirm"]].concat(),
        Capture,
    );
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(state(&fx), before, "every refusal changes nothing");

    let lines = audit_lines(&fx, "entry forget");
    assert_eq!(lines.len(), cases.len(), "one line per refusal: {lines:?}");
    for (line, (_, _, code)) in lines.iter().zip(&cases) {
        assert_eq!(line["status"], "failure", "{line}");
        assert_eq!(line["error_code"], *code, "{line}");
        assert_eq!(line["target"], "decisions-1", "{line}");
    }
    let audit = support::audit_text(&fx.root());
    assert!(
        !audit.contains(words),
        "the instruction's words are not audited"
    );
}

#[test]
fn forget_leaves_no_store_file_holding_the_entry_its_quotes_or_the_instruction() {
    let fx = Fixture::seeded();
    let retire_words = "please retire preferences-1 today";
    let forget_words = "now forget preferences-1 for good";
    let purge_words = "and purge decisions-1 as well";
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-1", "user", retire_words, 3),
            ("u-2", "user", forget_words, 2),
            ("u-3", "user", purge_words, 1),
        ],
    );
    // Retired first on his word, so the entry holds an instruction quote.
    fx.check(&retire("preferences-1", retire_words), Success);
    let reason = "The newer decision replaces merge commits";
    for held in [PREFERENCE, retire_words, DECISION, reason] {
        assert!(!files_holding(&fx, held).is_empty(), "{held}");
    }

    for (id, words, message) in [
        ("preferences-1", forget_words, "u-2"),
        ("decisions-1", purge_words, "u-3"),
    ] {
        let out = fx.check(&forget(id, words), Success);
        assert_eq!(text(&out.stderr).trim(), "", "{id} changed");
        // The file stays, keeping its id, tag, dates, author and the
        // instruction's ids.
        let shown: Value =
            serde_json::from_slice(&fx.check(&["entry", "show", id, "--json"], Success).stdout)
                .unwrap();
        assert_eq!(shown["id"], id);
        assert_eq!(shown["status"], "forgotten");
        assert_eq!(shown["content"], "");
        assert_eq!(shown["author"]["kind"], "user");
        assert_eq!(shown["updated_by"]["name"], "opus");
        assert_eq!(shown["removal_instruction"]["session"], SESSION);
        assert_eq!(shown["removal_instruction"]["message"], message);
        assert_eq!(shown["removal_instruction"]["quote"], Value::Null);
        assert_eq!(shown["instruction"], Value::Null, "no agent approved it");
    }

    for gone in [
        PREFERENCE,
        DECISION,
        retire_words,
        forget_words,
        purge_words,
        reason,
    ] {
        assert_eq!(files_holding(&fx, gone), Vec::<PathBuf>::new(), "{gone}");
    }
    assert!(support::exported(&fx.root(), "entries/preferences/preferences-1.md").is_some());
    assert!(support::exported(&fx.root(), "entries/decisions/decisions-1.md").is_some());
    // Another entry's quote on the same proposal stays.
    assert!(!files_holding(&fx, "squash-merge agent PRs").is_empty());

    // Again: nothing to change, so no audit line.
    let again = fx.check(&forget("preferences-1", forget_words), Success);
    assert_eq!(text(&again.stderr).trim(), "nothing changed");
    let lines = audit_lines(&fx, "entry forget");
    assert_eq!(lines.len(), 2, "{lines:?}");
    for (line, id) in lines.iter().zip(["preferences-1", "decisions-1"]) {
        assert_eq!(line["status"], "success", "{line}");
        assert_eq!(line["target"], id, "{line}");
        assert_eq!(line["author"], "opus", "{line}");
    }
}

#[test]
fn an_instruction_does_not_let_entry_update_change_daniels_content() {
    let fx = Fixture::seeded();
    let words = "remove preferences-1 and rewrite it";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    let before = state(&fx);
    let update = [
        "entry",
        "update",
        "preferences-1",
        "--author",
        "opus",
        "--content",
        "rewritten",
    ];
    assert_eq!(
        run(&fx, &update),
        (Some(1), Some(String::from("user_only")))
    );
    let out = fx.check(
        &[&update[..], &["--session", SESSION, "--instruction", words]].concat(),
        Capture,
    );
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(state(&fx), before);
    let lines = audit_lines(&fx, "entry update");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["error_code"], "user_only");
}

include!("instruction/approvals.rs");
include!("instruction/session_resolution.rs");
