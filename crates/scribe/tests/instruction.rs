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

mod support;

use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Output;
use support::{Fixture, make_private, snapshot, text};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
const CODEX_SESSION: &str = "11111111-2222-4333-8444-555555555555";
const STORED_SESSION: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

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
/// `(id, text, minutes before now)`, private to this user; returns its path.
fn codex(fx: &Fixture, source: &str, turns: &[(&str, &str, i64)]) -> PathBuf {
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
    path
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

/// What a refusal must leave alone: the store's files, but the database,
/// its WAL and the audit spool (every audited refusal inserts its line
/// there), and every exported record but the audit log, which stands for the
/// rows inside the database.
fn state(fx: &Fixture) -> State {
    let files = snapshot(&fx.root())
        .into_iter()
        .filter(|(path, _, _)| {
            let name = path.to_string_lossy();
            !name.contains("scribe.db") && !name.contains("audit-spool")
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("export");
    scribe_core::Store::open(fx.root())
        .export(&target, scribe_core::ExportFormat::Markdown)
        .unwrap();
    let records = snapshot(dir.path())
        .into_iter()
        .filter(|(path, _, _)| !path.ends_with("audit.jsonl"))
        .map(|(path, bytes, mode)| {
            (
                path.strip_prefix(dir.path()).unwrap().to_path_buf(),
                bytes,
                mode,
            )
        })
        .collect();
    State { files, records }
}

/// [`state`]'s result; a mismatch prints the files' names and sizes, not
/// their bytes.
#[derive(PartialEq)]
struct State {
    files: Vec<(PathBuf, Vec<u8>, u32)>,
    records: Vec<(PathBuf, Vec<u8>, u32)>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (kind, set) in [("file", &self.files), ("record", &self.records)] {
            for (path, bytes, mode) in set {
                let name = path.components().rev().take(3).collect::<Vec<_>>();
                let sum = bytes
                    .iter()
                    .fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(u64::from(*b)));
                writeln!(
                    f,
                    "{kind} {name:?} {} bytes sum {sum} mode {mode:o}",
                    bytes.len()
                )?;
            }
        }
        Ok(())
    }
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
    let out = ok(&fx, &retire("preferences-2", "retire preferences-2"));
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

/// Daniel's superseded entry `decisions-1` in the seeded store, word for
/// word, which the open proposal `p-1` quotes.
const DECISION: &str = "merge agent PRs with a merge commit";

/// Daniel's active entry `preferences-1` in the seeded store, word for word.
const PREFERENCE: &str = "short commit subjects, imperative mood";

/// `entry forget <id>` on `instruction` in [`SESSION`], with `--confirm`.
fn forget<'a>(id: &'a str, instruction: &'a str) -> Vec<&'a str> {
    vec![
        "entry",
        "forget",
        id,
        "--author",
        "opus",
        "--session",
        SESSION,
        "--instruction",
        instruction,
        "--confirm",
    ]
}

/// The audit log's lines for `command`.
fn audit_lines(fx: &Fixture, command: &str) -> Vec<Value> {
    support::audit_text(&fx.root())
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|l| l["command"] == command)
        .collect()
}

/// Every file under the store root (`pre-sqlite-*/` included), and every
/// exported record, whose bytes hold `needle`.
fn files_holding(fx: &Fixture, needle: &str) -> Vec<PathBuf> {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("export");
    scribe_core::Store::open(fx.root())
        .export(&target, scribe_core::ExportFormat::Markdown)
        .unwrap();
    snapshot(&fx.root())
        .into_iter()
        .chain(snapshot(dir.path()))
        .filter(|(_, bytes, _)| {
            bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        })
        .map(|(path, _, _)| path)
        .collect()
}

#[test]
fn instructed_retire_hides_daniels_entry_and_keeps_its_text() {
    let fx = Fixture::new();
    fx.seed();
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
    let out = ok(&fx, &retire("preferences-1", words));
    assert_eq!(text(&out.stderr).trim(), "", "it changed the entry");

    let shown: Value =
        serde_json::from_slice(&ok(&fx, &["entry", "show", "preferences-1", "--json"]).stdout)
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

    let active: Value =
        serde_json::from_slice(&ok(&fx, &["entry", "list", "--status", "active", "--json"]).stdout)
            .unwrap();
    let ids: Vec<&str> = active["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&"preferences-1"), "{ids:?}");
    assert!(ids.contains(&"decisions-2"), "{ids:?}");

    // `ask` hands the model the active decision, never the retired one.
    let claude = "#!/bin/sh\n\
        cat > \"$HOME/claude-stdin\"\n\
        printf '{\"type\":\"result\",\"is_error\":false,\"result\":\"\",\"structured_output\":{\"status\":\"no_record\",\"answer\":\"\",\"claims\":[],\"conflicts\":[]}}\\n'\n";
    let path = fx.fake_program("claude", claude);
    let out = fx
        .cmd(&[
            "question",
            "ask",
            "how should commit subjects read and how do we merge agent PRs",
            "--json",
        ])
        .env("GITHUB_TOKEN", "synthetic-fixture-token-value")
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let prompt = std::fs::read_to_string(fx.home().join("claude-stdin")).unwrap();
    assert!(prompt.contains("squash-merge agent PRs"), "{prompt}");
    assert!(!prompt.contains(PREFERENCE), "{prompt}");

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
    let fx = Fixture::new();
    fx.seed();
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
    let out = exec(
        &fx,
        &[&bare[..], &["--session", SESSION, "--confirm"]].concat(),
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
    let fx = Fixture::new();
    fx.seed();
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
    ok(&fx, &retire("preferences-1", retire_words));
    let reason = "The newer decision replaces merge commits";
    for held in [PREFERENCE, retire_words, DECISION, reason] {
        assert!(!files_holding(&fx, held).is_empty(), "{held}");
    }

    for (id, words, message) in [
        ("preferences-1", forget_words, "u-2"),
        ("decisions-1", purge_words, "u-3"),
    ] {
        let out = ok(&fx, &forget(id, words));
        assert_eq!(text(&out.stderr).trim(), "", "{id} changed");
        // The file stays, keeping its id, tag, dates, author and the
        // instruction's ids.
        let shown: Value =
            serde_json::from_slice(&ok(&fx, &["entry", "show", id, "--json"]).stdout).unwrap();
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
    let again = ok(&fx, &forget("preferences-1", forget_words));
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
    let fx = Fixture::new();
    fx.seed();
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
    let out = exec(
        &fx,
        &[&update[..], &["--session", SESSION, "--instruction", words]].concat(),
    );
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(state(&fx), before);
    let lines = audit_lines(&fx, "entry update");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["error_code"], "user_only");
}

/// Daniel's claim the session pass opened as `p-1` (reserving
/// `preferences-1`): scribe's statement of his turn, his words in `quote`.
const CLAIM_QUOTE: &str = "I prefer reviews that name the file and line.";
const CLAIM_STATEMENT: &str = "Daniel prefers reviews that name the file and line.";

/// An assistant turn's claim, `p-2` (reserving `preferences-2`).
const ASSISTANT_QUOTE: &str = "Run the goldens after every help change.";

/// `proposal settle <id>` with `act` on `instruction` in [`SESSION`].
fn settle<'a>(id: &'a str, act: &'a str, instruction: &'a str) -> Vec<&'a str> {
    vec![
        "proposal",
        "settle",
        id,
        act,
        "--author",
        "opus",
        "--session",
        SESSION,
        "--instruction",
        instruction,
    ]
}

/// A store holding only two open session-pass candidates: `p-1` from
/// Daniel's turn and `p-2` from an assistant's, in another session.
fn store_with_candidates() -> Fixture {
    store_with_candidates_opened_at("2026-11-15T00:01:00Z")
}

/// [`store_with_candidates`], both claims opened at `opened_at`.
fn store_with_candidates_opened_at(opened_at: &str) -> Fixture {
    let fx = Fixture::new();
    let root = fx.root();
    std::fs::create_dir_all(&root).unwrap();
    make_private(&root);
    let cite = |message: &str| {
        json!({"session": "s-1", "message": message, "at": "2026-11-15T00:00:00Z",
            "prompt_sha256": "0".repeat(64), "line": 1, "cwd": "/work/scribe"})
    };
    let candidate = |id: &str, older: &str, role: &str, quote: &str, statement: Option<&str>| {
        json!({
            "id": id, "kind": "candidate", "older": older, "newer": null,
            "candidate": {
                "tag": "preferences", "role": role, "quote": quote,
                "statement": statement,
                "subject": statement.map(|_| "code reviews"),
                "evidence": null, "cite": cite(&format!("turn-{id}")),
                "project": "scribe", "provider": "claude-code", "host": "host",
                "admitted": null
            },
            "reason": format!("Unmarked preferences statement: {quote}"), "quotes": [],
            "state": "open", "opened_at": opened_at, "settled_at": null,
            "settled_by": null, "author": {"kind": "scribe", "name": "session-pass"},
            "settled_author": null, "updated_by": null, "closed_reason": null,
            "instruction": null
        })
    };
    let path = root.join("proposals.json");
    std::fs::write(
        &path,
        json!({"format": 14, "proposals": [
            candidate("p-1", "preferences-1", "user", CLAIM_QUOTE, Some(CLAIM_STATEMENT)),
            candidate("p-2", "preferences-2", "assistant", ASSISTANT_QUOTE, None),
        ]})
        .to_string(),
    )
    .unwrap();
    make_private(&path);
    fx
}

/// `<noun> show <id> --json`.
fn show(fx: &Fixture, noun: &str, id: &str) -> Value {
    if noun == "proposal" {
        let listed: Value =
            serde_json::from_slice(&ok(fx, &["proposal", "list", "--json"]).stdout).unwrap();
        return listed["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .cloned()
            .unwrap();
    }
    serde_json::from_slice(&ok(fx, &[noun, "show", id, "--json"]).stdout).unwrap()
}

#[test]
fn an_instructed_approval_writes_daniels_reserved_entry_with_the_agent_as_approver() {
    let fx = store_with_candidates();
    let words = "approve p-1, that is how I want reviews";
    claude(
        &fx,
        Some("claude-desktop"),
        &[
            ("a-1", "assistant", "p-1 is open: approve it?", 2),
            ("u-1", "user", words, 1),
        ],
    );
    // Without an instruction a claim stays his to settle.
    let before = state(&fx);
    assert_eq!(
        run(
            &fx,
            &["proposal", "settle", "p-1", "--approve", "--author", "opus"]
        ),
        (Some(1), Some(String::from("user_only")))
    );
    assert_eq!(state(&fx), before);

    let out = ok(&fx, &settle("p-1", "--approve", words));
    assert_eq!(text(&out.stderr).trim(), "", "it settled the claim");
    let p = show(&fx, "proposal", "p-1");
    assert_eq!(p["state"], "approved");
    assert_eq!(
        p["settled_author"],
        json!({"kind": "agent", "name": "opus"})
    );
    assert_eq!(p["settled_by"]["session"], SESSION);
    assert_eq!(p["settled_by"]["message"], "u-1");
    assert_eq!(p["instruction"]["session"], SESSION);
    assert_eq!(p["instruction"]["message"], "u-1");
    assert_eq!(p["instruction"]["quote"], words);
    assert_eq!(p["closed_reason"], Value::Null);

    // His entry, under the id the claim reserved: his cite and words.
    let e = show(&fx, "entry", "preferences-1");
    assert_eq!(e["author"], json!({"kind": "user", "name": "user"}));
    assert_eq!(e["content"], CLAIM_STATEMENT);
    assert_eq!(e["quote"], CLAIM_QUOTE);
    assert_eq!(e["cite"]["session"], "s-1");
    assert_eq!(e["cite"]["message"], "turn-p-1");
    assert_eq!(e["content_from"]["kind"], "message");
    assert_eq!(e["content_from"]["role"], "user");
    assert_eq!(e["approved_by"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(e["instruction"], p["instruction"]);
    let said = format!(
        "scribe proposal settle p-1 --approve --author opus --session {SESSION} --instruction '{words}'"
    );
    assert_eq!(e["said"], said.as_str());

    // The same command again changes nothing; another settlement is refused.
    let again = ok(&fx, &settle("p-1", "--approve", words));
    assert!(
        text(&again.stderr).contains("nothing changed"),
        "{}",
        text(&again.stderr)
    );
    assert_eq!(
        run(&fx, &settle("p-1", "--reject", "reject p-1 after all")),
        (Some(1), Some(String::from("instruction_not_found")))
    );

    // One audit line per change or refusal: agent, target and the
    // instruction's ids, never its words.
    let lines = audit_lines(&fx, "proposal settle");
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0]["error_code"], "user_only");
    assert_eq!(lines[0]["instruction_session"], Value::Null);
    assert_eq!(lines[1]["status"], "success");
    assert_eq!(lines[2]["error_code"], "instruction_not_found");
    assert_eq!(lines[2]["instruction_message"], Value::Null);
    for line in &lines {
        assert_eq!(line["author"], "opus");
        assert_eq!(line["target"], "p-1");
    }
    assert_eq!(lines[1]["instruction_session"], SESSION);
    assert_eq!(lines[1]["instruction_message"], "u-1");
    assert_eq!(lines[2]["instruction_session"], SESSION);
    let audit = support::audit_text(&fx.root());
    assert!(!audit.contains("how I want reviews"), "{audit}");
}

#[test]
fn an_instructed_approval_of_an_assistant_claim_writes_one_agent_entry() {
    let fx = store_with_candidates();
    let words = "accept p-2, keep that one";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    ok(&fx, &settle("p-2", "--approve", words));
    let e = show(&fx, "entry", "preferences-2");
    assert_eq!(e["author"], json!({"kind": "agent", "name": "assistant"}));
    assert_eq!(e["content"], ASSISTANT_QUOTE);
    assert_eq!(e["content_from"]["role"], "assistant");
    assert_eq!(e["approved_by"], json!({"kind": "agent", "name": "opus"}));
    assert!(
        e["said"]
            .as_str()
            .unwrap()
            .starts_with("scribe proposal settle p-2 --approve --author opus"),
        "{e}"
    );
    // It rereads from disk in every listing.
    let listed: Value =
        serde_json::from_slice(&ok(&fx, &["entry", "list", "--json"]).stdout).unwrap();
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["warnings"], json!([]), "{listed}");
}

#[test]
fn an_instructed_rejection_writes_no_entry_and_closes_the_claim_instructed() {
    let fx = store_with_candidates();
    let words = "reject p-1 and p-2, neither is a rule";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    for id in ["p-1", "p-2"] {
        ok(&fx, &settle(id, "--reject", words));
        let p = show(&fx, "proposal", id);
        assert_eq!(p["state"], "rejected");
        assert_eq!(p["closed_reason"], "instructed");
        assert_eq!(p["settled_author"]["name"], "opus");
        assert_eq!(p["instruction"]["message"], "u-1");
        assert_eq!(p["instruction"]["quote"], words);
    }
    assert!(!fx.root().join("entries").exists(), "no entry is written");
    // An approval after it is refused: the rejection stands.
    let approve = "approve p-1 after all";
    claude(
        &fx,
        Some("cli"),
        &[("u-1", "user", words, 2), ("u-2", "user", approve, 1)],
    );
    assert_eq!(
        run(&fx, &settle("p-1", "--approve", approve)),
        (Some(1), Some(String::from("proposal_settled")))
    );
    let lines = audit_lines(&fx, "proposal settle");
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[2]["error_code"], "proposal_settled");
    assert_eq!(lines[2]["instruction_message"], "u-2");
}

