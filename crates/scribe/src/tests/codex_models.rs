//! Codex transport boundary tests against controlled children, never a live account.
#![cfg(unix)]
use crate::models::{ClaudeCliModels, allowed_env};
use scribe_core::{
    Models, Provider,
    models::{ModelError, ReferentRequest},
    pass,
};
use std::{
    ffi::OsString,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

struct Fake {
    dir: tempfile::TempDir,
}
impl Fake {
    fn new(body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let path = bin.join("codex");
        std::fs::write(&path, format!("#!/bin/sh\ncat >/dev/null\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir }
    }
    fn model(&self) -> ClaudeCliModels {
        let path = format!("{}:/usr/bin:/bin", self.dir.path().join("bin").display());
        ClaudeCliModels::new(allowed_env(&[], |key| match key {
            "PATH" => Some(OsString::from(&path)),
            "HOME" => Some(self.dir.path().into()),
            _ => None,
        }))
        .with_provider(Provider::Codex)
    }
}
fn request() -> ReferentRequest {
    ReferentRequest {
        model: String::from("explicit-model"),
        budget: Duration::from_secs(5),
        instruction: String::from("#scribe add this to decisions"),
        candidates: Vec::new(),
    }
}
fn answer(value: serde_json::Value) -> String {
    format!(
        "printf '%s\\n' '{}' '{}'\n",
        serde_json::json!({"type":"item.completed","item":{"type":"agent_message","text":value.to_string()}}),
        serde_json::json!({"type":"turn.completed"})
    )
}

#[test]
fn codex_validates_port_schemas_with_null_populated_and_omitted_open() {
    for open in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!({"heading":"Work","text":"Did work","topics":[],"related":[]})),
    ] {
        let mut value = serde_json::json!({"title":"Work","summary":"Did work","goals":"Work","next":[],"topics":[],"related":[],"terms":[],"new":[]});
        if let Some(open) = open {
            value["open"] = open;
        }
        let fake = Fake::new(&answer(value));
        let mut req = pass::request(
            "gpt-6-luna",
            None,
            Vec::new(),
            pass::Window {
                messages: Vec::new(),
                dropped: 0,
            },
            Vec::new(),
            Vec::new(),
            false,
        );
        req.budget = Duration::from_secs(5);
        assert!(fake.model().session(&req).is_ok());
    }
}

#[test]
fn malformed_tool_failure_and_oversized_results_are_refused_without_fallback() {
    let cases = [
        String::from("printf '%s\\n' not-json"),
        answer(serde_json::json!({"message":"a","span":"valid quote","extra":"must fail"})),
        answer(serde_json::json!({"message":"a"})),
        String::from(
            "printf '%s\\n' '{\"type\":\"item.started\",\"item\":{\"type\":\"command_execution\",\"command\":\"touch forbidden\"}}' '{\"type\":\"turn.completed\"}'",
        ),
        String::from("printf '%s\\n' '{\"type\":\"turn.failed\"}'; exit 7"),
        String::from("/usr/bin/head -c 1100000 /dev/zero"),
    ];
    for script in cases {
        let fake = Fake::new(&script);
        assert!(fake.model().referent(&request()).is_err(), "{script}");
    }
    let fake = Fake::new(&answer(
        serde_json::json!({"message":"a","span":"valid quote"}),
    ));
    assert!(fake.model().referent(&request()).is_ok());
}

#[test]
fn codex_timeout_uses_the_shared_group_supervisor_and_missing_runtime_is_actionable() {
    let fake = Fake::new("echo $$ > \"$HOME/leader\"; /bin/sleep 30");
    let mut req = request();
    req.budget = Duration::from_millis(60);
    let start = Instant::now();
    assert!(matches!(
        fake.model().referent(&req),
        Err(ModelError::Timeout { .. })
    ));
    assert!(start.elapsed() < Duration::from_secs(4));
    let pid: i32 = std::fs::read_to_string(fake.dir.path().join("leader"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let pid = rustix::process::Pid::from_raw(pid).unwrap();
    assert!(
        rustix::process::kill_process(pid, rustix::process::Signal::CONT).is_err(),
        "leader must be reaped"
    );
    let model = ClaudeCliModels::new(vec![(
        OsString::from("PATH"),
        OsString::from("/no/runtime"),
    )])
    .with_provider(Provider::Codex);
    let err = model.referent(&request()).unwrap_err();
    assert!(matches!(err, ModelError::Unavailable(_)));
    assert!(err.to_string().contains("codex"));
}

#[test]
fn codex_failure_diagnostics_mask_configured_credentials() {
    let fake = Fake::new("echo \"$OPENAI_API_KEY\" >&2; exit 7");
    let path = format!("{}:/usr/bin:/bin", fake.dir.path().join("bin").display());
    let env = vec![
        (OsString::from("PATH"), OsString::from(path)),
        (OsString::from("HOME"), fake.dir.path().into()),
        (
            OsString::from("OPENAI_API_KEY"),
            OsString::from("synthetic-secret-for-redaction"),
        ),
    ];
    let adapter = ClaudeCliModels::new(env).with_provider(Provider::Codex);
    let error = adapter.referent(&request()).unwrap_err().to_string();
    assert!(!error.contains("synthetic-secret-for-redaction"));
    assert!(error.contains("[redacted:OPENAI_API_KEY]"));
}
