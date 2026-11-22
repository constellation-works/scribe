//! End-to-end tests of agent-written session handoffs
//! (docs/design/scribe/2_design.md §16): the shipped `handoff` tag,
//! verbatim quote check against a human-typed turn in the target session,
//! fixed six headings in order and size limit, off/unrecorded sessions,
//! `session show` and `brief` rendering, `#scribe retire`, `search` and
//! `question ask` agent-note attribution, and pass isolation (no memory
//! item or proposed claim from handoff text).
//!
//! Every run is the built binary with a temp `HOME` and data root.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use support::{Fixture, make_private, text};

const OFF_SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
const ON_SESSION: &str = "6e2d1c8f-3a5b-4d40-9f72-1b0c9d8e7f60";

fn valid_handoff(state: &str, next: &str) -> String {
    format!(
        "\
## State
{state}

## In flight
Uncommitted edits in `crates/scribe-core/src/handoff.rs`.

## Next
{next}

## Waiting on you
Confirm whether we cut the release tag today.

## Do not touch
Leave `crates/scribe/tests/goldens/` untouched until the contract review passes.

## Pointers
- `crates/scribe-core/src/handoff.rs`
- `docs/design/scribe/2_design.md`
"
    )
}

/// A Claude Code transcript layout under the fixture's home.
struct Transcript {
    session: String,
    path: PathBuf,
    cwd: PathBuf,
}

impl Transcript {
    fn new(fx: &Fixture, session: &str) -> Self {
        let project = fx.home().join(".claude/projects/-work-scribe");
        std::fs::create_dir_all(&project).unwrap();
        let cwd = fx.elsewhere("work/scribe");
        std::fs::create_dir_all(&cwd).unwrap();
        Self {
            session: session.to_owned(),
            path: project.join(format!("{session}.jsonl")),
            cwd,
        }
    }

    fn write(&self, turns: &[(&str, &str, &str)]) {
        let mut out = format!(
            "{}\n",
            json!({"type": "session-owner", "sessionId": self.session})
        );
        let now = time::OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .unwrap();
        for (n, (uuid, role, body)) in turns.iter().enumerate() {
            let content = if *role == "user" {
                json!(body)
            } else {
                json!([{"type": "text", "text": body}])
            };
            let ago = i64::try_from(turns.len() - n).unwrap();
            let at = (now - time::Duration::minutes(ago))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
            let line = json!({
                "type": role,
                "uuid": uuid,
                "sessionId": self.session,
                "timestamp": at,
                "message": {"role": role, "content": content}
            });
            out.push_str(&format!("{line}\n"));
        }
        std::fs::write(&self.path, out).unwrap();
        make_private(&self.path);
    }