#[test]
fn an_instruction_does_not_settle_a_proposal_about_daniels_entry() {
    let fx = Fixture::new();
    fx.seed();
    // The seeded `p-1` proposes that Daniel's `decisions-2` supersede his
    // `decisions-1`.
    let words = "approve p-1 please";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    let before = state(&fx);
    for act in ["--approve", "--reject"] {
        let words = if act == "--approve" {
            words
        } else {
            "reject p-1 please"
        };
        claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
        assert_eq!(
            run(&fx, &settle("p-1", act, words)),
            (Some(1), Some(String::from("user_only"))),
            "{act}"
        );
    }
    assert_eq!(state(&fx), before);
    let lines = audit_lines(&fx, "proposal settle");
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert_eq!(line["error_code"], "user_only");
        assert_eq!(line["target"], "p-1");
        assert_eq!(line["instruction_session"], SESSION);
        assert_eq!(line["instruction_message"], "u-1");
    }
}

#[test]
fn a_scan_replays_an_interrupted_instructed_approval_with_its_attribution() {
    let fx = store_with_candidates();
    let words = "approve p-1";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    ok(&fx, &settle("p-1", "--approve", words));
    let entry = "entries/preferences/preferences-1.md";
    let written = support::exported(&fx.root(), entry).unwrap();
    // A crash between the claim and the entry leaves the approval.
    support::sql(&fx.root(), "DELETE FROM entries WHERE id = 'preferences-1'");
    let out = ok(&fx, &["consistency", "scan", "--json"]);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["reapplied"][0]["entry"], "preferences-1", "{report}");
    let e = show(&fx, "entry", "preferences-1");
    assert_eq!(e["approved_by"]["name"], "opus");
    assert_eq!(e["instruction"]["message"], "u-1");
    assert!(
        e["said"]
            .as_str()
            .unwrap()
            .starts_with("scribe proposal settle p-1 --approve"),
        "never a typed #scribe approve: {e}"
    );
    assert_eq!(
        support::exported(&fx.root(), entry).unwrap(),
        written,
        "the same entry"
    );
    // A typed repeat changes nothing either.
    let p = show(&fx, "proposal", "p-1");
    assert_eq!(p["state"], "approved");
}

