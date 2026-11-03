//! End-to-end tests of `scribe hook`, the Claude Code hook entry points:
//! `#scribe` capture from a fixture `UserPromptSubmit` payload, the reply
//! shown to Daniel and the agent, idempotent replays, the transcript
//! provenance check, the audit lines, and the `Stop` hook filling the cite's
//! message uuid (docs/design/scribe/2_design.md §4, §10 S2).
//!
//! Every run is the built binary with a temp `HOME` whose
//! `.claude/projects/` holds the fixture transcript; nothing touches the
//! real home.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use support::{Fixture, make_private, snapshot, text};

const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hooks")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

/// A Claude Code layout under the fixture's home: the transcript path for
/// `SESSION`, and a cwd that is not a git repository.
struct Claude {
    transcript: PathBuf,
    cwd: PathBuf,
}

impl Claude {
    fn new(fx: &Fixture) -> Self {
        let project = fx.home().join(".claude/projects/-work-scribe");
        std::fs::create_dir_all(&project).unwrap();
        let cwd = fx.elsewhere("work/scribe");
        std::fs::create_dir_all(&cwd).unwrap();
        Self {
            transcript: project.join(format!("{SESSION}.jsonl")),
            cwd,
        }
    }

    /// A fixture payload with the paths filled in.
    fn payload(&self, name: &str) -> Value {
        let text = fixture(name)
            .replace("$TRANSCRIPT", self.transcript.to_str().unwrap())
            .replace("$CWD", self.cwd.to_str().unwrap());
        serde_json::from_str(&text).unwrap()
    }