    fn prompt(&self, fx: &Fixture, prompt: &str) -> Value {
        let payload = json!({
            "session_id": self.session,
            "transcript_path": self.path,
            "cwd": self.cwd,
            "permission_mode": "default",
            "hook_event_name": "UserPromptSubmit",
            "prompt": prompt
        });
        let out = piped(fx.cmd(&["hook", "user-prompt-submit"]), &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn session_start(&self, fx: &Fixture) -> Value {
        let payload = json!({
            "session_id": self.session,
            "transcript_path": self.path,
            "cwd": self.cwd,
            "hook_event_name": "SessionStart",
            "source": "startup"
        });
        let mut cmd = fx.cmd(&["hook", "session-start"]);
        cmd.env("CLAUDE_CODE_ENTRYPOINT", "cli");
        let out = piped(cmd, &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

fn piped(mut cmd: Command, payload: &Value) -> Output {
    let child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = support::ChildGuard::new(child);
    guard
        .child()
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    guard.finish()
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn refused(fx: &Fixture, args: &[&str]) -> (String, String) {
    let mut args = args.to_vec();
    args.push("--json");
    let out = fx.run(&args);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected failure for {args:?}, got stdout={} stderr={}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    (
        err["code"].as_str().unwrap().to_owned(),
        err["error"].as_str().unwrap().to_owned(),
    )
}

#[test]
fn handoff_requires_verbatim_user_quote_and_session_ref_and_works_for_off_unrecorded_session() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Please write a handoff for this session before we pause.";
    tr.write(&[
        ("u-1", "user", "Let's check the store schema."),
        (
            "a-1",
            "assistant",
            "I can write a handoff for you right now.",
        ),
        ("u-2", "user", request),
    ]);

    // Shipped `handoff` tag is visible in `tag show` as a non-action tag with no capture rules.
    let shown_tag = json_of(&fx.ok(&["tag", "show", "handoff", "--json"]));
    assert_eq!(shown_tag["tag"], "handoff");
    assert_eq!(shown_tag["action"], Value::Null);
    assert_eq!(shown_tag["capture"]["route"], Value::Null);

    let body = valid_handoff("Schema check done.", "Run the integration suite.");
    let rel = format!("session:{OFF_SESSION}");

    // Refused: missing --quote.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            &body,
        ],
    );
    assert_eq!(code, "quote_required");

    // Refused: missing --related session:<id>.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--quote",
            request,
            &body,
        ],
    );
    assert_eq!(code, "quote_required");

    // Refused: non-verbatim quote.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            "Write a handoff before pausing please.",
            &body,
        ],
    );
    assert_eq!(code, "quote_not_found");

    // Refused: quote from an assistant turn (not a human-typed turn).
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            "I can write a handoff for you right now.",
            &body,
        ],
    );
    assert_eq!(code, "quote_not_found");

    // Accepted: verbatim quote from user's turn in an off, unrecorded session.
    let added = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &rel,
        "--quote",
        request,
        &body,
        "--json",
    ]));
    assert_eq!(added["id"], "handoff-1");
    assert_eq!(added["tag"], "handoff");
    assert_eq!(added["author"]["kind"], "agent");
    assert_eq!(added["author"]["name"], "opus");
    assert_eq!(added["quote"], request);
    assert_eq!(added["related"], json!([rel]));

    // Writing a handoff did NOT switch the session on or create a session record.
    assert!(
        !support::has_control(&fx.root()),
        "writing a handoff must not add a session switch"
    );
    assert!(
        support::record_text(&fx.root(), OFF_SESSION).is_none(),
        "writing a handoff must not create a session record"
    );
    let listed_sessions = json_of(&fx.ok(&["session", "list", "--include-inactive", "--json"]));
    assert!(listed_sessions["sessions"].as_array().unwrap().is_empty());

    // `session pass` has nothing due for an off session with only a handoff.
    let pass_out = json_of(&fx.ok(&["session", "pass", "--json"]));
    assert!(pass_out["passes"].as_array().unwrap().is_empty());
    assert_eq!(pass_out["remaining"], 0);
}

#[test]
fn handoff_enforces_fixed_headings_order_and_size_limit_naming_the_fault() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Write a handoff now so the next session can continue.";
    tr.write(&[("u-1", "user", request)]);
    let rel = format!("session:{OFF_SESSION}");

    // Missing heading(s).
    let missing_body = "\
## State
Done.

## In flight
WIP.

## Next
Tests.

## Do not touch
Goldens.

## Pointers
README.md
";
    let (code, msg) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            request,
            missing_body,
        ],
    );
    assert_eq!(code, "handoff_shape");
    assert!(msg.contains("Waiting on you"), "{msg}");

    // Out-of-order headings.
    let unordered_body = "\
## State
Done.

## Next
Tests.

## In flight
WIP.

## Waiting on you
Nothing.

## Do not touch
Goldens.