#[test]
fn a_scan_closes_an_instructed_approval_covered_since_as_covered_not_rejected() {
    let fx = store_with_candidates();
    let words = "approve p-1";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    ok(&fx, &settle("p-1", "--approve", words));
    support::sql(&fx.root(), "DELETE FROM entries WHERE id = 'preferences-1'");
    // An active entry now says the same.
    let added: Value = serde_json::from_slice(
        &ok(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--project",
                "scribe",
                "--author",
                "opus",
                CLAIM_STATEMENT,
                "--json",
            ],
        )
        .stdout,
    )
    .unwrap();
    let out = ok(&fx, &["consistency", "scan", "--json"]);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["covered"][0]["proposal"], "p-1", "{report}");
    assert_eq!(report["covered"][0]["by"], added["id"], "{report}");
    // The store still reads, and the approval and instruction are kept.
    let p = show(&fx, "proposal", "p-1");
    assert_eq!(p["state"], "rejected");
    assert_eq!(p["closed_reason"], "covered");
    assert_eq!(p["settled_author"]["name"], "opus");
    assert_eq!(p["instruction"]["message"], "u-1");
    assert_eq!(p["instruction"]["quote"], words);
    assert!(support::exported(&fx.root(), "entries/preferences/preferences-1.md").is_none());
    // The same instruction again is done; nothing changes.
    let again = ok(&fx, &settle("p-1", "--approve", words));
    assert!(
        text(&again.stderr).contains("nothing changed"),
        "{}",
        text(&again.stderr)
    );
}

