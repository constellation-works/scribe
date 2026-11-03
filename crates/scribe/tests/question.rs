//! End-to-end tests of `scribe question ask` through the built binary
//! (docs/design/scribe/2_design.md §6, §10 S5): lookups answer from the
//! stored records with no model call, anything else goes through retrieval
//! and a fake `claude`, and nothing is written.
//!
//! The fake `claude` counts its calls in `$HOME/claude-calls`, keeps its
//! last stdin and answers with `$HOME/claude-answer.json`; the fake `orbit`
//! answers `orbit.search` and `orbit.task.show` from fixtures. No test runs
//! either real program or touches the real home.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::{Value, json};
use std::ffi::OsString;
use std::process::Output;
use support::{Fixture, snapshot, text};

const NEWEST: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

const CLAUDE: &str = "#!/bin/sh\n\
    echo call >> \"$HOME/claude-calls\"\n\
    cat > \"$HOME/claude-stdin\"\n\
    printf '{\"type\":\"result\",\"is_error\":false,\"result\":\"\",\"structured_output\":%s}\\n' \"$(cat \"$HOME/claude-answer.json\")\"\n";

struct Setup {
    fx: Fixture,
    path: OsString,
}

impl Setup {
    fn new() -> Self {
        let fx = Fixture::new();
        fx.seed();
        let path = fx.fake_program("claude", CLAUDE);
        let setup = Self { fx, path };
        setup.answer(&json!({
            "status": "answered",
            "answer": "Agent PRs are squash-merged.",
            "claims": [
                {"text": "squash", "quote": "squash-merge agent PRs;", "cite": "decisions-2"},
                {"text": "invented", "quote": "rebase every agent PR", "cite": "decisions-2"}
            ],
            "conflicts": []
        }));
        setup
    }

    fn answer(&self, body: &Value) {
        std::fs::write(self.fx.home().join("claude-answer.json"), body.to_string()).unwrap();
    }