    /// Write the transcript as Claude Code would after the prompt: the user
    /// message carrying `prompt`, then the reply.
    fn write_transcript(&self, prompt: &str, uuid: &str) {
        let lines = [
            json!({"type": "session-owner", "sessionId": SESSION}),
            json!({"type": "user", "uuid": "u-earlier", "sessionId": SESSION,
                   "timestamp": "2026-01-01T00:00:00Z",
                   "message": {"role": "user", "content": "an earlier, different prompt"}}),
            json!({"type": "user", "uuid": uuid, "sessionId": SESSION,
                   "timestamp": now_rfc3339(),
                   "message": {"role": "user", "content": prompt}}),
            json!({"type": "assistant", "uuid": "a-1", "sessionId": SESSION,
                   "timestamp": now_rfc3339(),
                   "message": {"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}}),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&self.transcript, text).unwrap();
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

/// Run `scribe hook <event>` with `payload` on stdin.
fn hook(fx: &Fixture, event: &str, payload: &Value) -> Output {
    run_with_stdin(fx, &["hook", event], payload.to_string().as_bytes())
}

fn run_with_stdin(fx: &Fixture, args: &[&str], input: &[u8]) -> Output {
    let child = fx
        .cmd(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = support::ChildGuard::new(child);
    // Dropping the handle closes stdin, so the hook sees the end of input.
    guard
        .child()
        .stdin
        .take()
        .unwrap()
        .write_all(input)
        .unwrap();
    guard.finish()
}

fn hook_ok(fx: &Fixture, event: &str, payload: &Value) -> Output {
    let out = hook(fx, event, payload);
    assert_eq!(
        out.status.code(),
        Some(0),
        "hook {event} failed: {}",
        text(&out.stderr)
    );
    out
}

fn reply(out: &Output) -> Value {
    let stdout = text(&out.stdout);
    assert_eq!(stdout.lines().count(), 1, "one line of hook JSON: {stdout}");
    serde_json::from_str(&stdout).unwrap()
}

fn entries(fx: &Fixture) -> Vec<Value> {
    let out = fx.ok(&["entry", "list", "--json"]);
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

fn audit(fx: &Fixture) -> Vec<Value> {
    match std::fs::read_to_string(fx.root().join("audit.jsonl")) {
        Ok(text) => text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn entries_snapshot(fx: &Fixture) -> Vec<(PathBuf, Vec<u8>, u32)> {
    snapshot(&fx.root().join("entries"))
}

#[test]
fn each_scribe_line_of_the_fixture_prompt_becomes_one_entry_with_its_cite_and_echo() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let payload = claude.payload("user-prompt-submit.json");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);

    let reply = reply(&out);
    let message = reply["systemMessage"].as_str().unwrap();
    assert_eq!(
        message,
        "scribe: added decisions-1 to decisions — \"squash-merge agent PRs; keep merge commits for releases\"\n\
         scribe: added preferences-1 to preferences — \"short commit subjects\""
    );
    assert_eq!(
        reply["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    assert_eq!(
        reply["hookSpecificOutput"]["additionalContext"],
        format!("{message}\n(recorded by scribe; no action needed)")
    );
    assert!(
        reply.get("decision").is_none(),
        "the prompt is never blocked"
    );

    let entries = entries(&fx);
    assert_eq!(
        entries.len(),
        2,
        "fenced, quoted, pasted and bare lines write nothing"
    );
    let prompt = payload["prompt"].as_str().unwrap();
    let first = &entries[0];
    assert_eq!(first["id"], "decisions-1");
    assert_eq!(
        first["said"],
        "#scribe decisions: squash-merge agent PRs; keep merge commits for releases #merge-policy"
    );
    assert_eq!(
        first["content"],
        "squash-merge agent PRs; keep merge commits for releases"
    );
    assert_eq!(first["topics"], json!(["merge-policy"]));
    assert_eq!(first["status"], "active");
    assert_eq!(first["provider"], "claude-code");
    assert_eq!(first["project"], "scribe", "the cwd's basename outside git");
    assert_eq!(first["content_from"]["kind"], "instruction");
    assert_eq!(
        first["review_after"],
        Value::Null,
        "decisions never go stale"
    );
    let cite = &first["cite"];
    assert_eq!(cite["session"], SESSION);
    assert_eq!(cite["message"], Value::Null, "filled by the Stop hook");
    assert_eq!(cite["line"], 2);
    assert_eq!(cite["cwd"], claude.cwd.to_str().unwrap());
    assert_eq!(cite["prompt_sha256"].as_str().unwrap().len(), 64);
    let second = &entries[1];
    assert_eq!(second["id"], "preferences-1");
    assert_eq!(
        second["said"],
        "#scribe add short commit subjects to my preferences"
    );
    assert_eq!(second["content"], "short commit subjects");
    assert_eq!(second["cite"]["line"], 3);
    assert_eq!(second["cite"]["prompt_sha256"], cite["prompt_sha256"]);
    assert!(second["review_after"].is_string(), "preferences go stale");
    assert!(prompt.contains(first["said"].as_str().unwrap()));
}

#[test]
fn replaying_the_same_payload_writes_nothing_new() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let payload = claude.payload("user-prompt-submit.json");
    hook_ok(&fx, "user-prompt-submit", &payload);
    let before = entries_snapshot(&fx);
    let audit_before = audit(&fx).len();

    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert_eq!(entries_snapshot(&fx), before, "no entry file changed");
    assert_eq!(
        audit(&fx).len(),
        audit_before,
        "nothing written, nothing audited"
    );
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.starts_with("scribe: already recorded decisions-1 in decisions — \"squash-merge"),
        "{message}"
    );
}

#[test]
fn a_prompt_without_an_instruction_line_writes_and_prints_nothing() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "```\n#scribe decisions: fenced\n```\n> #scribe decisions: quoted\n\
         <pasted_content>\n#scribe decisions: pasted\n</pasted_content>\n\
         #decision bare marker\n#preferences another\nplain #scribe mention"
    );
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(out.stdout.is_empty(), "no reply: {}", text(&out.stdout));
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
    assert!(!fx.root().exists(), "the store is not even created");
}

#[test]
fn an_unknown_tag_is_refused_with_the_configured_tags_and_audited_by_code() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    std::fs::create_dir(fx.root()).unwrap();
    make_private(&fx.root());
    let config = fx.root().join("config.toml");
    std::fs::write(
        &config,
        "[tags.decisions]\naliases = [\"decision\"]\n\n[tags.rituals]\naliases = [\"ritual\"]\n",
    )
    .unwrap();
    make_private(&config);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] =
        json!("#scribe add stand-up at nine to my habits\n#scribe ritual: stand-up at nine");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: refused \"#scribe add stand-up at nine to my habits\" — no tag \"habits\"; tags: decisions, rituals\n\
         scribe: added rituals-1 to rituals — \"stand-up at nine\""
    );
    let lines = audit(&fx);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["command"], "hook user-prompt-submit");
    assert_eq!(lines[0]["status"], "failure");
    assert_eq!(lines[0]["error_code"], "unknown_tag");
    assert_eq!(lines[0]["target"], format!("session:{SESSION}"));
    assert_eq!(lines[1]["status"], "success");
    assert_eq!(lines[1]["target"], "rituals-1");
    assert_eq!(lines[1]["error_code"], Value::Null);
}

#[test]
fn the_audit_log_holds_ids_and_codes_never_what_was_said() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] =
        json!("#scribe decisions: the secret phrase is marmalade\n#scribe nonsense without a form");
    hook_ok(&fx, "user-prompt-submit", &payload);
    let log = std::fs::read_to_string(fx.root().join("audit.jsonl")).unwrap();
    for needle in [
        "marmalade",
        "nonsense",
        "#scribe",
        claude.cwd.to_str().unwrap(),
    ] {
        assert!(!log.contains(needle), "{needle} leaked into {log}");
    }
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(codes, [Value::Null, json!("unparsed_instruction")]);
    for line in audit(&fx) {
        let mut keys: Vec<&str> = line
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "at",
                "command",
                "duration_ms",
                "error_code",
                "format",
                "status",
                "target"
            ]
        );
    }
}