/// The error code and message of a `--json` refusal.
fn refusal(fx: &Fixture, args: &[&str]) -> (Option<i32>, Value) {
    let mut args = args.to_vec();
    args.push("--json");
    let out = exec(fx, &args);
    (
        out.status.code(),
        serde_json::from_slice(&out.stderr).unwrap(),
    )
}

#[test]
fn without_an_instruction_a_claim_is_refused_with_the_wording_it_always_had() {
    let fx = store_with_candidates();
    for act in ["--approve", "--reject"] {
        let (code, err) = refusal(&fx, &["proposal", "settle", "p-1", act, "--author", "opus"]);
        assert_eq!(code, Some(1), "{act}");
        assert_eq!(err["code"], "user_only", "{act}");
        assert_eq!(
            err["error"], "The user settles candidate proposals through #scribe approve or reject",
            "{act}"
        );
    }
}

#[test]
fn an_instruction_turn_without_a_message_id_is_refused_before_any_write() {
    for uuid in ["", "  "] {
        let fx = store_with_candidates();
        let words = "approve p-1, that is how I want reviews";
        claude(
            &fx,
            Some("claude-desktop"),
            &[
                ("a-1", "assistant", "p-1 is open: approve it?", 2),
                (uuid, "user", words, 1),
            ],
        );
        let before = state(&fx);
        assert_eq!(
            run(&fx, &settle("p-1", "--approve", words)),
            (Some(1), Some(String::from("instruction_not_found"))),
            "{uuid:?}"
        );
        assert_eq!(state(&fx), before, "nothing is written");
        // The store still reads.
        assert_eq!(show(&fx, "proposal", "p-1")["state"], "open");
        let lines = audit_lines(&fx, "proposal settle");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["instruction_message"], Value::Null);
    }
}