## Pointers
README.md
";
    let (code, msg) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            request,
            unordered_body,
        ],
    );
    assert_eq!(code, "handoff_shape");
    assert!(msg.contains("out of order"), "{msg}");

    // Exceeds 3000-character limit.
    let huge = valid_handoff(&"x".repeat(3100), "Continue.");
    let (code, msg) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            request,
            &huge,
        ],
    );
    assert_eq!(code, "handoff_too_long");
    assert!(msg.contains("3000"), "{msg}");

    // Write a valid one, then verify `entry update --content` enforces the same contract.
    let valid = valid_handoff("Initial state.", "Next step.");
    let added = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &rel,
        "--quote",
        request,
        &valid,
        "--json",
    ]));
    assert_eq!(added["id"], "handoff-1");

    let (code, msg) = refused(
        &fx,
        &[
            "entry",
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--content",
            missing_body,
        ],
    );
    assert_eq!(code, "handoff_shape");
    assert!(msg.contains("Waiting on you"), "{msg}");

    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--content",
            &huge,
        ],
    );
    assert_eq!(code, "handoff_too_long");
}

#[test]
fn session_show_brief_and_retire_handle_multiple_handoffs_on_recorded_and_off_sessions() {
    let fx = Fixture::new();

    // 1. An off session with no record gets two handoffs (different timestamps).
    let off_tr = Transcript::new(&fx, OFF_SESSION);
    let req1 = "Please write a first handoff for this off session.";
    let req2 = "Please write a second updated handoff for this off session.";
    off_tr.write(&[("u-1", "user", req1), ("u-2", "user", req2)]);
    let off_rel = format!("session:{OFF_SESSION}");

    let h1 = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &off_rel,
        "--quote",
        req1,
        &valid_handoff("First state.", "First next."),
        "--json",
    ]));
    assert_eq!(h1["id"], "handoff-1");
    // Backdate handoff-1 so newest-first ordering is deterministic.
    support::sql(
        &fx.root(),
        "UPDATE entries SET created = '2026-11-01T10:00:00Z', updated = '2026-11-01T10:00:00Z' WHERE id = 'handoff-1'",
    );

    let h2 = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "sonnet",
        "--project",
        "scribe",
        "--related",
        &off_rel,
        "--quote",
        req2,
        &valid_handoff("Second state.", "Second next."),
        "--json",
    ]));
    assert_eq!(h2["id"], "handoff-2");
    support::sql(
        &fx.root(),
        "UPDATE entries SET created = '2026-11-01T11:00:00Z', updated = '2026-11-01T11:00:00Z' WHERE id = 'handoff-2'",
    );

    // `session show` works for the off session (by full id and unique prefix) and lists newest first.
    let shown_off = json_of(&fx.ok(&["session", "show", OFF_SESSION, "--json"]));
    assert_eq!(shown_off["id"], OFF_SESSION);
    assert_eq!(shown_off["has_record"], false);
    assert_eq!(shown_off["on"], false);
    let off_handoffs = shown_off["handoffs"].as_array().unwrap();
    assert_eq!(off_handoffs.len(), 2);
    assert_eq!(off_handoffs[0]["id"], "handoff-2");
    assert_eq!(off_handoffs[0]["author"]["name"], "sonnet");
    assert_eq!(off_handoffs[0]["created"], "2026-11-01T11:00:00Z");
    assert_eq!(off_handoffs[1]["id"], "handoff-1");
    assert_eq!(off_handoffs[1]["author"]["name"], "opus");
    assert_eq!(off_handoffs[1]["created"], "2026-11-01T10:00:00Z");

    let shown_prefix = json_of(&fx.ok(&["session", "show", &OFF_SESSION[..8], "--json"]));
    assert_eq!(shown_prefix["id"], OFF_SESSION);

    // Table and plain rendering include author, timestamp, and newest-first order.
    let table_off = text(
        &fx.ok(&["session", "show", OFF_SESSION, "--format", "table"])
            .stdout,
    );
    let pos2 = table_off.find("handoff-2").expect("handoff-2 in table");
    let pos1 = table_off.find("handoff-1").expect("handoff-1 in table");
    assert!(pos2 < pos1, "newest handoff first:\n{table_off}");
    assert!(
        table_off.contains("sonnet") && table_off.contains("2026-11-01T11:00:00Z"),
        "{table_off}"
    );

    // `--segment` on a handoff-only session is refused with segment_not_found.
    let (code, _) = refused(&fx, &["session", "show", OFF_SESSION, "--segment", "1"]);
    assert_eq!(code, "segment_not_found");

    // 2. A recorded session with a handoff.
    let on_tr = Transcript::new(&fx, ON_SESSION);
    let on_req = "Please save a handoff for this recorded session too.";
    on_tr.write(&[
        ("u-1", "user", "Let's refactor the brief builder."),
        ("a-1", "assistant", "Done refactoring."),
        ("u-2", "user", on_req),
    ]);
    on_tr.prompt(&fx, "/scribe on");
    let fake_pass = fx.fake_program(
        "claude",
        &format!(
            "#!/bin/sh\ncat > /dev/null\nprintf '%s\\n' '{}'\n",
            json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "result": "",
                "structured_output": {
                    "title": "Brief refactor",
                    "summary": "Refactored brief builder.",
                    "goals": "Surface handoffs in brief.",
                    "open": null,
                    "new": [{
                        "starts_at": "u-1",
                        "heading": "Brief work",
                        "text": "Refactored brief builder.",
                        "topics": [],
                        "related": []
                    }],
                    "next": ["Write tests."],
                    "topics": ["brief"],
                    "related": [],
                    "terms": [],
                    "candidates": [],
                    "memory": []
                }
            })
        ),
    );
    let mut pass_cmd = fx.cmd(&["session", "pass", "--session", ON_SESSION, "--json"]);
    pass_cmd.env("PATH", &fake_pass);
    let pass_out = pass_cmd.output().unwrap();
    assert_eq!(
        pass_out.status.code(),
        Some(0),
        "{}",
        text(&pass_out.stderr)
    );

    let on_rel = format!("session:{ON_SESSION}");
    let h3 = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &on_rel,
        "--quote",
        on_req,
        &valid_handoff("Recorded session state.", "Recorded next."),
        "--json",
    ]));
    assert_eq!(h3["id"], "handoff-3");

    // `session show` on the recorded session shows both scribe's header and the handoff.
    let shown_on = json_of(&fx.ok(&["session", "show", ON_SESSION, "--json"]));
    assert_eq!(shown_on["title"], "Brief refactor");
    assert_eq!(shown_on["summary"], "Refactored brief builder.");
    assert_eq!(shown_on["handoffs"][0]["id"], "handoff-3");
    assert_eq!(shown_on["handoffs"][0]["author"]["name"], "opus");

    // `scribe brief` marks the recorded session AND includes the unrecorded off session.
    let brief_json = json_of(&fx.ok(&["brief", "--project", "scribe", "--json"]));
    let sessions = brief_json["sessions"].as_array().unwrap();
    let rec = sessions
        .iter()
        .find(|s| s["id"] == ON_SESSION)
        .expect("recorded session in brief");
    assert_eq!(rec["has_record"], true);
    assert_eq!(rec["handoff"], "handoff-3");
    assert_eq!(rec["handoffs"], 1);
    let off = sessions
        .iter()
        .find(|s| s["id"] == OFF_SESSION)
        .expect("off session in brief");
    assert_eq!(off["has_record"], false);
    assert_eq!(off["handoff"], "handoff-2");
    assert_eq!(off["handoffs"], 2);

    let brief_text = text(&fx.ok(&["brief", "--project", "scribe"]).stdout);
    assert!(brief_text.contains("[handoff: handoff-3]"), "{brief_text}");
    assert!(
        brief_text.contains("no session record [handoff: handoff-2]"),
        "{brief_text}"
    );

    // `hook session-start` also surfaces the handoff marker in its brief context.
    let start_tr = Transcript::new(&fx, "7f3e2d90-4b6c-4e51-8a83-2c1d0e9f8a71");
    let start_reply = start_tr.session_start(&fx);
    let ctx = start_reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(ctx.contains("[handoff: handoff-3]"), "{ctx}");
    assert!(ctx.contains("[handoff: handoff-2]"), "{ctx}");

    // `#scribe retire handoff-2` removes the newest handoff from `session show` and updates `brief`.
    let retire_reply = off_tr.prompt(&fx, "#scribe retire handoff-2");
    assert!(
        retire_reply["systemMessage"]
            .as_str()
            .unwrap_or_default()
            .contains("retired"),
        "{retire_reply}"
    );
    let shown_after = json_of(&fx.ok(&["session", "show", OFF_SESSION, "--json"]));
    let remaining = shown_after["handoffs"].as_array().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0]["id"], "handoff-1");

    let brief_after = json_of(&fx.ok(&["brief", "--project", "scribe", "--json"]));
    let off_after = brief_after["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == OFF_SESSION)
        .unwrap();
    assert_eq!(off_after["handoff"], "handoff-1");
    assert_eq!(off_after["handoffs"], 1);
}

