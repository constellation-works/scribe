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
        self.start_from(fx, "startup", "claude-desktop")
    }

    fn start_from(&self, fx: &Fixture, source: &str, entrypoint: &str) -> Value {
        let payload = json!({
            "session_id": self.session,
            "transcript_path": self.path,
            "cwd": self.cwd,
            "hook_event_name": "SessionStart",
            "source": source
        });
        let mut cmd = fx.cmd(&["hook", "session-start"]);
        cmd.env("CLAUDE_CODE_ENTRYPOINT", entrypoint);
        let out = piped(cmd, &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        if out.stdout.is_empty() {
            Value::Null
        } else {
            json_of(&out)
        }
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
        "--mode",
        "new",
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
    assert!(
        hits[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("handoff mode: new")
    );

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
    assert_eq!(
        asked["claims"][0]["text"],
        "agent note (opus) [handoff mode: new]"
    );
    assert!(
        asked["answer"]
            .as_str()
            .unwrap()
            .starts_with("agent note (opus) [handoff mode: new]: "),
        "{asked}"
    );
    let ask_prompt = std::fs::read_to_string(fx.home().join("ask-stdin")).unwrap();
    assert!(
        ask_prompt.contains("words=\"agent note (opus) [handoff mode: new]\""),
        "{ask_prompt}"
    );
    // Plain rendering also marks it as an agent note, never the user's words.
    let mut plain_cmd = fx.cmd(&["question", "ask", "what is the state of widget migration?"]);
    plain_cmd.env("PATH", &claude);
    let plain = text(&plain_cmd.output().unwrap().stdout);
    assert!(
        plain.contains("agent note (opus) [handoff mode: new]"),
        "{plain}"
    );
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

#[test]
fn handoff_add_refuses_multiple_or_invalid_session_refs_so_unverified_sessions_never_display_it() {
    let fx = Fixture::new();
    let tr_a = Transcript::new(&fx, OFF_SESSION);
    let tr_b = Transcript::new(&fx, ON_SESSION);
    let third = "8a4f3e01-5c7d-4f62-9b94-3d2e1f0a9b82";
    let tr_c = Transcript::new(&fx, third);

    let request = "Please write a handoff for this session before stopping.";
    tr_a.write(&[("u-1", "user", request)]);
    tr_b.write(&[("u-1", "user", "Unrelated discussion in another session.")]);
    tr_c.write(&[("u-1", "user", request)]);

    let body = valid_handoff(
        "Work paused cleanly.",
        "Resume from the next checklist item.",
    );
    let rel_a = format!("session:{OFF_SESSION}");
    let rel_b = format!("session:{ON_SESSION}");
    let rel_c = format!("session:{third}");

    // Refused: verified session A paired with unverified session B.
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
            &rel_a,
            "--related",
            &rel_b,
            "--quote",
            request,
            &body,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Refused: even when both transcripts contain the same generic phrase, a handoff belongs to one session.
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
            &rel_a,
            "--related",
            &rel_c,
            "--quote",
            request,
            &body,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Refused: invalid session id syntax alongside a valid session (or alone).
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
            &rel_a,
            "--related",
            "session:../bad",
            "--quote",
            request,
            &body,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Neither unverified session displays a handoff, and no entry was written.
    let (code, _) = refused(&fx, &["session", "show", ON_SESSION]);
    assert_eq!(code, "session_not_found");
    let listed = json_of(&fx.ok(&["entry", "list", "--json"]));
    assert!(listed["entries"].as_array().unwrap().is_empty());

    // Accepted: single verified session plus non-session related refs (and duplicate identical session ref).
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
        &rel_a,
        "--related",
        &rel_a,
        "--related",
        "task:ORB-1",
        "--related",
        "repo:scribe",
        "--quote",
        request,
        &body,
        "--json",
    ]));
    assert_eq!(added["id"], "handoff-1");

    // Only OFF_SESSION displays the handoff in session show and brief.
    let shown_a = json_of(&fx.ok(&["session", "show", OFF_SESSION, "--json"]));
    assert_eq!(shown_a["handoffs"].as_array().unwrap().len(), 1);
    assert_eq!(shown_a["handoffs"][0]["id"], "handoff-1");
    let (code, _) = refused(&fx, &["session", "show", ON_SESSION]);
    assert_eq!(code, "session_not_found");
    let brief = json_of(&fx.ok(&["brief", "--project", "scribe", "--json"]));
    let sessions = brief["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], OFF_SESSION);
}

#[test]
fn handoff_update_cannot_move_attach_or_orphan_session_reference_but_allows_other_metadata() {
    let fx = Fixture::new();
    let tr_a = Transcript::new(&fx, OFF_SESSION);
    let tr_b = Transcript::new(&fx, ON_SESSION);
    let request = "Please save a handoff before ending this session.";
    tr_a.write(&[("u-1", "user", request)]);
    tr_b.write(&[("u-1", "user", "Different session without that request.")]);

    let rel_a = format!("session:{OFF_SESSION}");
    let rel_b = format!("session:{ON_SESSION}");
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
        &rel_a,
        "--related",
        "task:ORB-1",
        "--quote",
        request,
        &valid_handoff("Initial state.", "Initial next."),
        "--json",
    ]));
    assert_eq!(added["id"], "handoff-1");

    // Refused: removing the verified session would orphan the handoff.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--remove-related",
            &rel_a,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Refused: adding another session would display the handoff on an unverified session.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--add-related",
            &rel_b,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Refused: replacing the verified session with another session.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--remove-related",
            &rel_a,
            "--add-related",
            &rel_b,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Allowed: editing content, topics, and non-session related references preserves the session ref.
    let updated_body = valid_handoff("Updated state.", "Updated next.");
    let updated = json_of(&fx.ok(&[
        "entry",
        "update",
        "handoff-1",
        "--author",
        "opus",
        "--content",
        &updated_body,
        "--add-topic",
        "release",
        "--remove-related",
        "task:ORB-1",
        "--add-related",
        "pr:constellation/scribe#42",
        "--json",
    ]));
    assert_eq!(
        updated["related"],
        json!([rel_a, "pr:constellation/scribe#42"])
    );
    assert_eq!(updated["topics"], json!(["release"]));

    // Allowed no-ops: re-adding the existing session or removing an absent session changes nothing.
    let noop_add = fx.ok(&[
        "entry",
        "update",
        "handoff-1",
        "--author",
        "opus",
        "--add-related",
        &rel_a,
        "--json",
    ]);
    assert_eq!(text(&noop_add.stderr).trim(), "nothing changed");
    let noop_remove = fx.ok(&[
        "entry",
        "update",
        "handoff-1",
        "--author",
        "opus",
        "--remove-related",
        &rel_b,
        "--json",
    ]);
    assert_eq!(text(&noop_remove.stderr).trim(), "nothing changed");

    // Only OFF_SESSION displays handoff-1; ON_SESSION still does not.
    let shown_a = json_of(&fx.ok(&["session", "show", OFF_SESSION, "--json"]));
    assert_eq!(shown_a["handoffs"].as_array().unwrap().len(), 1);
    assert_eq!(shown_a["handoffs"][0]["id"], "handoff-1");
    let (code, _) = refused(&fx, &["session", "show", ON_SESSION]);
    assert_eq!(code, "session_not_found");
    let brief = json_of(&fx.ok(&["brief", "--project", "scribe", "--json"]));
    let sessions = brief["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], OFF_SESSION);
}

