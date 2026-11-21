//! End-to-end tests of issue records (docs/design/scribe/2_design.md
//! §11.4): `issue add|occur|update|move|search|show|list`, the request
//! check, the reopen on a recurrence, the reserved `issue` tag, `#scribe
//! forget <issue-id>[#<n>]`, `record search --kind issue` and the audit
//! line each write leaves.
//!
//! Every run is the built binary with a temp `HOME` and data root.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use support::{Fixture, make_private, text};

const SESSION: &str = "7e2d4c1a-9b3f-4e8d-a6c5-1f0e2d3c4b5a";
const OTHER: &str = "00000000-0000-4000-8000-000000000000";

const ASK_1: &str = "can you investigate why the drain keeps failing with HTTP 429";
const ASK_2: &str = "the drain failed again, find out why it keeps hitting the limit";
const ASK_3: &str = "the 429s are back after the fix, look into it";

/// A Claude Code layout under the fixture's home.
struct Claude {
    transcript: PathBuf,
    cwd: PathBuf,
}

impl Claude {
    fn new(fx: &Fixture) -> Self {
        let project = fx.home().join(".claude/projects/-work-orbit");
        std::fs::create_dir_all(&project).unwrap();
        let cwd = fx.elsewhere("work/orbit");
        std::fs::create_dir_all(&cwd).unwrap();
        let claude = Self {
            transcript: project.join(format!("{SESSION}.jsonl")),
            cwd,
        };
        claude.write(&[
            ("u-1", "user", ASK_1),
            (
                "a-1",
                "assistant",
                "The GitHub API answered 429: a secondary rate limit.",
            ),
            ("u-2", "user", ASK_2),
            ("u-3", "user", ASK_3),
        ]);
        claude
    }

    /// Write the transcript: `(uuid, role, text)` turns, a minute apart.
    fn write(&self, turns: &[(&str, &str, &str)]) {
        let mut out = format!(
            "{}\n",
            json!({"type": "session-owner", "sessionId": SESSION})
        );
        for (n, (uuid, role, body)) in turns.iter().enumerate() {
            let content = if *role == "user" {
                json!(body)
            } else {
                json!([{"type": "text", "text": body}])
            };
            let line = json!({"type": role, "uuid": uuid, "sessionId": SESSION,
                "timestamp": format!("2026-11-21T09:{n:02}:00Z"),
                "message": {"role": role, "content": content}});
            out.push_str(&format!("{line}\n"));
        }
        std::fs::write(&self.transcript, out).unwrap();
        make_private(&self.transcript);
    }

    /// Run the `UserPromptSubmit` hook for `prompt`; the reply's
    /// `systemMessage`.
    fn prompt(&self, fx: &Fixture, prompt: &str) -> String {
        let payload = json!({"session_id": SESSION,
            "transcript_path": self.transcript, "cwd": self.cwd,
            "permission_mode": "default", "hook_event_name": "UserPromptSubmit",
            "prompt": prompt});
        let child = fx
            .cmd(&["hook", "user-prompt-submit"])
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
        let out = guard.finish();
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        reply["systemMessage"].as_str().unwrap().to_owned()
    }
}

fn session() -> String {
    format!("session:{SESSION}")
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

/// `args` as owned strings plus `--json`, run, required to succeed.
fn ok_json(fx: &Fixture, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    json_of(&fx.ok(&args))
}

/// Run `args` with `--json`, require failure, and return the error.
fn refused(fx: &Fixture, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    let out = fx.run(&args);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{args:?}: {}",
        text(&out.stdout)
    );
    serde_json::from_slice(&out.stderr).unwrap()
}

fn add_args<'a>(session: &'a str, request: &'a str) -> Vec<&'a str> {
    vec![
        "issue",
        "add",
        "--author",
        "opus",
        "--related",
        session,
        "--related",
        "task:ORB-1",
        "--request",
        request,
        "--title",
        "GitHub API secondary rate limit during drains",
        "--signature",
        "HTTP 429",
        "--signature",
        "secondary rate limit",
        "--summary",
        "The drain stopped after the GitHub API refused pushes.",
        "--error-line",
        "HTTP 429: You have exceeded a secondary rate limit",
        "--project",
        "orbit",
        "--topic",
        "github",
    ]
}

