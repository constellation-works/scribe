use crate::support;
use crate::support::command::Exit::{Capture, Success};
use crate::support::command::with_env;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, make_private, text};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Output;

pub(super) const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";

pub(super) const OLDER: &str = "squash-merge agent PRs into agent-main";

pub(super) const NEWER: &str = "rebase-merge agent PRs into agent-main, never squash";

pub(super) const RITUAL: &str = "run the full gate before every push";

/// The nine counters the scan report kept from before ORB-27911 step 2a.
/// No pair is judged any more, so each is neutral; removing one would break
/// `--json` (STD-01@2 §R10).
pub(super) const RETIRED_COUNTERS: [&str; 9] = [
    "judged_pairs",
    "pairs_left",
    "unrelated_pairs",
    "dropped_labels",
    "failed_batches",
    "removal_pairs_judged",
    "removal_pairs_left",
    "dropped_removal_labels",
    "failed_removal_batches",
];

const REMOVAL: &str = "The digest feature was ripped out and replaced by the weekly report.";

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input
            .into()
            .run_with(self, &["consistency"], expect, |fx, cmd| {
                cmd.env("PATH", fx.program_path());
            })
    }
}

pub(super) struct Setup {
    pub(super) fx: Fixture,
    pub(super) path: OsString,
    pub(super) transcript: PathBuf,
    pub(super) cwd: PathBuf,
}

impl Setup {
    /// A store of two contradicting preferences and a ritual past its review
    /// date, and a fake `claude` that labels pair 1 a contradiction.
    pub(super) fn new() -> Self {
        let fx = Fixture::new();
        let project = fx.home().join(".claude/projects/-work-scribe");
        std::fs::create_dir_all(&project).unwrap();
        let cwd = fx.elsewhere("work/scribe");
        std::fs::create_dir_all(&cwd).unwrap();
        let path = fx.fake_program("claude", "#!/bin/sh\nexit 1\n");
        let transcript = project.join(format!("{SESSION}.jsonl"));
        let setup = Self {
            fx,
            path,
            transcript,
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

    pub(super) fn entry(
        &self,
        id: &str,
        created: &str,
        content: &str,
        status: &str,
        review: Option<&str>,
    ) {
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

    pub(super) fn calls(&self) -> usize {
        std::fs::read_to_string(self.fx.home().join("claude-calls"))
            .map_or(0, |t| t.lines().count())
    }

    #[track_caller]
    pub(super) fn run(&self, args: &[&str], input: &[u8]) -> Output {
        self.fx
            .check(with_env(args, "PATH", &self.path), Capture.stdin(input))
    }

    #[track_caller]
    pub(super) fn scan(&self) -> Value {
        let out = self.fx.check(&["scan", "--json"], Success.stdin(b""));
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    #[track_caller]
    pub(super) fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args, b"");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    pub(super) fn proposal(&self, id: &str) -> Value {
        let list = self.json(&["proposal", "list", "--json"]);
        list["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no {id} in {list}"))
    }

    pub(super) fn entry_json(&self, id: &str) -> Value {
        self.json(&["entry", "show", id, "--json"])
    }

    pub(super) fn audit(&self) -> Vec<Value> {
        audit_in(&self.fx.root())
    }

    /// Type `prompt` in the session: the transcript holds it, then the hook
    /// runs. Returns the line Daniel sees.
    pub(super) fn type_prompt(&self, prompt: &str) -> String {
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

    pub(super) fn session_start(&self) -> String {
        let payload = json!({"session_id": SESSION, "transcript_path": self.transcript,
            "cwd": self.cwd, "hook_event_name": "SessionStart", "source": "startup"});
        let out = self.run(&["hook", "session-start"], payload.to_string().as_bytes());
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        text(&out.stdout)
    }

    pub(super) fn write_proposals(&self, proposals: &Value) {
        seed_proposals(&self.fx.root(), proposals);
    }
}

impl Setup {
    pub(super) fn session(&self, s: &SessionFile<'_>) {
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

    pub(super) fn session_json(&self, id: &str) -> Value {
        self.json(&["session", "show", id, "--json"])
    }
}

/// One format-31 session record: started `days_ago`, its one segment's last
/// message an hour later, `updated` `updated_ago` days ago.
pub(super) struct SessionFile<'a> {
    pub(super) id: &'a str,
    pub(super) days_ago: i64,
    pub(super) updated_ago: i64,
    pub(super) title: &'a str,
    pub(super) topics: &'a str,
    pub(super) keep_active: bool,
    pub(super) segment: &'a str,
}

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
pub(super) fn break_sessions(s: &Setup) {
    support::sql(
        &s.fx.root(),
        "ALTER TABLE segments RENAME TO segments_gone;",
    );
}

pub(super) fn mend_sessions(s: &Setup) {
    support::sql(
        &s.fx.root(),
        "ALTER TABLE segments_gone RENAME TO segments;",
    );
}

fn audit_in(root: &Path) -> Vec<Value> {
    support::audit_lines(root)
}

pub(super) fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .collect()
}

/// Run `plugin call` for `tool` with `config`, without `SCRIBE_ROOT`.
pub(super) fn plugin(s: &Setup, tool: &str, config: &Value) -> Value {
    let envelope = json!({"schema_version": 1, "tool": tool, "input": {},
        "context": {"config": config}});
    let out = s.fx.check(
        s.fx.cmd(&["plugin", "call"])
            .env_remove("SCRIBE_ROOT")
            .env("PATH", &s.path),
        Success.stdin(envelope.to_string().as_bytes()),
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// `days` before now, whole seconds.
pub(super) fn days_ago(days: i64) -> time::OffsetDateTime {
    let at = time::OffsetDateTime::now_utc() - time::Duration::days(days);
    at.replace_nanosecond(0).unwrap()
}

/// RFC 3339.
fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

pub(super) fn session_files() -> [SessionFile<'static>; 5] {
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
        // About the digest, recent: stays active.
        SessionFile {
            id: "s-a-digest",
            days_ago: 20,
            updated_ago: 20,
            title: "digest feature",
            topics: "digest",
            keep_active: false,
            segment: "Built the daily digest.",
        },
        // Also about the digest, recent: stays active.
        SessionFile {
            id: "s-b-digest",
            days_ago: 21,
            updated_ago: 21,
            title: "digest settings",
            topics: "digest",
            keep_active: false,
            segment: "Tuned the digest settings.",
        },
        // A later session whose segment says the digest was removed: no
        // scan deactivates a session for that.
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