#[test]
fn session_show_reports_actual_switch_state_for_switched_on_handoff_only_session_before_first_pass()
{
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, ON_SESSION);
    let request = "Please save a handoff before our first pass runs.";
    tr.write(&[("u-1", "user", request)]);

    // Switch the session on, without running a session pass (no catalog record yet).
    tr.prompt(&fx, "/scribe on");
    assert!(
        support::record_text(&fx.root(), ON_SESSION).is_none(),
        "no catalog record exists before the first pass"
    );

    // Add an active handoff for the switched-on, unrecorded session.
    let rel = format!("session:{ON_SESSION}");
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
        &valid_handoff("Switched on, no pass yet.", "Run first pass or resume."),
        "--json",
    ]));
    assert_eq!(added["id"], "handoff-1");

    // `session show` must report the actual switch state while keeping `has_record: false` and the handoff.
    let shown = json_of(&fx.ok(&["session", "show", ON_SESSION, "--json"]));
    assert_eq!(shown["id"], ON_SESSION);
    assert_eq!(shown["has_record"], false);
    assert_eq!(shown["on"], true);
    assert_eq!(shown["due"], true);
    assert_eq!(shown["handoffs"].as_array().unwrap().len(), 1);
    assert_eq!(shown["handoffs"][0]["id"], "handoff-1");

    // Prefix and short id (`s-1`) resolve the same way.
    let shown_short = json_of(&fx.ok(&["session", "show", "s-1", "--json"]));
    assert_eq!(shown_short, shown);

    // Table and plain rendering reflect `on: yes` / `due: yes` and avoid saying it was not switched on.
    let table = text(
        &fx.ok(&["session", "show", ON_SESSION, "--format", "table"])
            .stdout,
    );
    assert!(table.contains("handoff-1"), "{table}");
    assert!(
        !table.contains("not switched on"),
        "switched-on session must not claim it was not switched on:\n{table}"
    );
    assert!(
        table
            .lines()
            .any(|l| l.starts_with("on ") && l.contains("yes")),
        "table must report on=yes:\n{table}"
    );
    assert!(
        table
            .lines()
            .any(|l| l.starts_with("due ") && l.contains("yes")),
        "table must report due=yes:\n{table}"
    );

    let plain = text(&fx.ok(&["session", "show", ON_SESSION]).stdout);
    let first_line = plain.lines().next().unwrap();
    assert_eq!(
        first_line.split('\t').nth(2),
        Some("yes"),
        "plain row due column must be yes:\n{plain}"
    );

    // Switching off before any pass still has no record, and now reports `on: false`, `due: false`.
    tr.prompt(&fx, "/scribe off");
    let shown_off = json_of(&fx.ok(&["session", "show", ON_SESSION, "--json"]));
    assert_eq!(shown_off["has_record"], false);
    assert_eq!(shown_off["on"], false);
    assert_eq!(shown_off["due"], false);
    assert_eq!(shown_off["handoffs"][0]["id"], "handoff-1");
}

