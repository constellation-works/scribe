//! End-to-end tests of S6 through the built binary
//! (docs/design/scribe/2_design.md §3, §7, §10 S6): `scribe consistency scan`
//! with a fake `claude` opening one `supersedes` proposal for a pair the
//! model labels and one `stale` proposal per entry due for review, never
//! proposing a settled pair again and re-applying an interrupted approval;
//! `#scribe approve|reject|retire|forget` settling them from a typed
//! prompt; the `SessionStart` hook reporting the open count; and the two
//! routine tools of `scribe plugin call`.
//!
//! Every run is the built binary with a temp `HOME` and data root; nothing
//! touches the real home.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use support::{Fixture, make_private, text};

const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
const OLDER: &str = "squash-merge agent PRs into agent-main";
const NEWER: &str = "rebase-merge agent PRs into agent-main, never squash";
const RITUAL: &str = "run the full gate before every push";

/// The judgement the fake model gives pair 1: a contradiction quoting both
/// entries verbatim.
fn contradicts() -> Value {
    json!({"judgements": [{"pair": "1", "label": "contradicts",
        "reason": "The newer decision replaces squash merges with rebase merges.",
        "older_quote": "squash-merge agent PRs", "newer_quote": "rebase-merge agent PRs"}]})
}

/// One entry file: `<tag>-<n>` recorded on `created`.
fn entry_file(
    id: &str,
    created: &str,
    content: &str,
    status: &str,
    review_after: Option<&str>,
) -> String {
    let tag = id.rsplit_once('-').unwrap().0;
    format!(
        "---
format: 31
kind: entry
id: {id}
tag: {tag}
status: {status}
project: scribe
topics: []
related: []
provider: claude-code
host: host
author: {{kind: user, name: user}}
updated_by: null
created: {created}
updated: {created}
inactive_since: null
said: \"#scribe {tag}: {content}\"
content_from: {{kind: instruction}}
cite: {{session: s-1, message: null, at: {created}, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
retirement_reason: null
superseded_by: null
review_after: {review}
quote: null
window: null
evidence: null
as_of: null
approved_by: null
instruction: null
removal_instruction: null
stated_by: null
outcome: null
---
{content}
",
        sha = "0".repeat(64),
        review = review_after.unwrap_or("null"),
    )
}

struct Setup {
    fx: Fixture,
    path: OsString,
    transcript: PathBuf,
    cwd: PathBuf,
}

impl Setup {
    /// A store of two contradicting preferences and a ritual past its review
    /// date, and a fake `claude` that labels pair 1 a contradiction.
    fn new() -> Self {
        let fx = Fixture::new();
        let project = fx.home().join(".claude/projects/-work-scribe");
        std::fs::create_dir_all(&project).unwrap();
        let cwd = fx.elsewhere("work/scribe");
        std::fs::create_dir_all(&cwd).unwrap();
        let path = fx.fake_program("claude", "#!/bin/sh\nexit 1\n");
        let setup = Self {
            transcript: project.join(format!("{SESSION}.jsonl")),
            fx,
            path,
            cwd,
        };
        setup.entry(
            "preferences-1",
            "2026-01-02T03:04:05Z",
            OLDER,
            "active",
            None,
        );
        setup.entry(
            "preferences-2",
            "2026-02-02T03:04:05Z",
            NEWER,
            "active",
            None,
        );
        setup.entry(
            "rituals-1",
            "2026-01-05T03:04:05Z",
            RITUAL,
            "active",
            Some("2026-03-01"),
        );
        setup.answer(&contradicts());
        setup
    }

    fn entry(&self, id: &str, created: &str, content: &str, status: &str, review: Option<&str>) {
        support::seed_records(
            &self.fx.root(),
            &[&entry_file(id, created, content, status, review)],
        );
    }

    /// The fake `claude`: counts its calls and answers `body` as
    /// `claude -p --output-format json` does.
    fn answer(&self, body: &Value) {
        let envelope = json!({"type": "result", "subtype": "success", "is_error": false,
                              "result": "", "structured_output": body});
        self.fx.fake_program(
            "claude",
            &format!(
                "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\ncat > /dev/null\necho '{envelope}'\n"
            ),
        );
    }

    /// A fake `claude` that fails every call.
    fn fail(&self) {
        self.fx.fake_program(
            "claude",
            "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\ncat > /dev/null\necho 'boom' >&2\nexit 1\n",
        );
    }

    fn calls(&self) -> usize {
        std::fs::read_to_string(self.fx.home().join("claude-calls"))
            .map_or(0, |t| t.lines().count())
    }

    fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let mut cmd = self.fx.cmd(args);
        cmd.env("PATH", &self.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut guard = support::ChildGuard::new(cmd.spawn().unwrap());
        guard
            .child()
            .stdin
            .take()
            .unwrap()
            .write_all(input)
            .unwrap();
        guard.finish()
    }

    fn scan(&self) -> Value {
        let out = self.run(&["consistency", "scan", "--json"], b"");
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args, b"");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn proposal(&self, id: &str) -> Value {
        let list = self.json(&["proposal", "list", "--json"]);
        list["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no {id} in {list}"))
    }

    fn entry_json(&self, id: &str) -> Value {
        self.json(&["entry", "show", id, "--json"])
    }

    fn audit(&self) -> Vec<Value> {
        audit_in(&self.fx.root())
    }

    /// Type `prompt` in the session: the transcript holds it, then the hook
    /// runs. Returns the line Daniel sees.
    fn type_prompt(&self, prompt: &str) -> String {
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let lines = [
            json!({"type": "user", "uuid": "u-1", "sessionId": SESSION, "timestamp": now,
                   "message": {"role": "user", "content": prompt}}),
        ];
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&self.transcript, body).unwrap();
        make_private(&self.transcript);
        let payload = json!({"session_id": SESSION, "transcript_path": self.transcript,
            "cwd": self.cwd, "permission_mode": "default",
            "hook_event_name": "UserPromptSubmit", "prompt": prompt});
        let out = self.run(
            &["hook", "user-prompt-submit"],
            payload.to_string().as_bytes(),
        );
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        reply["systemMessage"].as_str().unwrap().to_owned()
    }

    fn session_start(&self) -> String {
        let payload = json!({"session_id": SESSION, "transcript_path": self.transcript,
            "cwd": self.cwd, "hook_event_name": "SessionStart", "source": "startup"});
        let out = self.run(&["hook", "session-start"], payload.to_string().as_bytes());
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        text(&out.stdout)
    }

    fn write_proposals(&self, proposals: &Value) {
        seed_proposals(&self.fx.root(), proposals);
    }
}