#[test]
fn an_instruction_typed_before_a_claim_opened_cannot_settle_it() {
    // The claims opened a minute ago; his instruction is three minutes old.
    let opened = (OffsetDateTime::now_utc() - time::Duration::minutes(1))
        .format(&Rfc3339)
        .unwrap();
    let fx = store_with_candidates_opened_at(&opened);
    let approve = "approve p-1, that is how I want reviews";
    let reject = "reject p-2, it is noise";
    claude(
        &fx,
        Some("cli"),
        &[("u-1", "user", approve, 3), ("u-2", "user", reject, 3)],
    );
    let before = state(&fx);
    assert_eq!(
        run(&fx, &settle("p-1", "--approve", approve)),
        (Some(1), Some(String::from("instruction_stale")))
    );
    assert_eq!(
        run(&fx, &settle("p-2", "--reject", reject)),
        (Some(1), Some(String::from("instruction_stale")))
    );
    assert_eq!(state(&fx), before, "neither claim changed");
    assert_eq!(show(&fx, "proposal", "p-1")["state"], "open");
    assert_eq!(show(&fx, "proposal", "p-2")["state"], "open");
    // Typed after it opened, the same words settle it.
    claude(&fx, Some("cli"), &[("u-3", "user", approve, 0)]);
    ok(&fx, &settle("p-1", "--approve", approve));
    assert_eq!(show(&fx, "proposal", "p-1")["state"], "approved");
}