#[test]
fn self_refresh_records_mode_and_requires_current_interactive_session() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    tr.write(&[("u-1", "user", "Continue working on the widget.")]);
    let body = valid_handoff("Widget ready.", "Continue widget work.");
    let related = format!("session:{OFF_SESSION}");
    let args = [
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--related",
        &related,
        "--mode",
        "continue",
        "--json",
        &body,
    ];
    let out = fx
        .cmd(&args)
        .env("CLAUDE_CODE_SESSION_ID", OFF_SESSION)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["handoff_mode"], "continue");
    let shown = json_of(&fx.ok(&["session", "show", OFF_SESSION, "--json"]));
    assert_eq!(shown["handoffs"][0]["handoff_mode"], "continue");
    let brief = json_of(&fx.ok(&["brief", "--json"]));
    assert!(brief.to_string().contains("continue"));
    let search = json_of(&fx.ok(&["search", "widget", "--json"]));
    assert!(search.to_string().contains("handoff mode: continue"));
    let other = fx
        .cmd(&args)
        .env("CLAUDE_CODE_SESSION_ID", ON_SESSION)
        .output()
        .unwrap();
    assert_eq!(other.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&other.stderr).unwrap()["code"],
        "quote_required"
    );
    tr.write(&[("a-1", "assistant", "No user turn.")]);
    let empty = fx
        .cmd(&args)
        .env("CLAUDE_CODE_SESSION_ID", OFF_SESSION)
        .output()
        .unwrap();
    assert_eq!(empty.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&empty.stderr).unwrap()["code"],
        "quote_not_found"
    );
}

#[test]
fn legacy_database_reads_without_migrating_and_upgrades_on_write() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Please write this session handoff.";
    tr.write(&[("u-1", "user", request)]);
    let rel = format!("session:{OFF_SESSION}");
    let body = valid_handoff("Legacy state.", "Next task.");
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--related",
        &rel,
        "--quote",
        request,
        &body,
    ]);
    support::remove_group_capture_schema(&fx.root());
    support::sql(
        &fx.root(),
        "DROP TABLE group_members; DROP TABLE group_seats; DROP TABLE groups;
             ALTER TABLE entries DROP COLUMN handoff_mode; PRAGMA user_version = 6;",
    );
    let shown = json_of(&fx.ok(&["session", "show", OFF_SESSION, "--json"]));
    assert_eq!(shown["handoffs"][0]["handoff_mode"], Value::Null);
    let out = fx
        .cmd(&[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--related",
            &rel,
            "--mode",
            "new",
            "--json",
            &valid_handoff("New state.", "Next task."),
        ])
        .env("CLAUDE_CODE_SESSION_ID", OFF_SESSION)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["handoff_mode"], "new");
}

const CHILD: &str = "7f3e2d90-4b6c-4e51-8a83-2c1d0e9f8a71";