/// Every audit line, parsed.
fn audit(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("audit.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Every file under `dir`, read whole.
fn every_file(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            every_file(&path, out);
        } else {
            out.push((
                path.clone(),
                String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned(),
            ));
        }
    }
}

#[test]
fn an_issue_is_added_occurred_updated_shown_listed_and_searched() {
    let fx = Fixture::new();
    let _claude = Claude::new(&fx);
    let s = session();

    // add: plain prints the row and how Daniel cancels; JSON the issue.
    let out = fx.ok(&add_args(&s, ASK_1));
    let plain = text(&out.stdout);
    assert!(
        plain.starts_with("issue-1\topen\t1\torbit\t"),
        "plain add: {plain}"
    );
    assert!(plain.contains("agent_evidence\t1\tHTTP 429: You have exceeded"));
    assert!(plain.contains(&format!("daniel_asked\t1\t{ASK_1}")));
    assert!(
        text(&out.stderr).contains("#scribe forget issue-1#1"),
        "{}",
        text(&out.stderr)
    );
    let shown = ok_json(&fx, &["issue", "show", "issue-1"]);
    assert_eq!(shown["id"], "issue-1");
    assert_eq!(shown["status"], "open");
    assert_eq!(shown["author"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(shown["cause"], "cause not found");
    assert_eq!(shown["fix"], "fix not found");
    assert_eq!(shown["projects"], json!(["orbit"]));
    assert_eq!(shown["topics"], json!(["github"]));
    let first = &shown["occurrences"][0];
    assert_eq!(first["n"], 1);
    assert_eq!(first["session"], SESSION);
    assert_eq!(first["request_message"], "u-1");
    assert_eq!(first["request"], ASK_1);
    assert_eq!(first["recurred"], false);
    assert_eq!(first["moved_from"], Value::Null);
    assert_eq!(first["links"], json!(["task:ORB-1"]));
    assert_eq!(first["cause"], "cause not found");

    // occur: a second occurrence, newest first.
    let occurred = ok_json(
        &fx,
        &[
            "issue",
            "occur",
            "issue-1",
            "--author",
            "sonnet",
            "--related",
            &s,
            "--request",
            "find out why it keeps hitting the limit",
            "--summary",
            "Another drain hit the limit.",
            "--cause",
            "Parallel pushes from one token",
            "--project",
            "scribe",
        ],
    );
    assert_eq!(occurred["occurrences"][0]["n"], 2);
    assert_eq!(occurred["occurrences"][0]["request_message"], "u-2");
    assert_eq!(occurred["occurrences"][0]["author"]["name"], "sonnet");
    assert_eq!(occurred["projects"], json!(["orbit", "scribe"]));

    // update: the header's cause, fix and status.
    let updated = ok_json(
        &fx,
        &[
            "issue",
            "update",
            "issue-1",
            "--author",
            "opus",
            "--cause",
            "Parallel pushes from one token",
            "--fix",
            "Serialize pushes per token",
            "--status",
            "resolved",
        ],
    );
    assert_eq!(updated["status"], "resolved");
    assert_eq!(updated["fix"], "Serialize pushes per token");
    assert_eq!(updated["updated_by"]["name"], "opus");
    let again = fx.ok(&[
        "issue", "update", "issue-1", "--author", "opus", "--status", "resolved",
    ]);
    assert!(text(&again.stderr).contains("nothing changed"));

    // list: plain rows and the JSON envelope; search finds it, labelled.
    let listed = text(&fx.ok(&["issue", "list"]).stdout);
    assert_eq!(listed.lines().count(), 1, "{listed}");
    assert!(listed.starts_with("issue-1\tresolved\t2\torbit,scribe\t"));
    let listed = ok_json(&fx, &["issue", "list"]);
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["truncated"], false);
    assert_eq!(listed["issues"][0]["id"], "issue-1");
    let none = fx.ok(&["issue", "list", "--status", "open"]);
    assert_eq!(text(&none.stdout), "");
    assert!(text(&none.stderr).contains("no issues match --status open"));

    for args in [
        &["issue", "search", "secondary rate limit 429"][..],
        &[
            "record",
            "search",
            "secondary rate limit 429",
            "--kind",
            "issue",
        ][..],
    ] {
        let found = ok_json(&fx, args);
        assert_eq!(found["total"], 1, "{args:?}");
        let hit = &found["hits"][0];
        assert_eq!(hit["cite"], "issue-1");
        assert_eq!(hit["kind"], "issue");
        let snippet = hit["snippet"].as_str().unwrap();
        assert!(
            snippet.contains("[agent account (opus); Daniel asked: \"find out why"),
            "{snippet}"
        );
    }
    let plain = text(&fx.ok(&["issue", "search", "429"]).stdout);
    assert!(plain.starts_with("issue-1\tissue\t"), "{plain}");
    let entries_only = ok_json(&fx, &["record", "search", "429", "--kind", "entry"]);
    assert_eq!(entries_only["total"], 0);
}

#[test]
fn an_occurrence_after_resolved_reopens_the_issue_and_is_marked_recurred() {
    let fx = Fixture::new();
    let _claude = Claude::new(&fx);
    let s = session();
    fx.ok(&add_args(&s, ASK_1));
    let occur = |request: &str| {
        ok_json(
            &fx,
            &[
                "issue",
                "occur",
                "issue-1",
                "--author",
                "opus",
                "--related",
                &s,
                "--request",
                request,
                "--summary",
                "It came back.",
            ],
        )
    };
    let open = occur(ASK_2);
    assert_eq!(
        open["occurrences"][0]["recurred"], false,
        "open: no recurrence"
    );
    fx.ok(&[
        "issue", "update", "issue-1", "--author", "opus", "--status", "resolved",
    ]);
    let out = fx.ok(&[
        "issue",
        "occur",
        "issue-1",
        "--author",
        "opus",
        "--related",
        &s,
        "--request",
        ASK_3,
        "--summary",
        "It came back after the fix.",
    ]);
    assert!(text(&out.stderr).contains("issue-1 recurred after its fix: reopened"));
    let shown = ok_json(&fx, &["issue", "show", "issue-1"]);
    assert_eq!(shown["status"], "open");
    assert_eq!(shown["occurrences"][0]["n"], 3);
    assert_eq!(shown["occurrences"][0]["recurred"], true);
    assert_eq!(shown["occurrences"][1]["recurred"], false);
    let table = text(
        &fx.ok(&["issue", "show", "issue-1", "--format", "table"])
            .stdout,
    );
    assert!(table.contains("recurred after fix"), "{table}");
    let persisted = std::fs::read_to_string(fx.root().join("issues/issue-1.md")).unwrap();
    assert!(persisted.contains("recurred: true"), "{persisted}");
}

#[test]
fn a_request_is_required_and_must_be_daniels_typed_words_in_the_session_named() {
    let fx = Fixture::new();
    let _claude = Claude::new(&fx);
    let s = session();
    let without_request: Vec<&str> = add_args(&s, ASK_1)
        .into_iter()
        .filter(|a| *a != "--request" && *a != ASK_1)
        .collect();
    assert_eq!(refused(&fx, &without_request)["code"], "quote_required");
    let without_session: Vec<&str> = add_args("task:ORB-2", ASK_1);
    assert_eq!(refused(&fx, &without_session)["code"], "quote_required");
    let assistant = add_args(&s, "The GitHub API answered 429");
    assert_eq!(
        refused(&fx, &assistant)["code"],
        "quote_not_found",
        "the assistant's words are not Daniel's"
    );
    let other = format!("session:{OTHER}");
    assert_eq!(
        refused(&fx, &add_args(&other, ASK_1))["code"],
        "quote_not_found",
        "no transcript for that session"
    );
    assert!(!fx.root().join("issues").exists(), "nothing written");

    // A span of his words is enough.
    let added = ok_json(&fx, &add_args(&s, "why the drain keeps failing"));
    assert_eq!(
        added["occurrences"][0]["request"],
        "why the drain keeps failing"
    );
    assert_eq!(added["occurrences"][0]["request_message"], "u-1");
    let occur = refused(
        &fx,
        &[
            "issue",
            "occur",
            "issue-1",
            "--author",
            "opus",
            "--related",
            &s,
            "--request",
            "words he never typed in this session",
            "--summary",
            "x",
        ],
    );
    assert_eq!(occur["code"], "quote_not_found");
}

#[test]
fn the_issue_tag_is_reserved_for_entries_and_scribe_lines_and_names_the_issue_commands() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let err = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "issue",
            "--author",
            "opus",
            "the drain hits 429s",
        ],
    );
    assert_eq!(err["code"], "reserved_tag");
    assert!(
        err["error"].as_str().unwrap().contains("scribe issue add"),
        "{err}"
    );
    for line in [
        "#scribe issue: the drain hits 429s",
        "#scribe add the drain hits 429s to issue",
    ] {
        let reply = claude.prompt(&fx, line);
        assert!(
            reply.contains("reserved for issue records") && reply.contains("scribe issue add"),
            "{line}: {reply}"
        );
    }
    assert!(!fx.root().join("entries").join("issue").exists());
    let lines = audit(&fx.root());
    assert!(
        lines
            .iter()
            .filter(|l| l["command"] == "hook user-prompt-submit")
            .all(|l| l["error_code"] == "reserved_tag"),
        "{lines:?}"
    );
}