#[test]
fn a_refusal_after_the_instruction_turn_is_found_audits_its_message_id() {
    let fx = store_with_candidates();
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-1", "user", "approve p-1, that is how I want reviews", 50),
            ("u-2", "user", "approve p-9, a different claim", 2),
            ("u-3", "user", "anything else for now", 1),
        ],
    );
    // Found but stale, found but naming another claim, and never found.
    let cases = [
        (
            "approve p-1, that is how",
            "instruction_stale",
            json!("u-1"),
        ),
        (
            "approve p-9, a different",
            "instruction_mismatch",
            json!("u-2"),
        ),
        (
            "never typed by anyone",
            "instruction_not_found",
            Value::Null,
        ),
    ];
    for (words, code, _) in &cases {
        assert_eq!(
            run(&fx, &settle("p-1", "--approve", words)),
            (Some(1), Some(String::from(*code))),
            "{words}"
        );
    }
    let lines = audit_lines(&fx, "proposal settle");
    assert_eq!(lines.len(), cases.len(), "{lines:?}");
    for (line, (_, code, message)) in lines.iter().zip(&cases) {
        assert_eq!(line["error_code"], *code);
        assert_eq!(line["instruction_session"], SESSION);
        assert_eq!(line["instruction_message"], *message, "{code}");
    }
}

#[test]
fn retiring_an_instructed_approval_keeps_the_approval_apart_from_the_retirement() {
    let fx = store_with_candidates();
    let approve = "approve p-1, that is how I want reviews";
    claude(&fx, Some("cli"), &[("u-1", "user", approve, 2)]);
    ok(&fx, &settle("p-1", "--approve", approve));
    let approved = show(&fx, "entry", "preferences-1");
    let retire_words = "please retire preferences-1 now";
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-1", "user", approve, 2),
            ("u-2", "user", retire_words, 1),
        ],
    );
    ok(&fx, &retire("preferences-1", retire_words));
    let e = show(&fx, "entry", "preferences-1");
    assert_eq!(e["status"], "retired");
    // The approval: who, the command and the instruction it was run on.
    assert_eq!(e["approved_by"], approved["approved_by"]);
    assert_eq!(e["said"], approved["said"]);
    assert_eq!(e["instruction"], approved["instruction"]);
    assert_eq!(e["instruction"]["message"], "u-1");
    assert_eq!(e["instruction"]["quote"], approve);
    // The retirement, on its own.
    assert_eq!(e["removal_instruction"]["message"], "u-2");
    assert_eq!(e["removal_instruction"]["quote"], retire_words);
    // Forgetting it later blanks both quotes and keeps both ids apart.
    let forget_words = "forget preferences-1 for good";
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-2", "user", retire_words, 2),
            ("u-3", "user", forget_words, 1),
        ],
    );
    ok(&fx, &forget("preferences-1", forget_words));
    let e = show(&fx, "entry", "preferences-1");
    assert_eq!(e["status"], "forgotten");
    assert_eq!(e["instruction"]["message"], "u-1");
    assert_eq!(e["instruction"]["quote"], Value::Null);
    assert_eq!(e["removal_instruction"]["message"], "u-3");
    assert_eq!(e["removal_instruction"]["quote"], Value::Null);
}

#[test]
fn a_multiline_instruction_cites_the_line_its_quote_starts_on() {
    let fx = store_with_candidates();
    // The quote's first line also appears on line 1, but the quote as a
    // whole starts on line 3.
    let turn =
        "approve p-1, that is\nwhat I said earlier\napprove p-1, that is\nhow I want reviews";
    let words = "approve p-1, that is\nhow I want reviews";
    claude(&fx, Some("cli"), &[("u-1", "user", turn, 1)]);
    ok(&fx, &settle("p-1", "--approve", words));
    let p = show(&fx, "proposal", "p-1");
    assert_eq!(p["settled_by"]["line"], 3, "{p}");
}