fn refresh_handoff(fx: &Fixture, parent: &Transcript, mode: &str, body: &str) -> String {
    parent.write(&[("u-1", "user", "Continue working on the widget.")]);
    let out = fx
        .cmd(&[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--related",
            &format!("session:{}", parent.session),
            "--mode",
            mode,
            "--json",
            body,
        ])
        .env("CLAUDE_CODE_SESSION_ID", &parent.session)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    json_of(&out)["id"].as_str().unwrap().to_owned()
}

fn after_clear(fx: &Fixture, parents: &[&str]) -> Transcript {
    let child = Transcript::new(fx, CHILD);
    child.write(&[]);
    let mut sessions = vec![json!({"via":"launch", "id":ON_SESSION})];
    sessions.extend(parents.iter().map(|id| json!({"via":"clear","id":id})));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&child.path)
        .unwrap();
    writeln!(
        file,
        "{}",
        json!({"type":"derived-from","sessions":sessions})
    )
    .unwrap();
    child
}

#[test]
fn refresh_continue_after_desktop_clear_inherits_switch_links_and_prepends_one_handoff() {
    for on in [true, false] {
        let fx = Fixture::new();
        let parent = Transcript::new(&fx, OFF_SESSION);
        let body = valid_handoff("The chosen handoff.", "Resume the widget.");
        refresh_handoff(&fx, &parent, "continue", &body);
        let neighbor = Transcript::new(&fx, ON_SESSION);
        refresh_handoff(
            &fx,
            &neighbor,
            "continue",
            &valid_handoff("Other captain.", "Stay here."),
        );
        if on {
            parent.prompt(&fx, "/scribe on");
        }
        let child = after_clear(&fx, &[OFF_SESSION]);
        let before = support::audit_lines(&fx.root()).len();
        for _ in 0..2 {
            let reply = child.session_start(&fx);
            let context = reply["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();
            assert!(context.starts_with(body.trim_end()), "{context}");
            assert_eq!(context.matches("## State").count(), 1);
            assert!(!context.contains("Other captain."));
            let status = child.prompt(&fx, "/scribe status")["reason"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(
                status.starts_with(if on { "scribe: on" } else { "scribe: off" }),
                "{status}"
            );
            assert!(
                status.contains(&format!("continues {OFF_SESSION}")),
                "{status}"
            );
        }
        let audit = support::audit_lines(&fx.root());
        assert_eq!(audit.len(), before + 1);
        assert_eq!(audit[before]["command"], "hook session-start");
        if on {
            child.prompt(&fx, "/scribe off");
            child.session_start(&fx);
            let status = child.prompt(&fx, "/scribe status");
            assert!(
                status["reason"]
                    .as_str()
                    .unwrap()
                    .starts_with("scribe: off")
            );
        }
    }
}

#[test]
fn refresh_new_or_absent_handoff_leaves_the_ordinary_brief_and_switch_alone() {
    let fx = Fixture::new();
    let parent = Transcript::new(&fx, OFF_SESSION);
    let body = valid_handoff("Leave this context behind.", "Start afresh.");
    refresh_handoff(
        &fx,
        &parent,
        "continue",
        &valid_handoff("Original plan.", "Keep working."),
    );
    refresh_handoff(&fx, &parent, "new", &body);
    parent.prompt(&fx, "/scribe on");
    for predecessor in [OFF_SESSION, ON_SESSION] {
        let child = after_clear(&fx, &[predecessor]);
        let before = support::audit_lines(&fx.root());
        let reply = child.session_start(&fx);
        let context = reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            context.contains("[handoff: handoff-2, mode: new]"),
            "{context}"
        );
        assert!(!context.contains("## State"));
        assert!(reply.get("systemMessage").is_none(), "{reply}");
        assert_eq!(support::audit_lines(&fx.root()), before);
        let status = child.prompt(&fx, "/scribe status");
        assert!(
            status["reason"]
                .as_str()
                .unwrap()
                .contains("never switched on")
        );
    }
}

#[test]
fn refresh_ambiguous_or_missing_ancestry_waits_for_a_typed_handoff_id() {
    let fx = Fixture::new();
    let parent = Transcript::new(&fx, OFF_SESSION);
    let body = valid_handoff("Resume exactly here.", "Run the fixture.");
    let id = refresh_handoff(&fx, &parent, "continue", &body);
    let child = after_clear(&fx, &[OFF_SESSION, ON_SESSION]);
    let before = support::audit_lines(&fx.root());
    let ambiguous = child.session_start(&fx);
    let message = ambiguous["systemMessage"].as_str().unwrap();
    assert!(
        message.contains(OFF_SESSION) && message.contains(ON_SESSION),
        "{message}"
    );
    assert!(
        !ambiguous["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("## State")
    );
    child.write(&[]);
    let ordinary = child.session_start(&fx);
    assert!(
        !ordinary["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("## State")
    );
    assert_eq!(support::audit_lines(&fx.root()), before);
    let reply = child.prompt(&fx, &format!("/scribe:scribe continue {id}"));
    assert!(reply.get("decision").is_none(), "{reply}");
    assert_eq!(
        reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .trim_end(),
        body.trim_end()
    );
    let status = child.prompt(&fx, "/scribe status");
    assert!(
        status["reason"]
            .as_str()
            .unwrap()
            .contains(&format!("continues {OFF_SESSION}"))
    );
}

#[test]
fn refresh_requires_interactive_start_and_provenance_and_refuses_a_self_link() {
    let fx = Fixture::new();
    let parent = Transcript::new(&fx, OFF_SESSION);
    let id = refresh_handoff(&fx, &parent, "continue", &valid_handoff("Parent.", "Next."));
    let child = after_clear(&fx, &[OFF_SESSION]);
    let before = support::audit_lines(&fx.root());
    for (source, entrypoint) in [
        ("startup", "sdk-cli"),
        ("resume", "cli"),
        ("compact", "claude-desktop"),
    ] {
        let reply = child.start_from(&fx, source, entrypoint);
        assert!(
            !reply["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap_or_default()
                .contains("## State")
        );
    }
    let mut mismatched = Transcript::new(&fx, ON_SESSION);
    mismatched.path = child.path.clone();
    let reply = mismatched.session_start(&fx);
    assert!(
        !reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("## State")
    );
    assert_eq!(support::audit_lines(&fx.root()), before);
    let refused = parent.prompt(&fx, &format!("/scribe continue {id}"));
    assert_eq!(refused["decision"], "block");
    assert!(refused.get("hookSpecificOutput").is_none());
    assert_eq!(
        support::audit_lines(&fx.root()).last().unwrap()["error_code"],
        "link_self"
    );
    let status = parent.prompt(&fx, "/scribe status");
    assert!(
        status["reason"]
            .as_str()
            .unwrap()
            .contains("never switched on")
    );
}

// Write the opening ancestry before the peer-origin user turn, as Claude does.
fn clear_transcript(fx: &Fixture, id: &str, parents: &[&str]) -> Transcript {
    let edges: Vec<_> = parents.iter().map(|parent| ("clear", *parent)).collect();
    ancestry_transcript(fx, id, &edges)
}

// Write the opening ancestry of `id`, one `(via, parent)` edge each, before the
// peer-origin user turn, as Claude does.
fn ancestry_transcript(fx: &Fixture, id: &str, edges: &[(&str, &str)]) -> Transcript {
    let tr = Transcript::new(fx, id);
    let sessions: Vec<_> = edges
        .iter()
        .map(|(via, parent)| json!({"via": via, "id": parent}))
        .collect();
    let rows = [
        json!({"type":"session-owner", "sessionId":id}),
        json!({"type":"derived-from", "sessions":sessions}),
        json!({"type":"user", "uuid":"peer-1", "sessionId":id,
            "timestamp":"2026-11-22T12:00:00Z", "promptSource":"system",
            "turnOrigin":"peer", "message":{"role":"user", "content":"<cross-session>continue from handoff-1</cross-session>"}}),
    ];
    std::fs::write(
        &tr.path,
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    make_private(&tr.path);
    tr
}

fn try_refresh(fx: &Fixture, id: &str) -> Output {
    fx.cmd(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--related",
        &format!("session:{id}"),
        "--mode",
        "continue",
        "--json",
        &valid_handoff(&format!("Continued {id}."), "Keep working."),
    ])
    .env("CLAUDE_CODE_SESSION_ID", id)
    .output()
    .unwrap()
}

#[test]
fn peer_restarted_session_can_write_second_refresh_through_clear_ancestry() {
    let fx = Fixture::new();
    let parent = Transcript::new(&fx, OFF_SESSION);
    refresh_handoff(
        &fx,
        &parent,
        "continue",
        &valid_handoff("First.", "Continue."),
    );
    clear_transcript(&fx, CHILD, &[OFF_SESSION]);
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["handoff_mode"], "continue");
    assert_eq!(json_of(&out)["id"], "handoff-2");
    let grandchild = "8f3e2d90-4b6c-4e51-8a83-2c1d0e9f8a72";
    clear_transcript(&fx, grandchild, &[CHILD]);
    let out = try_refresh(&fx, grandchild);
    assert!(out.status.success(), "{}", text(&out.stderr));
}

#[test]
fn refresh_rejects_invalid_clear_ancestry_and_reports_failed_link() {
    let fx = Fixture::new();
    let parent = clear_transcript(&fx, OFF_SESSION, &[]);
    clear_transcript(&fx, CHILD, &[OFF_SESSION]);
    let assert_refused = |id: &str, failed: &str| {
        let out = try_refresh(&fx, id);
        assert_eq!(out.status.code(), Some(1));
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], "quote_not_found");
        assert!(error.to_string().contains(failed), "{error}");
    };
    assert_refused(CHILD, OFF_SESSION);
    clear_transcript(&fx, OFF_SESSION, &[CHILD]);
    assert_refused(CHILD, CHILD);
    clear_transcript(&fx, OFF_SESSION, &[CHILD, ON_SESSION]);
    assert_refused(CHILD, OFF_SESSION);
    // A symlink is never acceptable transcript provenance, even to a typed transcript.
    let typed = Transcript::new(&fx, ON_SESSION);
    typed.write(&[("u", "user", "A typed turn")]);
    std::fs::remove_file(&parent.path).unwrap();
    std::os::unix::fs::symlink(&typed.path, &parent.path).unwrap();
    assert_refused(CHILD, OFF_SESSION);
}

#[test]
fn refresh_clear_ancestry_is_limited_to_sixteen_hops() {
    let fx = Fixture::new();
    let ids: Vec<String> = (0..18)
        .map(|n| format!("00000000-0000-4000-8000-{n:012x}"))
        .collect();
    for n in 0..17 {
        clear_transcript(&fx, &ids[n], &[&ids[n + 1]]);
    }
    Transcript::new(&fx, &ids[17]).write(&[("u", "user", "A typed turn")]);
    let accepted = try_refresh(&fx, &ids[2]);
    assert!(accepted.status.success(), "{}", text(&accepted.stderr));
    let refused = try_refresh(&fx, &ids[0]);
    assert_eq!(refused.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&refused.stderr).unwrap();
    assert_eq!(error["code"], "quote_not_found");
    assert!(error.to_string().contains(&ids[15]));
    assert!(error.to_string().contains("16-transcript"));
}

// A session id no transcript on this host has.
const UNREADABLE_SESSION: &str = "9a0b1c2d-3e4f-4a5b-8c6d-7e8f9a0b1c2d";

#[test]
fn refresh_follows_a_launch_edge_from_a_typed_session_to_a_cleared_rank() {
    let fx = Fixture::new();
    Transcript::new(&fx, ON_SESSION).write(&[("u", "user", "A typed turn")]);
    // The seat's root: a rank a peer launched from the typed session, never typed in.
    ancestry_transcript(&fx, OFF_SESSION, &[("launch", ON_SESSION)]);
    // The session a peer restarted after a clear from that root.
    clear_transcript(&fx, CHILD, &[OFF_SESSION]);
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["handoff_mode"], "continue");
}