#[test]
fn scribe_forget_removes_an_occurrence_or_the_whole_issue_leaving_no_words() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let s = session();
    fx.ok(&add_args(&s, ASK_1));
    fx.ok(&[
        "issue",
        "occur",
        "issue-1",
        "--author",
        "opus",
        "--related",
        &s,
        "--request",
        ASK_2,
        "--summary",
        "Zanzibar quokka telemetry overflowed.",
        "--error-line",
        "quokka-error: overflow in zanzibar",
        "--fix",
        "Raised the zanzibar quota",
    ]);
    let words = [
        "Zanzibar",
        "zanzibar",
        "quokka",
        ASK_2,
        "secondary rate limit",
        "GitHub API",
        "drain stopped",
        ASK_1,
    ];

    let reply = claude.prompt(&fx, "#scribe forget issue-1#2");
    assert!(reply.contains("forgot issue-1#2"), "{reply}");
    let shown = ok_json(&fx, &["issue", "show", "issue-1"]);
    assert_eq!(shown["occurrences"].as_array().unwrap().len(), 1);
    assert_eq!(
        shown["gone"],
        json!([{"n": 2, "reason": "forgotten", "to": null}])
    );
    let file = std::fs::read_to_string(fx.root().join("issues/issue-1.md")).unwrap();
    for word in ["Zanzibar", "zanzibar", "quokka", ASK_2] {
        assert!(!file.contains(word), "{word} left in {file}");
    }
    assert!(file.contains("secondary rate limit"), "the issue stays");

    let reply = claude.prompt(&fx, "#scribe forget issue-1");
    assert!(reply.contains("forgot issue-1"), "{reply}");
    let reply = claude.prompt(&fx, "#scribe forget issue-1");
    assert!(reply.contains("already forgotten"), "{reply}");
    let shown = ok_json(&fx, &["issue", "show", "issue-1"]);
    assert_eq!(shown["status"], "forgotten");
    assert_eq!(shown["title"], "");
    assert_eq!(shown["occurrences"], json!([]));
    let mut files = Vec::new();
    every_file(&fx.root(), &mut files);
    for (path, body) in &files {
        for word in words {
            assert!(!body.contains(word), "{word} left in {}", path.display());
        }
    }
    assert_eq!(ok_json(&fx, &["issue", "list"])["total"], 0);
    assert_eq!(
        ok_json(&fx, &["issue", "list", "--status", "forgotten"])["total"],
        1
    );
    assert_eq!(
        ok_json(&fx, &["issue", "search", "secondary rate"])["total"],
        0
    );
    let err = refused(
        &fx,
        &[
            "issue",
            "occur",
            "issue-1",
            "--author",
            "opus",
            "--related",
            &s,
            "--request",
            ASK_3,
            "--summary",
            "x",
        ],
    );
    assert_eq!(err["code"], "issue_forgotten");
    // The next issue never reuses the id.
    assert_eq!(ok_json(&fx, &add_args(&s, ASK_3))["id"], "issue-2");
    let reply = claude.prompt(&fx, "#scribe forget issue-9");
    assert!(
        reply.contains("issue_not_found") || reply.contains("no issue"),
        "{reply}"
    );
}