fn write_desktop_transcript(fx: &Fixture, lines: &[(&str, &str, Value)]) {
    let project = fx.home().join(".claude/projects/-work-scribe");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{SESSION}.jsonl"));
    let at = (OffsetDateTime::now_utc() - time::Duration::minutes(1))
        .format(&Rfc3339)
        .unwrap();
    let mut out = String::new();
    for (uuid, body, extra) in lines {
        let mut line = json!({
            "type": "user",
            "uuid": uuid,
            "sessionId": SESSION,
            "timestamp": at,
            "cwd": "/work/scribe",
            "entrypoint": "claude-desktop",
            "message": {"role": "user", "content": body}
        });
        if let (Some(obj), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        out.push_str(&format!("{line}\n"));
    }
    std::fs::write(&path, out).unwrap();
    make_private(&path);
}

#[test]
fn claude_desktop_human_turn_is_accepted_and_non_human_desktop_turns_are_refused() {
    let human_extra = json!({
        "origin": {"kind": "human"},
        "promptSource": "sdk",
        "turnOrigin": "human"
    });
    let peer_extra = json!({
        "origin": {
            "kind": "peer",
            "from": "local_1",
            "name": "peer",
            "body": "approve p-1",
            "hostInjected": true
        },
        "promptSource": "system",
        "turnOrigin": "peer",
        "isMeta": true
    });
    let task_extra = json!({
        "origin": {
            "kind": "task-notification",
            "producer": "session-task",
            "runId": "r-1"
        },
        "promptSource": "system",
        "turnOrigin": "task_notification"
    });
    let synth_extra = json!({
        "promptSource": "sdk",
        "turnOrigin": "host_synthetic"
    });

    // `proposal settle --instruction` accepts human desktop turn and refuses all three non-human turns.
    for (name, extra) in [
        ("peer", &peer_extra),
        ("task_notification", &task_extra),
        ("host_synthetic", &synth_extra),
    ] {
        let fx = store_with_candidates();
        let words = "approve p-1, that is how I want reviews";
        write_desktop_transcript(&fx, &[("u-1", words, extra.clone())]);
        assert_eq!(
            run(&fx, &settle("p-1", "--approve", words)),
            (Some(1), Some(String::from("instruction_not_found"))),
            "settle must refuse {name}"
        );
    }

    let fx = store_with_candidates();
    let approve = "approve p-1, that is how I want reviews";
    write_desktop_transcript(&fx, &[("u-1", approve, human_extra.clone())]);
    ok(&fx, &settle("p-1", "--approve", approve));
    assert_eq!(show(&fx, "proposal", "p-1")["state"], "approved");

    // `entry retire --instruction` and `entry forget --instruction` accept the same desktop shape.
    let retire_words = "please retire preferences-1, it is outdated";
    let forget_words = "forget preferences-1 for good";
    write_desktop_transcript(
        &fx,
        &[
            ("u-2", retire_words, human_extra.clone()),
            ("u-3", forget_words, human_extra),
        ],
    );
    ok(&fx, &retire("preferences-1", retire_words));
    assert_eq!(show(&fx, "entry", "preferences-1")["status"], "retired");
    ok(&fx, &forget("preferences-1", forget_words));
    assert_eq!(show(&fx, "entry", "preferences-1")["status"], "forgotten");
}

#[test]
fn store_session_resolution_survives_unreadable_claude_transcript_directory() {
    use std::os::unix::fs::PermissionsExt;

    let fx = store_with_agent_entry();
    let memory_id = "2099-01-01-session-resolution";
    let memory_dir = fx.root().join("memory/now");
    fx.create_private_dir_all(&memory_dir);
    let memory_path = memory_dir.join(format!("{memory_id}.md"));
    std::fs::write(
        &memory_path,
        format!(
            "---\ndate: 2099-01-01\nkind: plan\nsources:\n- {STORED_SESSION}\nupdated: 2026-11-22T00:00:00Z\n---\n# Session resolution\n\nKeep this item for the CLI regression test.\n"
        ),
    )
    .unwrap();
    make_private(&memory_path);

    let locked = fx.home().join(".claude/projects/locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    struct RestorePermissions(PathBuf);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _restore = RestorePermissions(locked);

    let related = format!("session:{STORED_SESSION}");
    fx.ok(&["entry", "list", "--related", &related]);
    ok(
        &fx,
        &[
            "memory",
            "ack",
            memory_id,
            "--session",
            STORED_SESSION,
            "--author",
            "opus",
        ],
    );
}

#[test]
fn uncataloged_transcript_prefix_from_transcript_list_works_for_instruction_writes() {
    let fx = store_with_agent_entry();
    let listed_sessions = serde_json::from_slice::<Value>(
        &ok(&fx, &["session", "list", "--include-inactive", "--json"]).stdout,
    )
    .unwrap();
    assert!(
        !listed_sessions["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == SESSION),
        "test requires SESSION to have no store record"
    );

    let retire_words = "retire preferences-2 please";
    let forget_words = "forget preferences-1 for good";
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-1", "user", retire_words, 2),
            ("u-2", "user", forget_words, 1),
        ],
    );

    // `transcript list` prints the 8-char prefix `5d1c0b7e`; it works like the full UUID.
    let since = (OffsetDateTime::now_utc() - time::Duration::days(1))
        .date()
        .to_string();
    let listed = text(
        &ok(
            &fx,
            &[
                "transcript",
                "list",
                "--since",
                &since,
                "--min-turns",
                "1",
                "--format",
                "table",
            ],
        )
        .stdout,
    );
    let prefix = &SESSION[..8];
    assert!(listed.contains(prefix), "{listed}");
    assert!(!listed.contains(SESSION), "{listed}");

    ok(
        &fx,
        &[
            "entry",
            "retire",
            "preferences-2",
            "--author",
            "opus",
            "--reason",
            "Daniel asked",
            "--session",
            prefix,
            "--instruction",
            retire_words,
        ],
    );
    let retired = show(&fx, "entry", "preferences-2");
    assert_eq!(retired["status"], "retired");
    let expected = support::session_id_outcome("instruction_prefix");
    assert_eq!(expected["exit"], 0); // ok observed exit 0
    assert!(expected["code"].is_null());
    assert_eq!(retired["removal_instruction"]["session"], expected["value"]);
    assert_eq!(
        audit_lines(&fx, "entry retire").last().unwrap()["instruction_session"],
        SESSION
    );

    // A second uncataloged transcript sharing the 8-char prefix makes `5d1c0b7e` ambiguous.
    let sibling = "5d1c0b7e-ffff-4c39-8e61-0a9b8c7d6e5f";
    let sibling_path = fx
        .home()
        .join(".claude/projects/-work-scribe")
        .join(format!("{sibling}.jsonl"));
    let now_ts = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    std::fs::write(
        &sibling_path,
        format!(
            "{}\n",
            json!({
                "type": "user",
                "uuid": "s-u1",
                "sessionId": sibling,
                "timestamp": now_ts,
                "cwd": "/work/scribe",
                "entrypoint": "cli",
                "message": {"role": "user", "content": "forget preferences-1 for good"}
            })
        ),
    )
    .unwrap();
    make_private(&sibling_path);

    let disambiguated = text(
        &ok(
            &fx,
            &[
                "transcript",
                "list",
                "--since",
                &since,
                "--min-turns",
                "1",
                "--format",
                "table",
            ],
        )
        .stdout,
    );
    assert!(disambiguated.contains("5d1c0b7e-2 "), "{disambiguated}");
    assert!(disambiguated.contains("5d1c0b7e-f "), "{disambiguated}");

    let before = state(&fx);
    let (exit_code, err_code) = run(
        &fx,
        &[
            "entry",
            "forget",
            "preferences-1",
            "--author",
            "opus",
            "--session",
            prefix,
            "--instruction",
            forget_words,
            "--confirm",
        ],
    );
    let expected = support::session_id_outcome("instruction_ambiguous");
    assert_eq!(
        exit_code,
        expected["exit"].as_i64().map(|n| i32::try_from(n).unwrap())
    );
    assert_eq!(err_code.as_deref(), expected["code"].as_str());
    assert_eq!(state(&fx), before, "ambiguous --session changes nothing");

    // Passing the disambiguated prefix `5d1c0b7e-2` succeeds and cites the full UUID.
    ok(
        &fx,
        &[
            "entry",
            "forget",
            "preferences-1",
            "--author",
            "opus",
            "--session",
            "5d1c0b7e-2",
            "--instruction",
            forget_words,
            "--confirm",
        ],
    );
    let forgotten = show(&fx, "entry", "preferences-1");
    assert_eq!(forgotten["status"], "forgotten");
    assert_eq!(forgotten["removal_instruction"]["session"], SESSION);

    // `proposal settle` also resolves an uncataloged transcript prefix and cites the full UUID.
    let cand_fx = store_with_candidates();
    let approve_words = "approve p-1, that is how I want reviews";
    claude(&cand_fx, Some("cli"), &[("u-1", "user", approve_words, 1)]);
    ok(
        &cand_fx,
        &[
            "proposal",
            "settle",
            "p-1",
            "--approve",
            "--author",
            "opus",
            "--session",
            prefix,
            "--instruction",
            approve_words,
        ],
    );
    let settled = show(&cand_fx, "proposal", "p-1");
    assert_eq!(settled["state"], "approved");
    assert_eq!(settled["settled_by"]["session"], SESSION);
}

#[test]
fn codex_uncataloged_transcript_prefix_works_for_instruction_writes() {
    let fx = store_with_agent_entry();
    let words = "retire preferences-2 please";
    codex(&fx, "cli", &[("m-1", words, 1)]);
    let since = (OffsetDateTime::now_utc() - time::Duration::days(1))
        .date()
        .to_string();
    let listed = text(
        &ok(
            &fx,
            &[
                "--provider",
                "codex",
                "transcript",
                "list",
                "--since",
                &since,
                "--min-turns",
                "1",
                "--format",
                "table",
            ],
        )
        .stdout,
    );
    let prefix = &CODEX_SESSION[..8];
    assert!(listed.contains(prefix), "{listed}");

    ok(
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
            prefix,
            "--instruction",
            words,
        ],
    );
    let retired = show(&fx, "entry", "preferences-2");
    assert_eq!(retired["status"], "retired");
    assert_eq!(retired["removal_instruction"]["session"], CODEX_SESSION);
}