#[test]
fn refresh_launch_ancestry_refuses_untyped_ambiguous_cyclic_and_unreadable_links() {
    let fx = Fixture::new();
    let assert_refused = |id: &str, stopped_at: &str, reason: &str| {
        let out = try_refresh(&fx, id);
        assert_eq!(out.status.code(), Some(1));
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], "quote_not_found");
        assert!(error.to_string().contains(stopped_at), "{error}");
        assert!(error.to_string().contains(reason), "{error}");
    };
    // A launched root with no typed turn and no predecessor of its own.
    ancestry_transcript(&fx, OFF_SESSION, &[("launch", ON_SESSION)]);
    ancestry_transcript(&fx, ON_SESSION, &[]);
    clear_transcript(&fx, CHILD, &[OFF_SESSION]);
    assert_refused(CHILD, ON_SESSION, "typed user turn");
    // An unreadable first launcher is a dead end; the second can be typed.
    Transcript::new(&fx, ON_SESSION).write(&[("u", "user", "A typed turn")]);
    ancestry_transcript(
        &fx,
        OFF_SESSION,
        &[("launch", UNREADABLE_SESSION), ("launch", ON_SESSION)],
    );
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
    // Metadata order matters: a cycle before a typed launcher is refused,
    // even though the typed session sorts first by id.
    ancestry_transcript(
        &fx,
        OFF_SESSION,
        &[("launch", CHILD), ("launch", ON_SESSION)],
    );
    assert_refused(CHILD, CHILD, "cycle");
    // A launch edge back to a session already on the walk.
    ancestry_transcript(&fx, OFF_SESSION, &[("launch", CHILD)]);
    assert_refused(CHILD, CHILD, "cycle");
    // A launch edge to a session this host cannot read.
    ancestry_transcript(&fx, CHILD, &[("launch", UNREADABLE_SESSION)]);
    assert_refused(CHILD, UNREADABLE_SESSION, "not readable");
}