#[test]
fn search_and_ask_treat_handoff_as_agent_note_never_users_words() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Write a handoff about the widget migration before stopping.";
    tr.write(&[("u-1", "user", request)]);
    let rel = format!("session:{OFF_SESSION}");

    let body = valid_handoff(
        "Widget migration phase one is complete on branch widget-v2.",
        "Run the widget smoke test and merge.",
    );
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &rel,
        "--quote",
        request,
        &body,
        "--json",
    ]);

    // `scribe search` finds the handoff as an agent-authored entry.
    let found = json_of(&fx.ok(&["search", "widget-v2", "--json"]));
    let hits = found["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["id"], "handoff-1");
    assert_eq!(hits[0]["author"]["kind"], "agent");
    assert_eq!(hits[0]["author"]["name"], "opus");

    // `scribe question ask` includes `handoff` by default (non-action tag) and attributes it as an agent note.
    let answer = json!({
        "status": "answered",
        "answer": "Widget migration phase one is complete on branch widget-v2.",
        "claims": [{
            "text": "phase one is on widget-v2",
            "quote": "Widget migration phase one is complete on branch widget-v2.",
            "cite": "handoff-1"
        }],
        "conflicts": []
    });
    let claude = fx.fake_program(
        "claude",
        &format!(
            "#!/bin/sh\ncat > \"$HOME/ask-stdin\"\nprintf '{{\"type\":\"result\",\"is_error\":false,\"result\":\"\",\"structured_output\":%s}}\\n' '{}'\n",
            answer
        ),
    );
    let mut cmd = fx.cmd(&[
        "question",
        "ask",
        "what is the state of widget migration?",
        "--json",
    ]);
    cmd.env("PATH", &claude);
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let asked = json_of(&out);
    assert_eq!(asked["status"], "answered", "{asked}");
    assert_eq!(asked["claims"][0]["cite"], "handoff-1");
    assert_eq!(asked["claims"][0]["text"], "agent note (opus)");
    assert!(
        asked["answer"]
            .as_str()
            .unwrap()
            .starts_with("agent note (opus): "),
        "{asked}"
    );
    let ask_prompt = std::fs::read_to_string(fx.home().join("ask-stdin")).unwrap();
    assert!(
        ask_prompt.contains("words=\"agent note (opus)\""),
        "{ask_prompt}"
    );
    // Plain rendering also marks it as an agent note, never the user's words.
    let mut plain_cmd = fx.cmd(&["question", "ask", "what is the state of widget migration?"]);
    plain_cmd.env("PATH", &claude);
    let plain = text(&plain_cmd.output().unwrap().stdout);
    assert!(plain.contains("agent note (opus)"), "{plain}");
}