#[test]
fn a_moved_occurrence_keeps_where_it_came_from_and_its_old_number_is_not_reused() {
    let fx = Fixture::new();
    let _claude = Claude::new(&fx);
    let s = session();
    fx.ok(&add_args(&s, ASK_1));
    fx.ok(&[
        "issue",
        "occur",
        "issue-1",
        "--author",
        "opus",
        "--related",
        &s,
        "--request",
        ASK_2,
        "--summary",
        "CPU was exhausted, not a rate limit.",
    ]);
    let moved = fx.ok(&[
        "issue",
        "move",
        "issue-1#2",
        "--author",
        "opus",
        "--new",
        "--title",
        "CPU exhausted on the build host",
        "--signature",
        "load average",
        "--json",
    ]);
    assert!(text(&moved.stderr).contains("moved issue-1#2 to issue-2#1"));
    let target = json_of(&moved);
    assert_eq!(target["id"], "issue-2");
    assert_eq!(target["occurrences"][0]["moved_from"], "issue-1#2");
    assert_eq!(target["occurrences"][0]["request_message"], "u-2");
    let source = ok_json(&fx, &["issue", "show", "issue-1"]);
    assert_eq!(source["occurrences"].as_array().unwrap().len(), 1);
    assert_eq!(
        source["gone"],
        json!([{"n": 2, "reason": "moved", "to": "issue-2#1"}])
    );
    let same = fx.run(&[
        "issue",
        "move",
        "issue-2#1",
        "--author",
        "opus",
        "--to",
        "issue-2",
        "--json",
    ]);
    assert_eq!(same.status.code(), Some(2), "{}", text(&same.stderr));
    let err: Value = serde_json::from_slice(&same.stderr).unwrap();
    assert_eq!(err["code"], "usage_error");
    let back = ok_json(
        &fx,
        &[
            "issue",
            "move",
            "issue-2#1",
            "--author",
            "opus",
            "--to",
            "issue-1",
        ],
    );
    assert_eq!(back["occurrences"][0]["n"], 3, "2 is never given again");
    assert_eq!(back["occurrences"][0]["moved_from"], "issue-2#1");
}