#[test]
fn refresh_follows_a_refreshed_ranks_clear_edge_before_its_launch_edge() {
    let fx = Fixture::new();
    Transcript::new(&fx, ON_SESSION).write(&[("u", "user", "A typed turn")]);
    ancestry_transcript(&fx, OFF_SESSION, &[("launch", ON_SESSION)]);
    // A refreshed rank: its launch edge names a session this host cannot read.
    // The walk takes the clear edge to the seat's root first, so it is not read.
    ancestry_transcript(
        &fx,
        CHILD,
        &[
            ("launch", UNREADABLE_SESSION),
            ("launch", ON_SESSION),
            ("clear", OFF_SESSION),
        ],
    );
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["handoff_mode"], "continue");
}

#[test]
fn refresh_launch_branches_share_the_transcript_budget() {
    let fx = Fixture::new();
    let ids: Vec<String> = (0..16)
        .map(|n| format!("10000000-0000-4000-8000-{n:012x}"))
        .collect();
    for n in 0..14 {
        clear_transcript(&fx, &ids[n], &[&ids[n + 1]]);
    }
    clear_transcript(&fx, &ids[14], &[]);
    Transcript::new(&fx, &ids[15]).write(&[("u", "user", "A typed turn")]);
    ancestry_transcript(&fx, CHILD, &[("launch", &ids[0]), ("launch", &ids[15])]);
    let out = try_refresh(&fx, CHILD);
    assert_eq!(out.status.code(), Some(1));
    let error = text(&out.stderr);
    assert!(error.contains(&ids[14]), "{error}");
    assert!(error.contains("16-transcript"), "{error}");
    // Shortening the first branch leaves room for the typed launcher.
    clear_transcript(&fx, &ids[13], &[]);
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
}