#[test]
fn session_pass_yields_no_memory_or_proposed_claim_from_handoff_text() {
    let fx = Fixture::new();
    // Configure an assistant-reading catcher too so we prove an assistant turn echoing a handoff
    // cannot be admitted as a candidate either.
    fx.config(
        r#"
[tags.notes.capture]
from = "both"
route = "propose"
triggers = ["always", "from now on"]
criteria = "Any standing rule mentioned by user or assistant."
not = "One-off remarks."
"#,
    );

    let tr = Transcript::new(&fx, ON_SESSION);
    let request = "Please write a handoff summarizing where we stopped.";
    let handoff_body = valid_handoff(
        "Always keep snapshots private; from now on we migrated to the new pipeline on 2026-11-22.",
        "From now on run the full suite before every release.",
    );
    let echoed_reply = format!("Recorded handoff:\n\n{handoff_body}");
    tr.write(&[
        ("u-1", "user", request),
        ("a-1", "assistant", &echoed_reply),
    ]);

    // Write the handoff entry on the session.
    let rel = format!("session:{ON_SESSION}");
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &rel,
        "--quote",
        request,
        &handoff_body,
        "--json",
    ]);

    // Switch the session on and run a pass whose fake model tries to return both a candidate
    // claim and a memory item quoting the handoff's text.
    tr.prompt(&fx, "/scribe on");
    let claim_quote =
        "Always keep snapshots private; from now on we migrated to the new pipeline on 2026-11-22.";
    let pass_answer = json!({
        "title": "Pipeline work",
        "summary": "Summarized pipeline state.",
        "goals": "Finish migration.",
        "open": null,
        "new": [{
            "starts_at": "u-1",
            "heading": "Handoff turn",
            "text": "User asked for a handoff.",
            "topics": [],
            "related": []
        }],
        "next": ["Run tests."],
        "topics": ["pipeline"],
        "related": [],
        "terms": [],
        "candidates": [{
            "quote": claim_quote,
            "message": "a-1",
            "tag": "notes",
            "subject": "snapshots",
            "context": "Always keep snapshots private. Mentioned in the handoff."
        }],
        "memory": [{
            "kind": "now",
            "now_kind": "plan",
            "title": "Pipeline migration",
            "text": "Migrated to the new pipeline.",
            "quote": claim_quote,
            "message": "a-1",
            "date": "2026-11-22"
        }]
    });
    let claude = fx.fake_program(
        "claude",
        &format!(
            "#!/bin/sh\ncat > \"$HOME/pass-stdin\"\nprintf '{{\"type\":\"result\",\"is_error\":false,\"result\":\"\",\"structured_output\":%s}}\\n' '{}'\n",
            pass_answer
        ),
    );
    let mut cmd = fx.cmd(&["session", "pass", "--session", ON_SESSION, "--json"]);
    cmd.env("PATH", &claude);
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));

    // Zero proposed claims and zero memory items were created from the handoff text.
    let proposals = json_of(&fx.ok(&["proposal", "list", "--json"]));
    assert!(
        proposals["proposals"].as_array().unwrap().is_empty(),
        "pass must open no claim from handoff text: {proposals}"
    );
    let memory = json_of(&fx.ok(&["memory", "show", "--json"]));
    assert!(
        memory["now"].as_array().unwrap().is_empty()
            && memory["milestones"].as_array().unwrap().is_empty(),
        "pass must write no memory item from handoff text: {memory}"
    );

    // The handoff entry body was not fed as a preferences/notes exclusion either.
    let prompt = std::fs::read_to_string(fx.home().join("pass-stdin")).unwrap();
    assert!(
        !prompt.contains("handoff-1"),
        "pass prompt should not include stored handoff entries:\n{prompt}"
    );
}

