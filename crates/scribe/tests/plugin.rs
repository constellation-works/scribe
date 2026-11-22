//! End-to-end tests of `scribe plugin call`, the Orbit exec backend
//! (docs/design/scribe/2_design.md §1, §6.1, §10 S5): `scribe.ask` returns
//! the payload `question ask --json` prints for the same store, and every
//! other outcome is one error envelope with exit 0. The manifest's
//! contract tests are compiled in from the plugin tree.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

#[path = "../../../plugin/.orbit-plugin/tests/conformance.rs"]
mod conformance;

use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::Write;
use std::process::{Output, Stdio};
use support::{Fixture, text};

const CLAUDE: &str = "#!/bin/sh\n\
    echo call >> \"$HOME/claude-calls\"\n\
    cat > /dev/null\n\
    printf '%s\\n' '{\"type\":\"result\",\"is_error\":false,\"result\":\"\",\"structured_output\":{\"status\":\"answered\",\"answer\":\"Squash.\",\"claims\":[{\"text\":\"squash\",\"quote\":\"squash-merge agent PRs;\",\"cite\":\"preferences-2\"}],\"conflicts\":[]}}'\n";

struct Setup {
    fx: Fixture,
    path: OsString,
}

impl Setup {
    fn new() -> Self {
        let fx = Fixture::new();
        fx.seed();
        let path = fx.fake_program("claude", CLAUDE);
        Self { fx, path }
    }

    /// Run `plugin call` with `envelope` on stdin. `SCRIBE_ROOT` is removed:
    /// the tool's store comes from the envelope alone.
    fn call(&self, envelope: &[u8]) -> Output {
        let mut cmd = self.fx.cmd(&["plugin", "call"]);
        cmd.env_remove("SCRIBE_ROOT")
            .env("PATH", &self.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = support::ChildGuard::new(cmd.spawn().unwrap());
        child
            .child()
            .stdin
            .take()
            .unwrap()
            .write_all(envelope)
            .unwrap();
        child.finish()
    }

    /// The reply envelope for `tool` with `input` and `config`; exit 0 and
    /// one line, whatever the outcome.
    fn reply(&self, tool: &str, input: Value, config: Value) -> Value {
        let envelope = json!({
            "schema_version": 1, "tool": tool, "input": input,
            "context": {"workspace_root": "/nonexistent", "agent": "claude", "model": null,
                        "config": config, "task_id": null, "job_run_id": null}
        });
        self.reply_raw(envelope.to_string().as_bytes())
    }

    fn reply_raw(&self, envelope: &[u8]) -> Value {
        let out = self.call(envelope);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let stdout = text(&out.stdout);
        assert_eq!(stdout.lines().count(), 1, "{stdout}");
        serde_json::from_str(&stdout).unwrap()
    }

    fn root_config(&self) -> Value {
        json!({"root": self.fx.root().to_str().unwrap(), "task_workspaces": []})
    }

    fn cli(&self, question: &str) -> Value {
        let mut cmd = self.fx.cmd(&["question", "ask", question, "--json"]);
        cmd.env("PATH", &self.path);
        let out = cmd.output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

fn error_code(reply: &Value) -> &str {
    assert_eq!(reply["ok"], false, "{reply}");
    reply["error"]["code"].as_str().unwrap()
}

#[test]
fn ask_returns_the_cli_payload_for_the_same_store() {
    let s = Setup::new();
    for question in [
        "can you fetch me last session summary?",
        "how do we merge agent PRs?",
        "zebra quantum chromodynamics",
    ] {
        let reply = s.reply("scribe.ask", json!({"question": question}), s.root_config());
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["output"], s.cli(question), "{question}");
    }
}

#[test]
fn ask_passes_scope_and_include_hidden_as_the_flags_do() {
    let s = Setup::new();
    let reply = s.reply(
        "scribe.ask",
        json!({"question": "how do we merge agent PRs?",
               "scope": {"tags": ["dec"], "since": "2026-01-01"}, "include_hidden": true}),
        s.root_config(),
    );
    let mut cmd = s.fx.cmd(&[
        "question",
        "ask",
        "how do we merge agent PRs?",
        "--tag",
        "dec",
        "--since",
        "2026-01-01",
        "--include-hidden",
        "--json",
    ]);
    cmd.env("PATH", &s.path);
    let cli: Value = serde_json::from_slice(&cmd.output().unwrap().stdout).unwrap();
    assert_eq!(reply["output"], cli);
    assert_eq!(cli["hidden_matches"], 0);
}

#[test]
fn ask_without_an_absolute_store_root_is_not_configured() {
    let s = Setup::new();
    for config in [
        json!({}),
        json!({"root": ""}),
        json!({"root": "relative/store"}),
    ] {
        let reply = s.reply("scribe.ask", json!({"question": "anything"}), config);
        assert_eq!(error_code(&reply), "not_configured");
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains("[plugins.scribe] root")
        );
    }
}

#[test]
fn bad_input_is_invalid_input() {
    let s = Setup::new();
    for input in [
        json!({}),
        json!({"question": ""}),
        json!({"question": "x", "limit": 3}),
        json!({"question": "x", "scope": {"kind": "discussion"}}),
        json!({"question": "x", "scope": {"since": "last week"}}),
        json!({"question": "x", "scope": {"tags": ["Not A Tag"]}}),
    ] {
        let reply = s.reply("scribe.ask", input.clone(), s.root_config());
        assert_eq!(error_code(&reply), "invalid_input", "{input}");
    }
}

#[test]
fn unknown_tools_are_refused() {
    let s = Setup::new();
    let reply = s.reply("scribe.delete-everything", json!({}), s.root_config());
    assert_eq!(error_code(&reply), "unknown_tool");
}

#[test]
fn the_routine_tools_refuse_any_input_key() {
    let s = Setup::new();
    for tool in [
        "scribe.session-pass",
        "scribe.consistency-scan",
        "scribe.memory-refresh",
    ] {
        let reply = s.reply(tool, json!({"limit": 1}), s.root_config());
        assert_eq!(error_code(&reply), "invalid_input", "{tool}");
    }
}

#[test]
fn a_malformed_envelope_is_one_error_envelope() {
    let s = Setup::new();
    assert_eq!(error_code(&s.reply_raw(b"not json")), "invalid_envelope");
    assert_eq!(
        error_code(&s.reply_raw(br#"{"schema_version":2,"tool":"scribe.ask","input":{}}"#)),
        "invalid_envelope"
    );
}

#[test]
fn a_model_failure_is_a_retryable_error_envelope() {
    let s = Setup::new();
    s.fx.fake_program("claude", "#!/bin/sh\ncat >/dev/null\nexit 3\n");
    let reply = s.reply(
        "scribe.ask",
        json!({"question": "how do we merge agent PRs?"}),
        s.root_config(),
    );
    assert_eq!(error_code(&reply), "model_failed");
    assert_eq!(reply["error"]["retryable"], true);
}

#[test]
fn a_call_is_not_audited() {
    let s = Setup::new();
    s.reply(
        "scribe.ask",
        json!({"question": "can you fetch me last session summary?"}),
        s.root_config(),
    );
    assert!(support::audit_lines(&s.fx.root()).is_empty());
}