#[test]
fn refresh_clear_and_launch_edges_share_the_sixteen_hop_limit() {
    let fx = Fixture::new();
    let ids: Vec<String> = (0..18)
        .map(|n| format!("00000000-0000-4000-8000-{n:012x}"))
        .collect();
    for n in 0..17 {
        let via = if n % 2 == 0 { "launch" } else { "clear" };
        ancestry_transcript(&fx, &ids[n], &[(via, ids[n + 1].as_str())]);
    }
    Transcript::new(&fx, &ids[17]).write(&[("u", "user", "A typed turn")]);
    let accepted = try_refresh(&fx, &ids[2]);
    assert!(accepted.status.success(), "{}", text(&accepted.stderr));
    let refused = try_refresh(&fx, &ids[0]);
    assert_eq!(refused.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&refused.stderr).unwrap();
    assert_eq!(error["code"], "quote_not_found");
    assert!(error.to_string().contains(&ids[15]));
    assert!(error.to_string().contains("16-transcript"));
}

#[test]
fn uncataloged_transcript_prefix_from_transcript_list_works_for_quoted_and_self_refresh_handoffs() {
    let since = (time::OffsetDateTime::now_utc() - time::Duration::days(1))
        .date()
        .to_string();

    // 1. Quoted handoff for an uncataloged transcript with no store record.
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Please write a handoff for this uncataloged session.";
    tr.write(&[("u-1", "user", request)]);

    let listed = text(
        &fx.ok(&[
            "transcript",
            "list",
            "--since",
            &since,
            "--min-turns",
            "1",
            "--format",
            "table",
        ])
        .stdout,
    );
    let off_prefix = &OFF_SESSION[..8];
    assert!(listed.contains(off_prefix), "{listed}");
    assert!(!listed.contains(OFF_SESSION), "{listed}");

    let quoted = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &format!("session:{off_prefix}"),
        "--quote",
        request,
        &valid_handoff("Quoted via prefix.", "Continue."),
        "--json",
    ]));
    assert_eq!(quoted["related"], json!([format!("session:{OFF_SESSION}")]));
    let shown = json_of(&fx.ok(&["entry", "show", "handoff-1", "--json"]));
    assert_eq!(shown["related"], json!([format!("session:{OFF_SESSION}")]));
    assert!(!support::has_control(&fx.root()));
    assert!(support::record_text(&fx.root(), OFF_SESSION).is_none());

    // 2. Self-refresh handoff for a fresh uncataloged transcript with no store record.
    let fx2 = Fixture::new();
    let refresh_tr = Transcript::new(&fx2, ON_SESSION);
    refresh_tr.write(&[("u-1", "user", "Let's refresh our handoff.")]);
    let listed2 = text(
        &fx2.ok(&[
            "transcript",
            "list",
            "--since",
            &since,
            "--min-turns",
            "1",
            "--format",
            "table",
        ])
        .stdout,
    );
    let on_prefix = &ON_SESSION[..8];
    assert!(listed2.contains(on_prefix), "{listed2}");
    assert!(!listed2.contains(ON_SESSION), "{listed2}");

    let refresh_out = fx2
        .cmd(&[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--related",
            &format!("session:{on_prefix}"),
            "--mode",
            "continue",
            "--json",
            &valid_handoff("Refreshed via prefix.", "Keep going."),
        ])
        .env("CLAUDE_CODE_SESSION_ID", ON_SESSION)
        .output()
        .unwrap();
    assert!(
        refresh_out.status.success(),
        "{}",
        text(&refresh_out.stderr)
    );
    let refreshed = json_of(&refresh_out);
    assert_eq!(refreshed["handoff_mode"], "continue");
    assert_eq!(
        refreshed["related"],
        json!([format!("session:{ON_SESSION}")])
    );

    // 3. Ambiguity across two uncataloged transcripts refuses both quoted and self-refresh writes.
    let fx3 = Fixture::new();
    let sibling = "5d1c0b7e-9999-4c39-8e61-0a9b8c7d6e5f";
    Transcript::new(&fx3, OFF_SESSION).write(&[("u-1", "user", request)]);
    Transcript::new(&fx3, sibling).write(&[("u-1", "user", "Another session sharing prefix.")]);

    let listed3 = text(
        &fx3.ok(&[
            "transcript",
            "list",
            "--since",
            &since,
            "--min-turns",
            "1",
            "--format",
            "table",
        ])
        .stdout,
    );
    assert!(listed3.contains("5d1c0b7e-2 "), "{listed3}");
    assert!(listed3.contains("5d1c0b7e-9 "), "{listed3}");

    let ambig_quoted = fx3.run(&[
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--related",
        &format!("session:{off_prefix}"),
        "--quote",
        request,
        "--json",
        &valid_handoff("Ambiguous quoted.", "Stop."),
    ]);
    assert_eq!(ambig_quoted.status.code(), Some(2));
    let err: Value = serde_json::from_slice(&ambig_quoted.stderr).unwrap();
    assert_eq!(err["code"], "id_ambiguous");
    assert!(
        err["error"].as_str().unwrap().contains(OFF_SESSION)
            && err["error"].as_str().unwrap().contains(sibling),
        "{err}"
    );
    assert_eq!(json_of(&fx3.ok(&["entry", "list", "--json"]))["total"], 0);

    let ambig_refresh = fx3
        .cmd(&[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--related",
            &format!("session:{off_prefix}"),
            "--mode",
            "continue",
            "--json",
            &valid_handoff("Ambiguous refresh.", "Stop."),
        ])
        .env("CLAUDE_CODE_SESSION_ID", OFF_SESSION)
        .output()
        .unwrap();
    assert_eq!(ambig_refresh.status.code(), Some(2));
    let err: Value = serde_json::from_slice(&ambig_refresh.stderr).unwrap();
    assert_eq!(err["code"], "id_ambiguous");
    assert_eq!(json_of(&fx3.ok(&["entry", "list", "--json"]))["total"], 0);

    // Passing the disambiguated prefix `5d1c0b7e-2` succeeds and cites the full ID.
    let ok_refresh = fx3
        .cmd(&[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--related",
            "session:5d1c0b7e-2",
            "--mode",
            "continue",
            "--json",
            &valid_handoff("Disambiguated refresh.", "Go."),
        ])
        .env("CLAUDE_CODE_SESSION_ID", OFF_SESSION)
        .output()
        .unwrap();
    assert!(ok_refresh.status.success(), "{}", text(&ok_refresh.stderr));
    assert_eq!(
        json_of(&ok_refresh)["related"],
        json!([format!("session:{OFF_SESSION}")])
    );
}