#[test]
fn every_issue_write_and_forget_leaves_one_audit_line_of_ids_and_no_words() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let s = session();
    refused(&fx, &add_args(&s, "never typed by him"));
    fx.ok(&add_args(&s, ASK_1));
    fx.ok(&[
        "issue",
        "occur",
        "issue-1",
        "--author",
        "opus",
        "--related",
        &s,
        "--request",
        ASK_2,
        "--summary",
        "Again.",
    ]);
    fx.ok(&[
        "issue",
        "update",
        "issue-1",
        "--author",
        "opus",
        "--status",
        "mitigated",
    ]);
    fx.ok(&[
        "issue",
        "move",
        "issue-1#2",
        "--author",
        "opus",
        "--new",
        "--title",
        "Other",
        "--signature",
        "other",
    ]);
    claude.prompt(&fx, "#scribe forget issue-2#1");
    let lines = audit(&fx.root());
    let issue: Vec<(&str, &str, Value)> = lines
        .iter()
        .filter(|l| !l["issue"].is_null())
        .map(|l| {
            (
                l["command"].as_str().unwrap(),
                l["status"].as_str().unwrap(),
                l["issue"].clone(),
            )
        })
        .collect();
    let id = |verb: &str, id: Value, n: Value, message: Value, moved: Value| {
        json!({"verb": verb, "id": id, "occurrence": n, "session": SESSION,
            "request_message": message, "moved_to": moved})
    };
    let ids_only = |verb: &str, n: Value, moved: Value| {
        json!({"verb": verb, "id": "issue-1", "occurrence": n, "session": null,
            "request_message": null, "moved_to": moved})
    };
    assert_eq!(
        issue,
        vec![
            (
                "issue add",
                "failure",
                id("add", Value::Null, Value::Null, Value::Null, Value::Null)
            ),
            (
                "issue add",
                "success",
                id("add", json!("issue-1"), json!(1), json!("u-1"), Value::Null)
            ),
            (
                "issue occur",
                "success",
                id(
                    "occur",
                    json!("issue-1"),
                    json!(2),
                    json!("u-2"),
                    Value::Null
                )
            ),
            (
                "issue update",
                "success",
                ids_only("update", Value::Null, Value::Null)
            ),
            (
                "issue move",
                "success",
                ids_only("move", json!(2), json!("issue-2#1"))
            ),
            (
                "hook user-prompt-submit",
                "success",
                id(
                    "forget",
                    json!("issue-2"),
                    json!(1),
                    Value::Null,
                    Value::Null
                )
            ),
        ]
    );
    assert_eq!(
        lines
            .iter()
            .filter(|l| l["error_code"] == "quote_not_found")
            .count(),
        1
    );
    let log = std::fs::read_to_string(fx.root().join("audit.jsonl")).unwrap();
    for word in ["drain", "429", "rate limit", "Again", "never typed"] {
        assert!(!log.contains(word), "{word} in the audit log");
    }
}