/// Add `proposals` (format-31 shape; a candidate's `context` defaults to
/// null) to the store at `root`.
fn seed_proposals(root: &Path, proposals: &Value) {
    let mut proposals = proposals.clone();
    for p in proposals.as_array_mut().unwrap() {
        if let Some(c) = p.get_mut("candidate").and_then(Value::as_object_mut) {
            c.entry("context").or_insert(Value::Null);
            c.entry("statement").or_insert(Value::Null);
            c.entry("subject").or_insert(Value::Null);
        }
        let o = p.as_object_mut().unwrap();
        o.entry("candidate").or_insert(Value::Null);
        o.entry("instruction").or_insert(Value::Null);
        o.entry("author")
            .or_insert(json!({"kind": "scribe", "name": "consistency"}));
        for field in ["updated_by", "settled_author", "closed_reason"] {
            o.entry(field).or_insert(Value::Null);
        }
    }
    let body = json!({"format": 31, "proposals": proposals}).to_string();
    support::seed_records(root, &[&body]);
}

/// Renames the `segments` table of `scribe.db` away so any later read of
/// the sessions fails (`store_database`).
/// [`mend_sessions`] puts it back so the records can be read.
fn break_sessions(s: &Setup) {
    support::sql(
        &s.fx.root(),
        "ALTER TABLE segments RENAME TO segments_gone;",
    );
}

fn mend_sessions(s: &Setup) {
    support::sql(
        &s.fx.root(),
        "ALTER TABLE segments_gone RENAME TO segments;",
    );
}