#[test]
fn a_transcript_outside_claudes_projects_fails_the_provenance_check() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    let elsewhere = fx.elsewhere(&format!("{SESSION}.jsonl"));
    payload["transcript_path"] = json!(elsewhere.to_str().unwrap());
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.starts_with("scribe: could not record \"#scribe decisions: squash-merge"),
        "{message}"
    );
    assert!(message.contains("transcript check"), "{message}");
    assert!(!fx.root().join("entries").exists(), "nothing written");
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(
        codes,
        [json!("provenance_failed"), json!("provenance_failed")]
    );

    // A transcript named for another session fails too.
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["session_id"] = json!("another-session");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("not named for the payload's session_id")
    );
    assert!(!fx.root().join("entries").exists());
}

#[cfg(unix)]
#[test]
fn a_linked_transcript_fails_the_provenance_check() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let target = fx.elsewhere("real.jsonl");
    std::fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, &claude.transcript).unwrap();
    let out = hook_ok(
        &fx,
        "user-prompt-submit",
        &claude.payload("user-prompt-submit.json"),
    );
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("not a regular file")
    );
    assert!(!fx.root().join("entries").exists());
}

#[test]
fn a_bad_config_is_reported_in_the_reply_and_nothing_is_written() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    std::fs::create_dir(fx.root()).unwrap();
    make_private(&fx.root());
    let config = fx.root().join("config.toml");
    std::fs::write(&config, "[tags.decisions]\nalias = [\"typo\"]\n").unwrap();
    make_private(&config);
    let out = hook_ok(
        &fx,
        "user-prompt-submit",
        &claude.payload("user-prompt-submit.json"),
    );
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.contains("is not a valid scribe config"),
        "{message}"
    );
    assert!(!fx.root().join("entries").exists());
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(codes, [json!("config_invalid"), json!("config_invalid")]);
}

#[test]
fn a_payload_that_is_not_hook_json_fails_with_a_stable_code() {
    let fx = Fixture::new();
    let out = run_with_stdin(
        &fx,
        &["hook", "user-prompt-submit", "--json"],
        b"{\"prompt\": 3}",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "hook_payload_invalid");
}

#[test]
fn the_stop_hook_fills_the_cite_message_uuid_from_the_transcript() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let payload = claude.payload("user-prompt-submit.json");
    hook_ok(&fx, "user-prompt-submit", &payload);
    let audit_before = audit(&fx).len();
    claude.write_transcript(payload["prompt"].as_str().unwrap(), "u-prompt");

    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert!(out.stdout.is_empty(), "the Stop hook prints nothing");
    let entries = entries(&fx);
    for entry in &entries {
        assert_eq!(entry["cite"]["message"], "u-prompt", "{}", entry["id"]);
    }
    let lines = audit(&fx);
    let filled: Vec<&Value> = lines[audit_before..].iter().map(|l| &l["target"]).collect();
    assert_eq!(filled, [&json!("decisions-1"), &json!("preferences-1")]);
    assert!(
        lines[audit_before..]
            .iter()
            .all(|l| l["command"] == "hook stop")
    );

    // Nothing waits any more: the next Stop changes nothing at all.
    let before = snapshot(&fx.root());
    hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert_eq!(snapshot(&fx.root()), before);
}

#[test]
fn the_stop_hook_with_nothing_waiting_touches_nothing() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    claude.write_transcript("no scribe here", "u-1");
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert!(out.stdout.is_empty());
    assert!(!fx.root().exists());
}

#[test]
fn the_stop_hook_leaves_a_cite_waiting_until_its_prompt_is_in_the_transcript() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    hook_ok(
        &fx,
        "user-prompt-submit",
        &claude.payload("user-prompt-submit.json"),
    );
    claude.write_transcript("a different prompt", "u-other");
    hook_ok(&fx, "stop", &claude.payload("stop.json"));
    for entry in entries(&fx) {
        assert_eq!(entry["cite"]["message"], Value::Null);
    }
}