    fn ask(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut all = vec!["question", "ask"];
        all.extend_from_slice(args);
        let mut cmd = self.fx.cmd(&all);
        cmd.env("PATH", &self.path);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn json(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let out = self.ask(&all, env);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn model_calls(&self) -> usize {
        std::fs::read_to_string(self.fx.home().join("claude-calls"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn stored_session(&self, id: &str) -> Value {
        let out = self.fx.ok(&["session", "show", id, "--json"]);
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

#[test]
fn last_session_summary_is_the_newest_record_verbatim_without_a_model_call() {
    let s = Setup::new();
    let before = snapshot(&s.fx.root());
    let v = s.json(&["can you fetch me last session summary?"], &[]);
    assert_eq!(v["status"], "record");
    assert_eq!(v["lookup"], "last_session");
    assert_eq!(v["evidence_considered"], 1);
    let records = v["records"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["kind"], "session");
    assert_eq!(records[0]["cite"], format!("session:{NEWEST}"));
    assert_eq!(records[0]["session"], s.stored_session(NEWEST));
    assert_eq!(s.model_calls(), 0);
    assert_eq!(snapshot(&s.fx.root()), before, "ask wrote to the store");
}

#[test]
fn a_question_that_is_not_a_lookup_is_answered_from_checked_evidence() {
    let s = Setup::new();
    let before = snapshot(&s.fx.root());
    let v = s.json(&["how do we merge agent PRs?"], &[]);
    assert_eq!(s.model_calls(), 1);
    assert_eq!(v["status"], "answered");
    assert_eq!(v["lookup"], Value::Null);
    assert_eq!(v["dropped_quotes"], 1);
    assert_eq!(v["claims"].as_array().unwrap().len(), 1);
    assert_eq!(v["claims"][0]["cite"], "decisions-2");
    // decisions-1 is superseded: counted, not shown to the model.
    assert_eq!(v["hidden_matches"], 1);
    let prompt = std::fs::read_to_string(s.fx.home().join("claude-stdin")).unwrap();
    assert!(prompt.contains("cite=\"decisions-2\""), "{prompt}");
    assert!(!prompt.contains("cite=\"decisions-1\""), "{prompt}");
    assert_eq!(snapshot(&s.fx.root()), before, "ask wrote to the store");
}

#[test]
fn include_hidden_gives_the_model_superseded_entries_too() {
    let s = Setup::new();
    let v = s.json(&["how do we merge agent PRs?", "--include-hidden"], &[]);
    assert_eq!(v["hidden_matches"], 0);
    let prompt = std::fs::read_to_string(s.fx.home().join("claude-stdin")).unwrap();
    assert!(prompt.contains("cite=\"decisions-1\""), "{prompt}");
}

#[test]
fn nothing_matching_is_no_record_without_a_model_call() {
    let s = Setup::new();
    let v = s.json(&["zebra quantum chromodynamics"], &[]);
    assert_eq!(v["status"], "no_record");
    assert_eq!(v["evidence_considered"], 0);
    assert_eq!(s.model_calls(), 0);
}

#[test]
fn a_term_returns_the_sessions_that_define_it_without_a_model_call() {
    let s = Setup::new();
    let path = s.fx.root().join(format!("sessions/2026/11/{NEWEST}.md"));
    let stored = std::fs::read_to_string(&path).unwrap();
    let with_term = stored.replace(
        "terms:\n",
        "terms:\n  - {term: graduate to level 4, said: \"graduate to level 4 means a crew merges without review\", message: 9a1b2c3d-0000-4000-8000-000000000003}\n",
    );
    assert_ne!(stored, with_term);
    std::fs::write(&path, with_term).unwrap();
    let v = s.json(&["graduate to level 4"], &[]);
    assert_eq!(v["status"], "record");
    assert_eq!(v["lookup"], "term");
    let record = &v["records"][0];
    assert_eq!(record["cite"], format!("session:{NEWEST}"));
    assert_eq!(
        record["terms"],
        json!([{
            "term": "graduate to level 4",
            "said": "graduate to level 4 means a crew merges without review",
            "message": "9a1b2c3d-0000-4000-8000-000000000003"
        }])
    );
    assert_eq!(record["session"], s.stored_session(NEWEST));
    assert_eq!(s.model_calls(), 0);
}

#[test]
fn the_last_on_call_session_is_the_newest_task_summary_verbatim_without_a_model_call() {
    let s = Setup::new();
    let summary = "## Outcome\n\nS1 to S4 landed; the board is clear.\n";
    let show = json!({
        "id": "T-2", "title": "[on-call]: orbit - opus (2026-11-01)", "status": "done",
        "tags": ["orchestration", "on-call"], "updated_at": "2026-11-01T10:00:00Z",
        "description": "the brief", "execution_summary": summary
    });
    let search = json!({"mode": "lexical", "kind": "task", "notes": [], "results": [
        {"kind": "task", "source": "lexical", "id": "T-2", "title": "t", "summary": "s", "status": "done"}
    ]});
    std::fs::write(s.fx.home().join("show.json"), show.to_string()).unwrap();
    std::fs::write(s.fx.home().join("search.json"), search.to_string()).unwrap();
    s.fx.fake_program(
        "orbit",
        "#!/bin/sh\n\
         echo \"$@\" >> \"$HOME/orbit-calls\"\n\
         case \"$3\" in orbit.search) cat \"$HOME/search.json\";; orbit.task.show) cat \"$HOME/show.json\";; esac\n",
    );
    let orbit = s.fx.elsewhere("bin/orbit");
    let v = s.json(
        &["last on-call session for orbit"],
        &[("ORBIT_BIN", orbit.to_str().unwrap())],
    );
    assert_eq!(v["status"], "record", "{v}");
    assert_eq!(v["lookup"], "orchestrator_session");
    assert_eq!(v["records"][0]["cite"], "task:T-2");
    assert_eq!(v["records"][0]["task"]["execution_summary"], summary);
    assert_eq!(s.model_calls(), 0);
    let calls = std::fs::read_to_string(s.fx.home().join("orbit-calls")).unwrap();
    assert!(calls.contains("\"tag\":[\"orchestration\"]"), "{calls}");
}

#[test]
fn a_model_failure_is_an_error_with_its_code() {
    let s = Setup::new();
    s.fx.fake_program(
        "claude",
        "#!/bin/sh\ncat >/dev/null\necho broken >&2\nexit 3\n",
    );
    let out = s.ask(&["how do we merge agent PRs?", "--json"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err: Value =
        serde_json::from_slice(text(&out.stderr).lines().last().unwrap().as_bytes()).unwrap();
    assert_eq!(err["code"], "model_failed");
}

#[test]
fn a_blank_question_or_a_bad_date_is_a_usage_error() {
    let s = Setup::new();
    assert_eq!(s.ask(&["   "], &[]).status.code(), Some(2));
    assert_eq!(
        s.ask(&["merge", "--since", "yesterday"], &[]).status.code(),
        Some(2)
    );
    let long = "x".repeat(2_001);
    assert_eq!(s.ask(&[&long], &[]).status.code(), Some(2));
}

#[test]
fn the_table_view_shows_the_record_in_full() {
    let s = Setup::new();
    let out = s.ask(
        &[
            "can you fetch me last session summary?",
            "--format",
            "table",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(0));
    let shown = text(&out.stdout);
    assert!(shown.contains("last_session"), "{shown}");
    assert!(
        shown.contains("Squash-merge agent PRs; releases keep merge commits."),
        "{shown}"
    );
}