fn audit_in(root: &Path) -> Vec<Value> {
    support::audit_lines(root)
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_scan_proposes_a_contradiction_and_a_stale_entry_once() {
    let s = Setup::new();
    let report = s.scan();
    assert_eq!(ids(&report["opened"]), ["p-1", "p-2"], "{report}");
    assert_eq!(report["judged_pairs"], 1);
    assert_eq!(report["stale_entries"], 1);
    assert_eq!(report["failed_batches"], json!([]));
    assert_eq!(s.calls(), 1, "one batch, one model call");

    let p1 = s.proposal("p-1");
    assert_eq!(p1["kind"], "supersedes");
    assert_eq!(
        (&p1["older"], &p1["newer"]),
        (&json!("preferences-1"), &json!("preferences-2"))
    );
    assert_eq!(p1["state"], "open");
    let quotes: Vec<&str> = p1["quotes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["text"].as_str().unwrap())
        .collect();
    assert_eq!(quotes, ["squash-merge agent PRs", "rebase-merge agent PRs"]);
    let p2 = s.proposal("p-2");
    assert_eq!(
        (&p2["kind"], &p2["older"], &p2["newer"]),
        (&json!("stale"), &json!("rituals-1"), &json!(null))
    );

    // The scan changes no entry.
    assert_eq!(s.entry_json("preferences-1")["status"], "active");
    assert_eq!(s.entry_json("rituals-1")["status"], "active");

    // One audit line per proposal opened.
    let audit = s.audit();
    let targets: Vec<&str> = audit
        .iter()
        .map(|l| l["target"].as_str().unwrap())
        .collect();
    assert_eq!(targets, ["p-1", "p-2"]);
    assert!(
        audit
            .iter()
            .all(|l| l["command"] == "consistency scan" && l["status"] == "success")
    );

    // Again: nothing to offer, no model call, no audit line.
    let out = s.run(&["consistency", "scan"], b"");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(
        text(&out.stderr).contains("nothing to propose"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(s.calls(), 1);
    assert_eq!(s.audit().len(), 2);
}

#[test]
fn settling_from_a_prompt_approves_rejects_retires_and_forgets() {
    let s = Setup::new();
    s.scan();
    assert_eq!(
        s.session_start(),
        "{\"systemMessage\":\"scribe: 2 proposals to review — scribe proposal list\"}\n"
    );

    let echo = s.type_prompt("Settling.\n#scribe approve p-1\n#scribe reject p-2");
    assert!(
        echo.contains("approved p-1 — preferences-1 is superseded by preferences-2"),
        "{echo}"
    );
    assert!(echo.contains("rejected p-2"), "{echo}");
    let older = s.entry_json("preferences-1");
    assert_eq!(older["status"], "superseded");
    assert_eq!(older["superseded_by"], "preferences-2");
    let p1 = s.proposal("p-1");
    assert_eq!(p1["state"], "approved");
    assert_eq!(p1["settled_by"]["session"], SESSION);
    assert_eq!(p1["settled_by"]["line"], 2);
    assert_eq!(s.proposal("p-2")["state"], "rejected");
    assert_eq!(
        s.entry_json("rituals-1")["status"],
        "active",
        "reject changes no entry"
    );
    assert_eq!(s.session_start(), "", "nothing open: no message");

    // A settled pair or stale entry is never proposed again.
    let calls = s.calls();
    assert_eq!(s.scan()["opened"], json!([]));
    assert_eq!(s.calls(), calls);

    // Replayed: nothing changes and nothing is audited.
    let lines = s.audit().len();
    let echo = s.type_prompt("#scribe approve p-1");
    assert!(
        echo.contains("already approved — nothing changed"),
        "{echo}"
    );
    assert_eq!(s.audit().len(), lines);

    // Approving a rejected proposal is refused, with its code audited.
    let echo = s.type_prompt("#scribe approve p-2");
    assert!(echo.contains("refused"), "{echo}");
    let last = s.audit().pop().unwrap();
    assert_eq!(last["error_code"], "proposal_settled");

    let echo = s.type_prompt("#scribe retire preferences-2");
    assert!(echo.contains("retired preferences-2"), "{echo}");
    let retired = s.entry_json("preferences-2");
    assert_eq!(retired["status"], "retired");
    assert_eq!(retired["content"], NEWER, "retire keeps the text");

    let echo = s.type_prompt("#scribe forget rituals-1");
    assert!(echo.contains("forgot rituals-1"), "{echo}");
    let forgotten = s.entry_json("rituals-1");
    assert_eq!(forgotten["status"], "forgotten");
    assert_eq!(
        (&forgotten["said"], &forgotten["content"]),
        (&json!(""), &json!(""))
    );
    let p2 = s.proposal("p-2");
    assert_eq!(p2["reason"], "");
    assert_eq!(p2["quotes"][0]["text"], "");
    let store = support::exported(&s.fx.root(), "proposals.json").unwrap()
        + &support::exported(&s.fx.root(), "entries/rituals/rituals-1.md").unwrap();
    assert!(
        !store.contains(RITUAL),
        "the forgotten words left the store"
    );

    let echo = s.type_prompt("#scribe retire rituals-1");
    assert!(echo.contains("refused"), "{echo}");
    assert_eq!(s.audit().pop().unwrap()["error_code"], "entry_not_active");
    let echo = s.type_prompt("#scribe approve preferences-1");
    assert!(echo.contains("takes a proposal id"), "{echo}");
}

#[test]
fn a_scan_reapplies_an_approval_whose_entry_is_still_active() {
    let s = Setup::new();
    s.write_proposals(&json!([{
        "id": "p-1", "kind": "supersedes", "older": "preferences-1", "newer": "preferences-2",
        "reason": "replaced", "quotes": [], "state": "approved",
        "opened_at": "2026-10-12T06:00:00Z", "settled_at": "2026-10-13T08:00:00Z",
        "settled_by": {"session": SESSION, "message": null, "at": "2026-10-13T08:00:00Z",
                       "prompt_sha256": "0".repeat(64), "line": 1, "cwd": "/work"}
    }]));
    let report = s.scan();
    assert_eq!(
        report["reapplied"],
        json!([{"proposal": "p-1", "entry": "preferences-1", "status": "superseded"}])
    );
    assert_eq!(
        ids(&report["opened"]),
        ["p-2"],
        "the pair is settled; the ritual is stale"
    );
    let older = s.entry_json("preferences-1");
    assert_eq!(older["status"], "superseded");
    assert_eq!(older["superseded_by"], "preferences-2");
    assert_eq!(s.calls(), 0, "the only pair is settled: no model call");
    let targets: Vec<String> = s
        .audit()
        .iter()
        .map(|l| l["target"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(targets, ["preferences-1", "p-2"]);
    assert!(s.scan()["reapplied"].as_array().unwrap().is_empty(), "once");
}

#[test]
fn a_failed_batch_is_reported_audited_and_offered_again() {
    let s = Setup::new();
    s.fail();
    let report = s.scan();
    assert_eq!(report["failed_batches"][0]["batch"], 1, "{report}");
    assert_eq!(
        ids(&report["opened"]),
        ["p-1"],
        "the stale proposal still opens"
    );
    let failure = s
        .audit()
        .into_iter()
        .find(|l| l["status"] == "failure")
        .unwrap();
    assert_eq!(failure["target"], "batch:1");
    assert!(failure["error_code"].is_string());

    s.answer(&contradicts());
    assert_eq!(
        ids(&s.scan()["opened"]),
        ["p-2"],
        "the pair is offered again"
    );
}

/// Run `plugin call` for `tool` with `config`, without `SCRIBE_ROOT`.
fn plugin(s: &Setup, tool: &str, config: &Value) -> Value {
    let envelope = json!({"schema_version": 1, "tool": tool, "input": {},
        "context": {"config": config}});
    let mut cmd = s.fx.cmd(&["plugin", "call"]);
    cmd.env_remove("SCRIBE_ROOT")
        .env("PATH", &s.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut guard = support::ChildGuard::new(cmd.spawn().unwrap());
    guard
        .child()
        .stdin
        .take()
        .unwrap()
        .write_all(envelope.to_string().as_bytes())
        .unwrap();
    let out = guard.finish();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn the_routine_tools_run_the_commands_against_the_configured_store() {
    let s = Setup::new();
    let root = s.fx.root();
    let transcripts = s.fx.home().join(".claude/projects");
    let config = json!({"root": root, "transcripts": transcripts, "task_workspaces": []});

    let reply = plugin(&s, "scribe.consistency-scan", &config);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(ids(&reply["output"]["opened"]), ["p-1", "p-2"]);
    assert_eq!(s.proposal("p-1")["state"], "open");
    let audit = s.audit();
    assert_eq!(audit.len(), 2);
    assert!(audit.iter().all(|l| l["command"] == "consistency scan"));

    let reply = plugin(&s, "scribe.session-pass", &config);
    assert_eq!(
        reply,
        json!({"ok": true, "output": {"passes": [], "remaining": 0}})
    );
    assert_eq!(s.audit().len(), 2, "nothing due: no audit line");

    // Unset settings, including the manifest's defaults, are refused.
    for (tool, config) in [
        (
            "scribe.session-pass",
            json!({"root": root, "transcripts": ""}),
        ),
        (
            "scribe.session-pass",
            json!({"root": "/var/tmp", "transcripts": transcripts}),
        ),
        ("scribe.consistency-scan", json!({"root": "/var/tmp"})),
        ("scribe.consistency-scan", json!({"root": "relative/path"})),
    ] {
        let reply = plugin(&s, tool, &config);
        assert_eq!(
            reply["error"]["code"], "not_configured",
            "{tool} {config}: {reply}"
        );
    }
}

// S10: the scan deactivates sessions by age and by removed feature.

const REMOVAL: &str = "The digest feature was ripped out and replaced by the weekly report.";

/// `days` before now, whole seconds.
fn days_ago(days: i64) -> time::OffsetDateTime {
    let at = time::OffsetDateTime::now_utc() - time::Duration::days(days);
    at.replace_nanosecond(0).unwrap()
}

/// RFC 3339.
fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

/// One format-31 session record: started `days_ago`, its one segment's last
/// message an hour later, `updated` `updated_ago` days ago.
struct SessionFile<'a> {
    id: &'a str,
    days_ago: i64,
    updated_ago: i64,
    title: &'a str,
    topics: &'a str,
    keep_active: bool,
    segment: &'a str,
}

impl Setup {
    fn session(&self, s: &SessionFile<'_>) {
        let started = days_ago(s.days_ago);
        let start = rfc3339(started);
        let last = rfc3339(started + time::Duration::hours(1));
        let text = format!(
            "---
format: 31
kind: session
id: {id}
status: final
active: true
keep_active: {keep}
active_change: null
cataloged_by: null
rebuilt_by: null
project: scribe
topics: [{topics}]
related: []
links: []
terms: []
provider: claude-code
host: host
created: {start}
updated: {updated}
title: {title}
cwd: /work
started: {start}
on_since: {start}
scribed_through: m-2
passes: 1
next_open: 0
log:
  - {{n: 1, heading: Work, first_message: m-1, last_message: m-2, from: {start}, to: {last}, state: sealed, windows: 1, origin: pass, topics: [{topics}], related: []}}
---
## Summary
Built the {title}.

## Goals
Ship it.

## Next

## Log
### 1. Work
{segment}
",
            id = s.id,
            keep = s.keep_active,
            topics = s.topics,
            title = s.title,
            updated = rfc3339(days_ago(s.updated_ago)),
            segment = s.segment,
        );
        support::seed_records(&self.fx.root(), &[&text]);
    }

    fn session_json(&self, id: &str) -> Value {
        self.json(&["session", "show", id, "--json"])
    }

    /// A fake `claude` that answers a removed-feature prompt with
    /// `removal` and any other with the entry pair's contradiction.
    fn answer_both(&self, removal: &Value) {
        let envelope = |body: &Value| {
            json!({"type": "result", "subtype": "success", "is_error": false,
                   "result": "", "structured_output": body})
        };
        self.fx.fake_program(
            "claude",
            &format!(
                "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\n\
                 if grep -q 'cataloged work sessions'; then echo '{}'; else echo '{}'; fi\n",
                envelope(removal),
                envelope(&contradicts()),
            ),
        );
    }

    /// A fake `claude` that corrupts one removal target after the scan has
    /// loaded its sessions, so the next target must still be processed.
    /// Answers both model calls; the removal call waits while the test
    /// damages `session`'s row, so its deactivation fails on a record that
    /// no longer reads. Join the returned hold after the scan.
    fn answer_both_corrupting(&self, removal: &Value, session: &'static str) -> support::Hold {
        let envelope = |body: &Value| {
            json!({"type": "result", "subtype": "success", "is_error": false,
                   "result": "", "structured_output": body})
        };
        let root = self.fx.root();
        let hold = support::Hold::new(&self.fx.home(), move || {
            support::sql(
                &root,
                &format!("UPDATE sessions SET project = '' WHERE provider_session = '{session}'"),
            );
        });
        self.fx.fake_program(
            "claude",
            &format!(
                "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\ncat > \"$HOME/claude-prompt\"\nif grep -q 'cataloged work sessions' \"$HOME/claude-prompt\"; then\n{hold}echo '{}'; else echo '{}'; fi\n",
                envelope(removal),
                envelope(&contradicts()),
                hold = hold.script(),
            ),
        );
        hold
    }
}

fn session_files() -> [SessionFile<'static>; 5] {
    [
        // Silent for 200 days: deactivated by age.
        SessionFile {
            id: "s-quiet",
            days_ago: 200,
            updated_ago: 200,
            title: "old export",
            topics: "export",
            keep_active: false,
            segment: "Wrote the export.",
        },
        // As old, but pinned: never touched.
        SessionFile {
            id: "s-pinned",
            days_ago: 200,
            updated_ago: 200,
            title: "store layout",
            topics: "store",
            keep_active: true,
            segment: "Chose the layout.",
        },
        // About the digest, which the later session says was removed.
        SessionFile {
            id: "s-a-digest",
            days_ago: 20,
            updated_ago: 20,
            title: "digest feature",
            topics: "digest",
            keep_active: false,
            segment: "Built the daily digest.",
        },
        // Also about the digest; the model's quote for it is invented.
        SessionFile {
            id: "s-b-digest",
            days_ago: 21,
            updated_ago: 21,
            title: "digest settings",
            topics: "digest",
            keep_active: false,
            segment: "Tuned the digest settings.",
        },
        // A later session whose model-written segment claims removal:
        // never evidence on its own (see `removal_entry`).
        SessionFile {
            id: "s-later",
            days_ago: 2,
            updated_ago: 1,
            title: "weekly report",
            topics: "digest",
            keep_active: false,
            segment: REMOVAL,
        },
    ]
}

/// Daniel's own entry saying the digest was removed, five days ago:
/// after the digest sessions and within the scan's 8-day window, but
/// before `s-later`'s last message so that session never pairs with it.
/// Its words name both digest sessions so each pairs by token overlap.
fn removal_entry(s: &Setup) {
    s.entry(
        "preferences-9",
        &rfc3339(days_ago(5)),
        "The digest feature was ripped out and replaced by the weekly \
         report; built the digest settings.",
        "active",
        None,
    );
}

#[test]
fn a_scan_deactivates_old_sessions_and_removed_features_but_never_a_pinned_one() {
    let s = Setup::new();
    for file in &session_files() {
        s.session(file);
    }
    removal_entry(&s);
    // Pair 1 is s-a-digest (ties on the later entry break by session
    // id), pair 2 s-b-digest, whose quote is not in the later entry.
    s.answer_both(&json!({"judgements": [
        {"pair": "1", "label": "removed", "feature": "the digest",
         "reason": "The weekly report replaced the digest.", "quote": "ripped out and replaced"},
        {"pair": "2", "label": "removed", "feature": "the digest",
         "reason": "Gone.", "quote": "the settings were deleted"},
    ]}));

    let report = s.scan();

    let deactivated = report["deactivated"].as_array().unwrap();
    assert_eq!(deactivated.len(), 2, "{report}");
    assert_eq!(deactivated[0]["session"], "s-quiet");
    assert_eq!(deactivated[0]["route"], "age");
    assert_eq!(deactivated[0]["cite"], Value::Null);
    assert!(
        deactivated[0]["reason"]
            .as_str()
            .unwrap()
            .ends_with("(limit 120 days)"),
        "{report}"
    );
    assert_eq!(deactivated[1]["session"], "s-a-digest");
    assert_eq!(deactivated[1]["route"], "removed");
    assert_eq!(deactivated[1]["cite"], "preferences-9");
    assert_eq!(deactivated[1]["quote"], "ripped out and replaced");
    assert_eq!(report["removal_pairs_judged"], 2);
    assert_eq!(report["removal_pairs_left"], 0);
    assert_eq!(report["dropped_removal_labels"], 1, "the invented quote");
    assert_eq!(report["failed_removal_batches"], json!([]));

    let quiet = s.session_json("s-quiet");
    assert_eq!(quiet["active"], false);
    assert_eq!(quiet["active_change"]["route"], "age");
    assert_eq!(quiet["active_change"]["by"]["kind"], "scribe");
    assert_eq!(quiet["active_change"]["by"]["name"], "consistency");
    let digest = s.session_json("s-a-digest");
    assert_eq!(digest["active"], false);
    assert_eq!(digest["active_change"]["route"], "removed");
    assert_eq!(digest["active_change"]["cite"], "preferences-9");
    assert_eq!(
        digest["active_change"]["reason"],
        "The weekly report replaced the digest."
    );
    for id in ["s-pinned", "s-b-digest", "s-later"] {
        let record = s.session_json(id);
        assert_eq!(record["active"], true, "{id}");
        assert_eq!(record["active_change"], Value::Null, "{id}");
    }

    let targets: Vec<String> = s
        .audit()
        .iter()
        .filter_map(|l| l["target"].as_str())
        .filter(|t| t.starts_with("session:"))
        .map(str::to_owned)
        .collect();
    assert_eq!(targets, ["session:s-quiet", "session:s-a-digest"]);

    // A dropped label is not stored: the next scan offers s-b-digest
    // again (now pair 1), and never the sessions already inactive.
    let calls = s.calls();
    let out = s.run(&["consistency", "scan", "--format", "auto"], b"");
    assert_eq!(out.status.code(), Some(0));
    assert!(s.calls() > calls);
    let rows: Vec<String> = text(&out.stdout)
        .lines()
        .filter(|l| l.starts_with("deactivated"))
        .map(str::to_owned)
        .collect();
    // `s-<n>` is the row number, so s-b-digest, seeded fourth, is `s-4`.
    assert_eq!(rows, ["deactivated\ts-4\tremoved\tpreferences-9\t-"]);
}

#[test]
fn a_failed_removal_deactivation_is_reported_and_other_sessions_continue() {
    let s = Setup::new();
    for file in &session_files() {
        s.session(file);
    }
    removal_entry(&s);
    let removal = json!({"judgements": [
        {"pair": "1", "label": "removed", "feature": "the digest",
         "reason": "The weekly report replaced the digest.", "quote": "ripped out and replaced"},
        {"pair": "2", "label": "removed", "feature": "the digest",
         "reason": "The weekly report replaced the digest.", "quote": "ripped out and replaced"},
    ]});
    let hold = s.answer_both_corrupting(&removal, "s-a-digest");

    let report = s.scan();
    hold.join();

    assert_eq!(
        report["failed_deactivations"],
        json!([{
            "session": "s-a-digest",
            "route": "removed",
            "error_code": "store_corrupt",
        }])
    );
    assert!(
        report["deactivated"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["session"] == "s-b-digest" && item["route"] == "removed" })
    );
    let audit = s.audit();
    assert!(audit.iter().any(|event| {
        event["target"] == "session:s-a-digest"
            && event["status"] == "failure"
            && event["error_code"] == "store_corrupt"
    }));
    assert!(
        audit.iter().any(|event| {
            event["target"] == "session:s-b-digest" && event["status"] == "success"
        })
    );
}

#[test]
fn deactivated_rows_print_in_plain_output() {
    let s = Setup::new();
    s.session(&session_files()[0]);
    let out = s.run(&["consistency", "scan", "--format", "auto"], b"");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let rows = text(&out.stdout);
    assert!(
        rows.lines().any(|l| l == "deactivated\ts-1\tage\t-\t-"),
        "{rows}"
    );
}

#[test]
fn a_shorter_configured_limit_deactivates_sooner() {
    let s = Setup::new();
    let [_, _, digest, ..] = session_files();
    let file = SessionFile {
        days_ago: 40,
        updated_ago: 40,
        ..digest
    };
    s.session(&file);
    std::fs::write(
        s.fx.root().join("config.toml"),
        "[sessions]\ninactive_after_days = 30\n",
    )
    .unwrap();
    make_private(&s.fx.root().join("config.toml"));
    let report = s.scan();
    assert_eq!(report["deactivated"][0]["session"], "s-a-digest");
    assert_eq!(report["deactivated"][0]["route"], "age");

    std::fs::write(
        s.fx.root().join("config.toml"),
        "[sessions]\ninactive_after_days = 0\n",
    )
    .unwrap();
    let out = s.run(&["consistency", "scan", "--json"], b"");
    assert_eq!(out.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "config_invalid");
}

#[test]
fn consistency_scan_closes_retired_tag_candidates_without_a_model_call() {
    let fx = Fixture::new();
    let path = fx.fake_program(
        "claude",
        "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\nexit 1\n",
    );
    let s = Setup {
        transcript: fx
            .home()
            .join(".claude/projects/-work-scribe")
            .join(format!("{SESSION}.jsonl")),
        cwd: fx.elsewhere("work/scribe"),
        fx,
        path,
    };
    let cite = |msg: &str| {
        json!({
            "session": SESSION,
            "message": msg,
            "at": "2026-11-15T00:00:00Z",
            "prompt_sha256": "0".repeat(64),
            "line": 1,
            "cwd": "/work/scribe"
        })
    };
    let mut initial = Vec::new();
    let mut n = 0usize;
    for (tag, count) in [
        ("decisions", 7),
        ("task", 7),
        ("finding", 2),
        ("preferences", 2),
        ("standard", 2),
    ] {
        for i in 1..=count {
            n += 1;
            let quote = format!("I always keep {tag} item {i} documented.");
            initial.push(json!({
                "id": format!("p-{n}"),
                "kind": "candidate",
                "older": format!("{tag}-{i}"),
                "newer": null,
                "candidate": {
                    "tag": tag,
                    "role": "user",
                    "quote": quote,
                    "evidence": null,
                    "cite": cite(&format!("seed-{n}")),
                    "project": "scribe",
                    "provider": "claude-code",
                    "host": "host",
                    "admitted": null
                },
                "reason": format!("Unmarked {tag} statement: {quote}"),
                "quotes": [],
                "state": "open",
                "opened_at": "2026-11-15T00:01:00Z",
                "settled_at": null,
                "settled_by": null,
                "author": {"kind": "scribe", "name": "session-pass"},
                "updated_by": null,
                "settled_author": null,
                "closed_reason": null
            }));
        }
    }
    s.write_proposals(&json!(initial));

    let report = s.scan();
    assert_eq!(s.calls(), 0, "retired-tag cleanup needs no model call");
    let entries = s.json(&["entry", "list", "--json"]);
    assert!(
        entries["entries"].as_array().unwrap().is_empty(),
        "no entry is written"
    );
    assert_eq!(report["closed"].as_array().unwrap().len(), 16, "{report}");
    for p in report["closed"].as_array().unwrap() {
        assert_eq!(p["state"], "rejected");
        assert_eq!(p["closed_reason"], "tag_retired");
        assert_eq!(
            p["settled_author"],
            json!({"kind": "scribe", "name": "tag-retired"})
        );
        assert!(p["settled_by"].is_null());
    }

    let audit = s.audit();
    assert_eq!(audit.len(), 16, "{audit:?}");
    assert!(
        audit
            .iter()
            .all(|l| l["command"] == "consistency scan" && l["status"] == "success")
    );
    let audit_targets: Vec<&str> = audit
        .iter()
        .map(|l| l["target"].as_str().unwrap())
        .collect();
    for i in 1..=16 {
        assert!(audit_targets.contains(&format!("p-{i}").as_str()));
    }

    // Running consistency scan again changes nothing more.
    let second = s.scan();
    assert_eq!(second["closed"], json!([]));
    assert_eq!(second["opened"], json!([]));
    assert_eq!(s.calls(), 0);
    assert_eq!(s.audit().len(), 16);

    // Open preferences and standard candidates remain untouched.
    let open = s.json(&["proposal", "list", "--state", "open", "--json"]);
    assert_eq!(open["total"], 4);
    let rejected_table = text(
        &s.fx
            .ok(&[
                "proposal", "list", "--state", "rejected", "--format", "table",
            ])
            .stdout,
    );
    assert!(rejected_table.contains("CLOSED_REASON"), "{rejected_table}");
    assert_eq!(
        rejected_table.matches("tag_retired").count(),
        16,
        "{rejected_table}"
    );
}

#[test]
fn a_scan_that_fails_after_persisting_closures_still_audits_each_one() {
    let fx = Fixture::new();
    let path = fx.fake_program(
        "claude",
        "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\nexit 1\n",
    );
    let s = Setup {
        transcript: fx
            .home()
            .join(".claude/projects/-work-scribe")
            .join(format!("{SESSION}.jsonl")),
        cwd: fx.elsewhere("work/scribe"),
        fx,
        path,
    };
    let proposal = json!({
        "id": "p-1",
        "kind": "candidate",
        "older": "decisions-1",
        "newer": null,
        "candidate": {
            "tag": "decisions",
            "role": "user",
            "quote": "I always keep decisions item 1 documented.",
            "evidence": null,
            "cite": {
                "session": SESSION,
                "message": "seed-1",
                "at": "2026-11-15T00:00:00Z",
                "prompt_sha256": "0".repeat(64),
                "line": 1,
                "cwd": "/work/scribe"
            },
            "project": "scribe",
            "provider": "claude-code",
            "host": "host",
            "admitted": null
        },
        "reason": "Unmarked decisions statement.",
        "quotes": [],
        "state": "open",
        "opened_at": "2026-11-15T00:01:00Z",
        "settled_at": null,
        "settled_by": null,
        "author": {"kind": "scribe", "name": "session-pass"},
        "updated_by": null,
        "settled_author": null,
        "closed_reason": null
    });
    s.write_proposals(&json!([proposal]));

    // The seeded files move into scribe.db first, then a missing sessions
    // table is invisible to the close (that reads proposals only) and fails
    // list_sessions afterwards, deterministically.
    break_sessions(&s);

    let out = s.run(&["consistency", "scan", "--json"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "store_database", "{err}");
    assert_eq!(s.calls(), 0, "the failure is before any model call");

    mend_sessions(&s);
    let stored = support::proposals_doc(&s.fx.root());
    assert_eq!(stored["proposals"][0]["state"], "rejected");
    assert_eq!(stored["proposals"][0]["closed_reason"], "tag_retired");

    let scan_lines: Vec<Value> = s
        .audit()
        .into_iter()
        .filter(|line| line["command"] == "consistency scan")
        .collect();
    let closures = scan_lines
        .iter()
        .filter(|line| line["status"] == "success" && line["target"] == "p-1")
        .count();
    assert_eq!(closures, 1, "{scan_lines:?}");
    let failures = scan_lines
        .iter()
        .filter(|line| line["status"] == "failure" && line["error_code"] == "store_database")
        .count();
    assert_eq!(failures, 1, "{scan_lines:?}");
}

#[test]
fn consistency_scan_with_custom_tag_config_keeps_open_preferences_candidates() {
    let fx = Fixture::new();
    let path = fx.fake_program(
        "claude",
        "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\nexit 1\n",
    );
    let s = Setup {
        transcript: fx
            .home()
            .join(".claude/projects/-work-scribe")
            .join(format!("{SESSION}.jsonl")),
        cwd: fx.elsewhere("work/scribe"),
        fx,
        path,
    };
    std::fs::create_dir_all(s.fx.root()).unwrap();
    make_private(&s.fx.root());
    let config = s.fx.root().join("config.toml");
    std::fs::write(&config, "[tags.remind]\naction = true\n").unwrap();
    make_private(&config);

    let candidate = |n: usize, tag: &str| {
        let quote = format!("I always keep {tag} item {n} documented.");
        json!({
            "id": format!("p-{n}"),
            "kind": "candidate",
            "older": format!("{tag}-{n}"),
            "newer": null,
            "candidate": {
                "tag": tag,
                "role": "user",
                "quote": quote,
                "evidence": null,
                "cite": {
                    "session": SESSION,
                    "message": format!("seed-{n}"),
                    "at": "2026-11-15T00:00:00Z",
                    "prompt_sha256": "0".repeat(64),
                    "line": 1,
                    "cwd": "/work/scribe"
                },
                "project": "scribe",
                "provider": "claude-code",
                "host": "host",
                "admitted": null
            },
            "reason": format!("Unmarked {tag} statement: {quote}"),
            "quotes": [],
            "state": "open",
            "opened_at": "2026-11-15T00:01:00Z",
            "settled_at": null,
            "settled_by": null,
            "author": {"kind": "scribe", "name": "session-pass"},
            "updated_by": null,
            "settled_author": null,
            "closed_reason": null
        })
    };
    s.write_proposals(&json!([
        candidate(1, "preferences"),
        candidate(2, "decisions")
    ]));

    let report = s.scan();
    assert_eq!(s.calls(), 0);
    let closed = report["closed"].as_array().unwrap();
    assert_eq!(closed.len(), 1, "{report}");
    assert_eq!(closed[0]["id"], "p-2");
    assert_eq!(closed[0]["closed_reason"], "tag_retired");

    let open = s.json(&["proposal", "list", "--state", "open", "--json"]);
    assert_eq!(open["total"], 1, "{open}");
    assert_eq!(open["proposals"][0]["id"], "p-1");
    assert_eq!(open["proposals"][0]["candidate"]["tag"], "preferences");
}

#[test]
fn a_scan_that_fails_after_reapply_and_open_still_audits_each_write() {
    let fx = Fixture::new();
    let path = fx.fake_program(
        "claude",
        "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\nexit 1\n",
    );
    let s = Setup {
        transcript: fx
            .home()
            .join(".claude/projects/-work-scribe")
            .join(format!("{SESSION}.jsonl")),
        cwd: fx.elsewhere("work/scribe"),
        fx,
        path,
    };
    // Active entries:
    // - preferences-1 will be superseded when p-1 is reapplied
    // - preferences-2 covers approved candidate p-2 (closed as covered)
    // - rituals-1 is past review_after so open_proposals opens p-4
    s.entry(
        "preferences-1",
        "2026-01-02T03:04:05Z",
        OLDER,
        "active",
        None,
    );
    s.entry(
        "preferences-2",
        "2026-02-02T03:04:05Z",
        NEWER,
        "active",
        None,
    );
    s.entry(
        "rituals-1",
        "2026-01-05T03:04:05Z",
        RITUAL,
        "active",
        Some("2026-03-01"),
    );
    let cite = |msg: &str| {
        json!({
            "session": SESSION,
            "message": msg,
            "at": "2026-11-15T00:00:00Z",
            "prompt_sha256": "0".repeat(64),
            "line": 1,
            "cwd": "/work/scribe"
        })
    };
    let doc = json!({
        "proposals": [
            {
                "id": "p-1",
                "kind": "supersedes",
                "older": "preferences-1",
                "newer": "preferences-2",
                "candidate": null,
                "reason": "Newer replaces older.",
                "quotes": [],
                "state": "approved",
                "opened_at": "2026-11-15T00:00:00Z",
                "settled_at": "2026-11-15T00:05:00Z",
                "settled_by": cite("u-approve-1"),
                "author": {"kind": "scribe", "name": "consistency"},
                "updated_by": null,
                "settled_author": {"kind": "daniel", "name": "daniel"},
                "closed_reason": null
            },
            {
                "id": "p-2",
                "kind": "candidate",
                "older": "preferences-3",
                "newer": null,
                "candidate": {
                    "tag": "preferences",
                    "role": "user",
                    "quote": NEWER,
                    "evidence": null,
                    "cite": cite("u-cand-2"),
                    "project": "scribe",
                    "provider": "claude-code",
                    "host": "host",
                    "admitted": null
                },
                "reason": "Covered approved candidate.",
                "quotes": [],
                "state": "approved",
                "opened_at": "2026-11-15T00:01:00Z",
                "settled_at": "2026-11-15T00:06:00Z",
                "settled_by": cite("u-approve-2"),
                "author": {"kind": "scribe", "name": "session-pass"},
                "updated_by": null,
                "settled_author": {"kind": "daniel", "name": "daniel"},
                "closed_reason": null
            },
            {
                "id": "p-3",
                "kind": "candidate",
                "older": "decisions-1",
                "newer": null,
                "candidate": {
                    "tag": "decisions",
                    "role": "user",
                    "quote": "I always keep decisions item 1 documented.",
                    "evidence": null,
                    "cite": cite("u-cand-3"),
                    "project": "scribe",
                    "provider": "claude-code",
                    "host": "host",
                    "admitted": null
                },
                "reason": "Retired tag candidate.",
                "quotes": [],
                "state": "open",
                "opened_at": "2026-11-15T00:02:00Z",
                "settled_at": null,
                "settled_by": null,
                "author": {"kind": "scribe", "name": "session-pass"},
                "updated_by": null,
                "settled_author": null,
                "closed_reason": null
            }
        ]
    });
    s.write_proposals(&doc["proposals"]);

    // A missing segments table fails deactivate_sessions AFTER
    // reapply_approved, close_retired_candidates, and open_proposals have all
    // persisted.
    break_sessions(&s);

    let out = s.run(&["consistency", "scan", "--json"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "store_database", "{err}");
    assert_eq!(
        s.calls(),
        0,
        "stale-only open_proposals needs no model call"
    );
    mend_sessions(&s);

    let scan_lines: Vec<Value> = s
        .audit()
        .into_iter()
        .filter(|line| line["command"] == "consistency scan")
        .collect();
    for expected in ["preferences-1", "p-2", "p-3", "p-4"] {
        assert_eq!(
            scan_lines
                .iter()
                .filter(|line| line["status"] == "success" && line["target"] == expected)
                .count(),
            1,
            "expected persisted write {expected} to be audited once: {scan_lines:?}"
        );
    }
    assert_eq!(
        scan_lines
            .iter()
            .filter(|line| line["status"] == "failure" && line["error_code"] == "store_database")
            .count(),
        1,
        "{scan_lines:?}"
    );
}

#[test]
fn a_scan_that_fails_after_age_deactivation_still_audits_the_deactivated_session() {
    let s = Setup::new();
    for file in &session_files() {
        s.session(file);
    }
    removal_entry(&s);
    let removal = json!({"judgements": [
        {"pair": "1", "label": "removed", "feature": "the digest",
         "reason": "The weekly report replaced the digest.", "quote": "ripped out and replaced"},
    ]});
    let envelope = |body: &Value| {
        json!({"type": "result", "subtype": "success", "is_error": false,
               "result": "", "structured_output": body})
    };
    // On the removal call (after age deactivation has persisted `s-quiet`),
    // the segments table is renamed away, so the removal `set_active_many`
    // cannot read the sessions and the scan fails.
    let root = s.fx.root();
    let hold = support::Hold::new(&s.fx.home(), move || {
        support::sql(&root, "ALTER TABLE segments RENAME TO segments_gone;");
    });
    s.fx.fake_program(
        "claude",
        &format!(
            "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\ncat > \"$HOME/claude-prompt\"\nif grep -q 'cataloged work sessions' \"$HOME/claude-prompt\"; then\n{hold}echo '{}'; else echo '{}'; fi\n",
            envelope(&removal),
            envelope(&contradicts()),
            hold = hold.script(),
        ),
    );

    let out = s.run(&["consistency", "scan", "--json"], b"");
    hold.join();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "store_database", "{err}");
    support::sql(
        &s.fx.root(),
        "ALTER TABLE segments_gone RENAME TO segments;",
    );

    let scan_lines: Vec<Value> = s
        .audit()
        .into_iter()
        .filter(|line| line["command"] == "consistency scan")
        .collect();
    assert_eq!(
        scan_lines
            .iter()
            .filter(|line| line["status"] == "success" && line["target"] == "session:s-quiet")
            .count(),
        1,
        "aged session deactivation must be audited even when removal write fails: {scan_lines:?}"
    );
    assert_eq!(
        scan_lines
            .iter()
            .filter(|line| line["status"] == "failure" && line["error_code"] == "store_database")
            .count(),
        1,
        "{scan_lines:?}"
    );
}