#[test]
fn marker_capture_cannot_create_or_attribute_a_handoff_to_the_user() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let unhinted = "Please save a session summary before we stop #scribe";
    let hinted = "#scribe handoff: capture where we paused in this branch";
    let folded = "#scribe add this summary to handoff";
    let pref = "Always keep status updates concise #scribe preferences";
    tr.write(&[
        ("u-1", "user", unhinted),
        ("u-2", "user", hinted),
        ("u-3", "user", folded),
        ("u-4", "user", pref),
    ]);

    // First Stop handles at most 2 marked turns (u-1 and u-2), second Stop handles u-3 and u-4.
    let answers = json!([
        // Turn 1: u-1 (unhinted) tries to route to `handoff` -> must be refused.
        {
            "marker": 1,
            "kind": "entry",
            "tag": "handoff",
            "context": "The user asked to save a session summary. They marked the turn for capture.",
            "subject": "session summary",
            "quote": "Please save a session summary before we stop",
            "message": "u-1",
            "reason": ""
        }
    ]);
    let fake_claude = fx.fake_program(
        "claude",
        &format!(
            "#!/bin/sh\ncount_file=\"$HOME/call-count\"\nn=0\nif [ -f \"$count_file\" ]; then n=$(cat \"$count_file\"); fi\nn=$((n + 1))\necho \"$n\" > \"$count_file\"\ncat > \"$HOME/claude-stdin-$n\"\ncase \"$n\" in\n  1) out='{}' ;;\n  2) out='{}' ;;\n  3) out='{}' ;;\n  *) out='{}' ;;\nesac\nprintf '{{\"type\":\"result\",\"is_error\":false,\"result\":\"\",\"structured_output\":%s}}\\n' \"$out\"\n",
            json!({"marks": answers}),
            json!({"marks": [{
                "marker": 1,
                "kind": "entry",
                "tag": "preferences",
                "context": "The user marked a handoff line. The model tried to route it to preferences instead.",
                "subject": "handoff pause",
                "quote": "capture where we paused in this branch",
                "message": "u-2",
                "reason": ""
            }]}),
            json!({"marks": [{
                "marker": 1,
                "kind": "entry",
                "tag": "handoff",
                "context": "The user asked to add a summary to handoff. The model routed it to handoff.",
                "subject": "summary",
                "quote": "add this summary to handoff",
                "message": "u-3",
                "reason": ""
            }]}),
            json!({"marks": [{
                "marker": 1,
                "kind": "entry",
                "tag": "preferences",
                "context": "The user prefers concise status updates. They marked this rule for scribe.",
                "subject": "status updates",
                "quote": "Always keep status updates concise",
                "message": "u-4",
                "reason": ""
            }]}),
        ),
    );

    let stop_payload = json!({
        "session_id": OFF_SESSION,
        "transcript_path": tr.path,
        "cwd": tr.cwd,
        "hook_event_name": "Stop"
    });
    for _ in 0..2 {
        let mut cmd = fx.cmd(&["hook", "stop"]);
        cmd.env("PATH", &fake_claude);
        let out = piped(cmd, &stop_payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }

    // `handoff` was never offered in the annotation prompt's `Tags:` list.
    let prompt1 = std::fs::read_to_string(fx.home().join("claude-stdin-1")).unwrap();
    assert!(!prompt1.contains("\n- handoff:"), "{prompt1}");
    assert!(prompt1.contains("\n- preferences:"), "{prompt1}");

    // Zero `handoff` entries were written, and `u-2` did not leak into `preferences`.
    let listed = json_of(&fx.ok(&["entry", "list", "--json"]));
    let entries = listed["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "only the preferences marker should persist: {entries:?}"
    );
    assert_eq!(entries[0]["id"], "preferences-1");
    assert_eq!(entries[0]["tag"], "preferences");

    // All three handoff-targeted markers were recorded as refused (`unknown_tag`) in `marks.json`.
    let marks_doc: Value = serde_json::from_str(
        &support::exported(&fx.root(), "marks.json").expect("marks.json written"),
    )
    .unwrap();
    let marks = marks_doc["marks"].as_array().unwrap();
    assert_eq!(marks.len(), 4, "{marks:?}");
    for uuid in ["u-1", "u-2", "u-3"] {
        let m = marks.iter().find(|m| m["message"] == uuid).unwrap();
        assert_eq!(m["outcome"], "refused", "{m}");
        assert_eq!(m["code"], "unknown_tag", "{m}");
    }
    let m4 = marks.iter().find(|m| m["message"] == "u-4").unwrap();
    assert_eq!(m4["outcome"], "entry", "{m4}");
    assert_eq!(m4["entry"], "preferences-1", "{m4}");

    // Agent handoff path still works normally on the same session.
    let rel = format!("session:{OFF_SESSION}");
    let added = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &rel,
        "--quote",
        unhinted,
        &valid_handoff("State recorded.", "Continue next."),
        "--json",
    ]));
    assert_eq!(added["id"], "handoff-1");
    assert_eq!(added["author"]["kind"], "agent");
}