#[test]
fn an_issue_record_from_a_newer_format_is_refused_and_left_alone() {
    let fx = Fixture::new();
    let _claude = Claude::new(&fx);
    fx.ok(&add_args(&session(), ASK_1));
    let path = fx.root().join("issues/issue-1.md");
    let original = std::fs::read_to_string(&path).unwrap();
    assert!(
        original.starts_with("---\nformat: 19\nkind: issue\n"),
        "{original}"
    );
    let newer = original.replacen("format: 19", "format: 999", 1);
    std::fs::write(&path, &newer).unwrap();
    for args in [
        &["issue", "list"][..],
        &["issue", "show", "issue-1"][..],
        &["issue", "search", "rate limit"][..],
        &["record", "search", "rate limit"][..],
    ] {
        assert_eq!(refused(&fx, args)["code"], "store_too_new", "{args:?}");
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        newer,
        "not rewritten"
    );
}

/// A fake `claude` that keeps its stdin and answers with
/// `$HOME/claude-answer.json`.
#[cfg(unix)]
const CLAUDE: &str = "#!/bin/sh\n\
    cat > \"$HOME/claude-stdin\"\n\
    printf '{\"type\":\"result\",\"is_error\":false,\"result\":\"\",\"structured_output\":%s}\\n' \"$(cat \"$HOME/claude-answer.json\")\"\n";

#[cfg(unix)]
#[test]
fn ask_cites_an_issue_as_the_agents_account_with_daniels_request_as_his() {
    let fx = Fixture::new();
    let _claude = Claude::new(&fx);
    fx.ok(&add_args(&session(), ASK_1));
    let path = fx.fake_program("claude", CLAUDE);
    std::fs::write(
        fx.home().join("claude-answer.json"),
        json!({
            "status": "answered",
            "answer": "model prose",
            "claims": [
                {"text": "asked", "quote": "why the drain keeps failing", "cite": "issue-1"},
                {"text": "said", "quote": "the GitHub API refused pushes", "cite": "issue-1"}
            ],
            "conflicts": []
        })
        .to_string(),
    )
    .unwrap();
    let out = fx
        .cmd(&[
            "question",
            "ask",
            "why does the drain fail with a secondary rate limit?",
            "--kind",
            "issue",
            "--json",
        ])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v = json_of(&out);
    assert_eq!(v["status"], "answered", "{v}");
    assert_eq!(v["claims"][0]["text"], "Daniel");
    assert_eq!(v["claims"][1]["text"], "agent account (opus)");
    let answer = v["answer"].as_str().unwrap();
    assert!(
        answer.contains("agent account (opus): \"the GitHub API refused pushes\" [issue-1]"),
        "{answer}"
    );
    let prompt = std::fs::read_to_string(fx.home().join("claude-stdin")).unwrap();
    assert!(prompt.contains("cite=\"issue-1\""), "{prompt}");
    assert!(prompt.contains("Error lines (agent evidence)"), "{prompt}");
}
