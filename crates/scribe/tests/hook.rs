//! End-to-end tests of `scribe hook`, the Claude Code hook entry points:
//! `#scribe` capture from a fixture `UserPromptSubmit` payload, the reply
//! shown to Daniel and the agent, idempotent replays, the transcript
//! provenance check, the audit lines, and the `Stop` hook filling the cite's
//! message uuid (docs/design/scribe/2_design.md §4, §10 S2), and the
//! `SessionStart` reminder to keep recording a continuation (§5.4, §7).
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
        "scribe: added preferences-1 to preferences — \"squash-merge agent PRs; keep merge commits for releases\"\n\
         scribe: added preferences-2 to preferences — \"short commit subjects\""
    );
    assert_eq!(
        reply["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    assert_eq!(
        reply["hookSpecificOutput"]["additionalContext"],
        "scribe: added preferences-1 to preferences — \"squash-merge agent PRs; keep merge commits for releases\" (recorded by scribe; no action needed)\n\
         scribe: added preferences-2 to preferences — \"short commit subjects\" (recorded by scribe; no action needed)"
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
    assert_eq!(first["id"], "preferences-1");
    assert_eq!(
        first["said"],
        "#scribe preferences: squash-merge agent PRs; keep merge commits for releases #merge-policy"
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
    assert!(first["review_after"].is_string(), "preferences go stale");
    let cite = &first["cite"];
    assert_eq!(cite["session"], SESSION);
    assert_eq!(cite["message"], Value::Null, "filled by the Stop hook");
    assert_eq!(cite["line"], 2);
    assert_eq!(cite["cwd"], claude.cwd.to_str().unwrap());
    assert_eq!(cite["prompt_sha256"].as_str().unwrap().len(), 64);
    let second = &entries[1];
    assert_eq!(second["id"], "preferences-2");
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
        message
            .starts_with("scribe: already recorded preferences-1 in preferences — \"squash-merge"),
        "{message}"
    );
}

#[test]
fn a_prompt_without_an_instruction_line_writes_and_prints_nothing() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "```\n#scribe preferences: fenced\n```\n> #scribe preferences: quoted\n\
         <pasted_content>\n#scribe preferences: pasted\n</pasted_content>\n\
         #decision bare marker\n#preferences another\nplain #scribe mention"
    );
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(out.stdout.is_empty(), "no reply: {}", text(&out.stdout));
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
    assert!(!fx.root().exists(), "the store is not even created");
}

#[test]
fn agent_originated_wrappers_record_and_decide_nothing() {
    let wrappers = [
        "<agent-message from=\"other-session\">\n#scribe preferences: agent wrote this\n</agent-message>",
        "<task-notification>\n<task-id>t-1</task-id>\n#scribe preferences: agent wrote this\n</task-notification>",
        "<side-sessions-event>\n#scribe preferences: agent wrote this\n</side-sessions-event>",
        "<scheduled-loop>\n#scribe preferences: agent wrote this\n</scheduled-loop>",
        "<scheduled-prompt>\n#scribe preferences: agent wrote this\n</scheduled-prompt>",
        "<loop-prompt>\n#scribe preferences: agent wrote this\n</loop-prompt>",
    ];
    for prompt in wrappers {
        let fx = Fixture::new();
        let claude = Claude::new(&fx);
        let mut payload = claude.payload("user-prompt-submit.json");
        payload["prompt"] = json!(prompt);
        let out = hook_ok(&fx, "user-prompt-submit", &payload);
        assert!(
            out.stdout.is_empty(),
            "wrapper {prompt:?}: {}",
            text(&out.stdout)
        );
        assert!(!fx.root().exists(), "wrapper {prompt:?} created the store");
    }
}

#[test]
fn an_agent_wrapper_cannot_forget_an_entry_or_switch_scribe_on() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    hook_ok(
        &fx,
        "user-prompt-submit",
        &claude.payload("user-prompt-submit.json"),
    );
    assert_eq!(entries(&fx).len(), 2);
    let audit_before = audit(&fx).len();

    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "<agent-message from=\"other-session\">\n#scribe forget preferences-1\n</agent-message>"
    );
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    let entries = entries(&fx);
    assert_eq!(
        entries[0]["content"],
        "squash-merge agent PRs; keep merge commits for releases"
    );
    assert_eq!(
        audit(&fx).len(),
        audit_before,
        "a skipped turn audits nothing"
    );

    // A non-plain payload source refuses a `/scribe on` first line too.
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("/scribe on");
    payload["source"] = json!("agent");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(
        !fx.root().join("control.json").exists(),
        "agent turn switched scribe on"
    );
}

#[test]
fn a_non_plain_payload_source_records_nothing() {
    for source in ["scheduled", "task-notification", "agent-message"] {
        let fx = Fixture::new();
        let claude = Claude::new(&fx);
        let mut payload = claude.payload("user-prompt-submit.json");
        payload["prompt"] = json!("#scribe preferences: agent wrote this");
        payload["source"] = json!(source);
        let out = hook_ok(&fx, "user-prompt-submit", &payload);
        assert!(
            out.stdout.is_empty(),
            "source {source}: {}",
            text(&out.stdout)
        );
        assert!(!fx.root().exists(), "source {source} created the store");
    }
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
        "[tags.preferences]\naliases = [\"preference\"]\n\n[tags.rituals]\naliases = [\"ritual\"]\n",
    )
    .unwrap();
    make_private(&config);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] =
        json!("#scribe add stand-up at nine to my habits\n#scribe ritual: stand-up at nine");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: refused \"#scribe add stand-up at nine to my habits\" — no tag \"habits\"; tags: finding, idea, preferences, remind, rituals, standard\n\
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
fn a_refused_line_tells_the_agent_to_relay_it_to_daniel_not_that_no_action_is_needed() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe nosuchtag: hello");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let reply = reply(&out);
    let message = reply["systemMessage"].as_str().unwrap();
    assert!(message.starts_with("scribe: refused"), "{message}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(
        context,
        format!("{message} (not recorded by scribe; relay this to Daniel)")
    );
    assert!(!context.contains("no action needed"), "{context}");
}

#[test]
fn a_failed_line_tells_the_agent_to_relay_it_to_daniel_not_that_no_action_is_needed() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe preferences: hello");
    let elsewhere = fx.elsewhere(&format!("{SESSION}.jsonl"));
    payload["transcript_path"] = json!(elsewhere.to_str().unwrap());
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let reply = reply(&out);
    let message = reply["systemMessage"].as_str().unwrap();
    assert!(message.starts_with("scribe: could not record"), "{message}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(
        context,
        format!("{message} (not recorded by scribe; relay this to Daniel)")
    );
    assert!(!context.contains("no action needed"), "{context}");
}

#[test]
fn a_refused_approve_tells_the_agent_to_relay_it_to_daniel() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe approve p-99");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let reply = reply(&out);
    let message = reply["systemMessage"].as_str().unwrap();
    assert!(message.starts_with("scribe: refused"), "{message}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(
        context,
        format!("{message} (not recorded by scribe; relay this to Daniel)")
    );
    assert!(!context.contains("no action needed"), "{context}");
}

#[test]
fn a_mixed_prompt_notes_each_line_by_whether_it_was_recorded() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    std::fs::create_dir(fx.root()).unwrap();
    make_private(&fx.root());
    let config = fx.root().join("config.toml");
    std::fs::write(
        &config,
        "[tags.preferences]\naliases = [\"preference\"]\n\n[tags.rituals]\naliases = [\"ritual\"]\n",
    )
    .unwrap();
    make_private(&config);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] =
        json!("#scribe add stand-up at nine to my habits\n#scribe ritual: stand-up at nine");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let reply = reply(&out);
    assert_eq!(
        reply["hookSpecificOutput"]["additionalContext"],
        "scribe: refused \"#scribe add stand-up at nine to my habits\" — no tag \"habits\"; tags: finding, idea, preferences, remind, rituals, standard (not recorded by scribe; relay this to Daniel)\n\
         scribe: added rituals-1 to rituals — \"stand-up at nine\" (recorded by scribe; no action needed)"
    );
}

#[test]
fn the_audit_log_holds_ids_and_codes_never_what_was_said() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "#scribe preferences: the secret phrase is marmalade\n#scribe nonsense without a form"
    );
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
                "author",
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
        message.starts_with("scribe: could not record \"#scribe preferences: squash-merge"),
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
    std::fs::write(&config, "[tags.preferences]\nalias = [\"typo\"]\n").unwrap();
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
    assert_eq!(filled, [&json!("preferences-1"), &json!("preferences-2")]);
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
fn stop_recovers_a_missed_prompt_once_beside_a_prompt_hook_capture() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe preference: short subjects");
    hook_ok(&fx, "user-prompt-submit", &payload);
    let original = entries(&fx).remove(0);
    let missed = "An opening turn\n#scribe preference: keep the words #capture";
    let lines = [
        json!({"type":"user", "uuid":"u-recorded", "timestamp":now_rfc3339(),
               "cwd":claude.cwd, "message":{"content":payload["prompt"]}}),
        json!({"type":"user", "uuid":"u-missed", "timestamp":"2026-11-16T05:50:00Z",
               "cwd":claude.cwd, "message":{"content":missed}}),
    ];
    std::fs::write(
        &claude.transcript,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let recorded = entries(&fx);
    assert_eq!(
        recorded.len(),
        2,
        "Stop must recover the missed prompt exactly once"
    );
    let old = recorded.iter().find(|e| e["id"] == original["id"]).unwrap();
    assert_eq!(
        old["cite"]["prompt_sha256"],
        original["cite"]["prompt_sha256"]
    );
    assert_eq!(old["cite"]["message"], "u-recorded");
    let recovered = recorded
        .iter()
        .find(|e| e["content"] == "keep the words")
        .unwrap();
    assert_eq!(
        recovered["said"],
        "#scribe preference: keep the words #capture"
    );
    assert_eq!(recovered["topics"], json!(["capture"]));
    assert_eq!(recovered["cite"]["session"], SESSION);
    assert_eq!(recovered["cite"]["message"], "u-missed");
    assert_eq!(recovered["cite"]["at"], "2026-11-16T05:50:00Z");
    assert_eq!(recovered["cite"]["cwd"], claude.cwd.to_str().unwrap());
    assert_eq!(recovered["cite"]["line"], 2);
    assert_eq!(
        recovered["cite"]["prompt_sha256"],
        scribe_core::capture::prompt_sha256(missed)
    );
    let echo = reply(&out);
    assert_eq!(
        echo["systemMessage"],
        format!(
            "scribe: added {} to preferences — \"keep the words\"",
            recovered["id"].as_str().unwrap()
        )
    );
    assert!(echo.get("decision").is_none());
    let before = snapshot(&fx.root());
    assert!(
        hook_ok(&fx, "stop", &claude.payload("stop.json"))
            .stdout
            .is_empty()
    );
    assert_eq!(
        snapshot(&fx.root()),
        before,
        "a second Stop neither writes nor audits"
    );
    hook_ok(
        &fx,
        "user-prompt-submit",
        &json!({
        "session_id":SESSION, "transcript_path":claude.transcript,
        "cwd":claude.cwd, "prompt":missed}),
    );
    assert_eq!(
        entries(&fx).len(),
        2,
        "a late prompt-hook replay is idempotent too"
    );
}

#[test]
fn stop_recovery_obeys_the_same_origin_and_quoted_text_guards_as_prompt_capture() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let cases = [
        json!({"content":"<agent-message>\n#scribe preference: agent\n</agent-message>"}),
        json!({"content":"#scribe preference: source", "source":"scheduled"}),
        json!({"content":"#scribe preference: origin", "prompt_origin":"agent"}),
        json!({"content":"#scribe preference: sidechain", "isSidechain":true}),
        json!({"content":"#scribe preference: meta", "isMeta":true}),
        json!({"content":"```\n#scribe preference: fenced\n```\n> #scribe preference: quoted\n<pasted_content>\n#scribe preference: pasted\n</pasted_content>"}),
        json!({"content":"#scribe preference: assistant", "type":"assistant"}),
        json!({"content":"/scribe on\n#scribe preference: command"}),
    ];
    let mut lines = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let mut line = case.clone();
        line["type"] = case.get("type").cloned().unwrap_or(json!("user"));
        line["uuid"] = json!(format!("u-{i}"));
        line["timestamp"] = json!("2026-11-16T05:50:00Z");
        line["cwd"] = json!(claude.cwd);
        line["message"] = json!({"content":case["content"]});
        lines.push(line);
    }
    std::fs::write(
        &claude.transcript,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
    assert!(
        hook_ok(&fx, "stop", &claude.payload("stop.json"))
            .stdout
            .is_empty()
    );
    assert!(
        !fx.root().exists(),
        "excluded turns must create no state or audit"
    );
}

#[test]
fn stop_recovers_only_the_missing_lines_of_a_partly_captured_block_message() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let first = "#scribe preference: already captured";
    let second = "#scribe preference: missed block";
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(first);
    hook_ok(&fx, "user-prompt-submit", &payload);
    let line = json!({"type":"user", "uuid":"u-blocks", "timestamp":now_rfc3339(),
        "cwd":claude.cwd, "message":{"content":[
            {"type":"text", "text":first}, {"type":"text", "text":second}]}});
    std::fs::write(&claude.transcript, format!("{line}\n{line}\n")).unwrap();
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert_eq!(
        entries(&fx).len(),
        2,
        "existing block hash and replayed uuid cannot duplicate entries"
    );
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: added preferences-2 to preferences — \"missed block\""
    );
    let before = snapshot(&fx.root());
    assert!(
        hook_ok(&fx, "stop", &claude.payload("stop.json"))
            .stdout
            .is_empty()
    );
    assert_eq!(snapshot(&fx.root()), before);
}

#[test]
fn stop_recovers_action_context_and_referents_from_before_the_missed_prompt_only() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let lines = [
        json!({"type":"assistant", "uuid":"a-before", "timestamp":"2026-11-16T05:49:00Z",
            "message":{"content":"The earlier verified answer."}}),
        json!({"type":"user", "uuid":"u-missed", "timestamp":"2026-11-16T05:50:00Z",
            "cwd":claude.cwd, "message":{"content":"#scribe standard: keep context\n#scribe add this to preferences"}}),
        json!({"type":"assistant", "uuid":"a-after", "timestamp":"2026-11-16T05:51:00Z",
            "message":{"content":"This later answer must not enter the capture."}}),
    ];
    std::fs::write(
        &claude.transcript,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let recorded = entries(&fx);
    assert_eq!(recorded.len(), 2);
    let referent = recorded.iter().find(|e| e["tag"] == "preferences").unwrap();
    assert_eq!(referent["content"], "The earlier verified answer.");
    assert_eq!(referent["content_from"]["message"], "a-before");
    let action = recorded.iter().find(|e| e["tag"] == "standard").unwrap();
    let detail = fx.ok(&[
        "entry",
        "show",
        action["id"].as_str().unwrap(),
        "--window",
        "--json",
    ]);
    let detail: Value = serde_json::from_slice(&detail.stdout).unwrap();
    let turns = detail["window"]["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0]["message"], "a-before");
    assert_eq!(turns[1]["role"], "user");
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("guessed:")
    );
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("Stop recovery does not call models")
    );
    let before = snapshot(&fx.root());
    assert!(
        hook_ok(&fx, "stop", &claude.payload("stop.json"))
            .stdout
            .is_empty()
    );
    assert_eq!(snapshot(&fx.root()), before);
}

#[test]
fn stop_reports_refused_missed_lines_and_continues_recovering_other_prompts() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let lines = [
        json!({"type":"user", "uuid":"u-bad", "timestamp":now_rfc3339(),
            "cwd":claude.cwd, "message":{"content":"#scribe rituals: unsupported tag"}}),
        json!({"type":"user", "uuid":"u-good", "timestamp":now_rfc3339(),
            "cwd":claude.cwd, "message":{"content":"#scribe preference: keep going"}}),
    ];
    std::fs::write(
        &claude.transcript,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert_eq!(entries(&fx).len(), 1);
    let echo = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(echo.contains("scribe: refused"));
    assert!(echo.contains("scribe: added preferences-1"));
    assert_eq!(
        audit(&fx)
            .iter()
            .filter(|line| line["error_code"] == "unknown_tag")
            .count(),
        1
    );
    assert_eq!(
        audit(&fx)
            .iter()
            .filter(|line| line["target"] == "preferences-1")
            .count(),
        1
    );
}

#[test]
fn stop_reports_missing_citation_metadata_and_retries_when_it_becomes_available() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    claude.write_transcript("#scribe preference: recover with a cwd", "u-missed");
    let mut payload = claude.payload("stop.json");
    payload.as_object_mut().unwrap().remove("cwd");
    let out = hook_ok(&fx, "stop", &payload);
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("could not recover")
    );
    assert!(entries(&fx).is_empty());
    assert_eq!(audit(&fx)[0]["error_code"], "hook_payload_invalid");
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("scribe: added")
    );
    assert_eq!(entries(&fx)[0]["cite"]["cwd"], claude.cwd.to_str().unwrap());
    // The stored cite is sufficient even when later payloads lack cwd.
    let before = snapshot(&fx.root());
    assert!(hook_ok(&fx, "stop", &payload).stdout.is_empty());
    assert_eq!(snapshot(&fx.root()), before);
}

#[test]
fn stop_never_replays_historical_settling_commands() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut prompt = claude.payload("user-prompt-submit.json");
    prompt["prompt"] = json!("#scribe preferences: keep this entry");
    hook_ok(&fx, "user-prompt-submit", &prompt);
    claude.write_transcript("#scribe forget preferences-1", "u-settle");
    let before = entries_snapshot(&fx);
    let audit_before = audit(&fx);
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert_eq!(
        entries_snapshot(&fx),
        before,
        "history cannot reapply a destructive instruction"
    );
    assert_eq!(audit(&fx), audit_before);
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

// --- form 3: referents, with a fake `claude` on PATH ---------------------

/// A fake `claude` in the fixture: records each call's argv and environment
/// under the `HOME` scribe gives it, then runs `body`.
fn fake_claude(fx: &Fixture, body: &str) -> std::ffi::OsString {
    let bin = fx.elsewhere("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = format!(
        "#!/bin/sh\n\
         n=$(ls \"$HOME\" | grep -c '^claude-argv')\n\
         for a in \"$@\"; do printf '%s\\n' \"<$a>\"; done > \"$HOME/claude-argv-$n\"\n\
         env > \"$HOME/claude-env-$n\"\n\
         cat > /dev/null\n\
         {body}\n"
    );
    let path = bin.join("claude");
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut var = bin.into_os_string();
    var.push(":/usr/bin:/bin");
    var
}

/// The argv of each call the fake `claude` received, oldest first.
fn claude_calls(fx: &Fixture) -> Vec<Vec<String>> {
    (0..)
        .map(|n| fx.home().join(format!("claude-argv-{n}")))
        .take_while(|p| p.exists())
        .map(|p| {
            std::fs::read_to_string(p)
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect()
        })
        .collect()
}

const SPOKEN: &str = "Options: squash or merge.\n\nI recommend we squash-merge agent PRs and keep merge commits for releases.";

/// The conversation so far, before the `#scribe add this` prompt.
fn write_conversation(claude: &Claude) {
    let lines = [
        json!({"type": "user", "uuid": "u-1", "sessionId": SESSION,
               "timestamp": "2026-11-03T06:00:00Z",
               "message": {"role": "user", "content": "How should agent PRs merge?"}}),
        json!({"type": "assistant", "uuid": "a-1", "sessionId": SESSION,
               "timestamp": "2026-11-03T06:00:05Z",
               "message": {"role": "assistant", "content": [
                   {"type": "thinking", "thinking": "hmm"},
                   {"type": "text", "text": SPOKEN}]}}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&claude.transcript, text).unwrap();
}

fn referent_payload(claude: &Claude, prompt: &str) -> Value {
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(prompt);
    payload
}

fn hook_with_path(fx: &Fixture, payload: &Value, path: &std::ffi::OsString) -> Output {
    let child = fx
        .cmd(&["hook", "user-prompt-submit"])
        .env("PATH", path)
        .env("LANG", "C.UTF-8")
        .env("CLAUDE_CODE_OAUTH_TOKEN", "oauth-test-value")
        .env("GITHUB_TOKEN", "not-for-the-model")
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
    out
}

#[test]
fn add_this_records_the_span_the_model_chose_through_a_safe_mode_claude() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_conversation(&claude);
    let path = fake_claude(
        &fx,
        r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"message":"a-1","span":"squash-merge agent PRs and keep merge commits for releases."}}'"#,
    );
    let payload = referent_payload(
        &claude,
        "Good.\n#scribe add this to preferences #merge-policy",
    );
    let out = hook_with_path(&fx, &payload, &path);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: added preferences-1 to preferences — \"squash-merge agent PRs and keep merge commits for releases.\""
    );
    let stored = entries(&fx);
    assert_eq!(stored.len(), 1);
    let entry = &stored[0];
    assert_eq!(
        entry["said"],
        "#scribe add this to preferences #merge-policy"
    );
    assert_eq!(
        entry["content"],
        "squash-merge agent PRs and keep merge commits for releases."
    );
    assert_eq!(entry["topics"], json!(["merge-policy"]));
    assert_eq!(
        entry["content_from"],
        json!({"kind": "message", "role": "assistant", "message": "a-1",
               "at": "2026-11-03T06:00:05Z"})
    );
    assert_eq!(entry["cite"]["line"], 2);
    let targets: Vec<Value> = audit(&fx).iter().map(|l| l["target"].clone()).collect();
    assert_eq!(targets, [json!("preferences-1")]);

    // The call: safe mode, no tools, the configured referent model.
    let calls = claude_calls(&fx);
    assert_eq!(calls.len(), 1);
    let argv = &calls[0];
    assert_eq!(argv[..4], ["<-p>", "<--safe-mode>", "<--tools>", "<>"]);
    assert_eq!(argv[4..6], ["<--model>", "<claude-haiku-5-5>"]);
    // Only the allowlist reached it: no SCRIBE_ROOT, no unconfigured token.
    let env = std::fs::read_to_string(fx.home().join("claude-env-0")).unwrap();
    assert!(
        env.contains("CLAUDE_CODE_OAUTH_TOKEN=oauth-test-value"),
        "{env}"
    );
    assert!(!env.contains("SCRIBE_ROOT"), "{env}");
    assert!(!env.contains("GITHUB_TOKEN"), "{env}");
    assert!(!argv.iter().any(|a| a.contains("oauth-test-value")));

    // A replay echoes it and does not call the model again.
    let out = hook_with_path(&fx, &payload, &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.starts_with("scribe: already recorded preferences-1 in preferences"),
        "{message}"
    );
    assert_eq!(claude_calls(&fx).len(), 1);
    assert_eq!(entries(&fx).len(), 1);
}

#[test]
fn a_failing_or_unverifiable_model_still_writes_one_guessed_entry() {
    for body in [
        "echo 'Not logged in' >&2; exit 1",
        r#"echo '{"type":"result","is_error":false,"structured_output":{"message":"a-1","span":"we always rebase agent PRs"}}'"#,
        r#"echo '{"type":"result","is_error":false,"structured_output":{"message":"a-9","span":"squash-merge agent PRs"}}'"#,
    ] {
        let fx = Fixture::new();
        let claude = Claude::new(&fx);
        write_conversation(&claude);
        let path = fake_claude(&fx, body);
        let payload = referent_payload(&claude, "#scribe record it in my preferences");
        let out = hook_with_path(&fx, &payload, &path);
        let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
        assert!(
            message.starts_with("scribe: added preferences-1 to preferences (guessed: "),
            "{body}: {message}"
        );
        let entries = entries(&fx);
        assert_eq!(entries.len(), 1, "{body}");
        assert_eq!(entries[0]["content"], SPOKEN);
        assert_eq!(entries[0]["content_from"]["role"], "assistant");
        assert_eq!(entries[0]["content_from"]["message"], "a-1");
        assert_eq!(claude_calls(&fx).len(), 1);
    }
}

#[test]
fn a_referent_with_nothing_before_it_is_refused_without_a_model_call() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let path = fake_claude(&fx, "exit 1");
    let payload = referent_payload(&claude, "#scribe add this to preferences");
    let out = hook_with_path(&fx, &payload, &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.starts_with(
            "scribe: refused \"#scribe add this to preferences\" — there is no earlier message"
        ),
        "{message}"
    );
    assert!(claude_calls(&fx).is_empty());
    assert!(!fx.root().join("entries").exists());
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(codes, [json!("no_referent")]);
}

#[test]
fn store_tag_capture_and_daniel_approval_of_agent_proposal() {
    let fx = Fixture::new();
    fx.ok(&[
        "tag",
        "add",
        "notes",
        "--author",
        "opus",
        "--description",
        "Agent notes",
        "--alias",
        "note",
    ]);
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe keep this note to note");
    claude.write_transcript(payload["prompt"].as_str().unwrap(), "u-note");
    hook_ok(&fx, "user-prompt-submit", &payload);
    let recorded = entries(&fx);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["tag"], "notes");
    assert_eq!(recorded[0]["author"]["kind"], "daniel");
    fx.ok(&[
        "proposal",
        "add",
        "--kind",
        "stale",
        "--older",
        "notes-1",
        "--reason",
        "Review this",
        "--author",
        "opus",
    ]);
    payload["prompt"] = json!("#scribe approve p-1");
    claude.write_transcript("#scribe approve p-1", "u-approve");
    hook_ok(&fx, "user-prompt-submit", &payload);
    assert_eq!(entries(&fx)[0]["status"], "retired");
    let proposals: Value =
        serde_json::from_slice(&fx.ok(&["proposal", "list", "--json"]).stdout).unwrap();
    assert_eq!(
        proposals["proposals"][0]["settled_author"]["kind"],
        "daniel"
    );
}

fn listed(fx: &Fixture, project: &str) -> Vec<Value> {
    let out = fx.ok(&["entry", "list", "--project", project, "--json"]);
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

fn frontmatter(fx: &Fixture, entry: &Value) -> String {
    let id = entry["id"].as_str().unwrap();
    let tag = entry["tag"].as_str().unwrap();
    std::fs::read_to_string(fx.root().join("entries").join(tag).join(format!("{id}.md"))).unwrap()
}

fn by_said<'a>(entries: &'a [Value], said: &str) -> &'a Value {
    entries
        .iter()
        .find(|entry| entry["said"] == said)
        .unwrap_or_else(|| panic!("no entry said {said}: {entries:?}"))
}

#[test]
fn each_capture_form_records_global_or_the_cwd_project() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_conversation(&claude);
    let path = fake_claude(
        &fx,
        r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"message":"a-1","span":"squash-merge agent PRs and keep merge commits for releases."}}'"#,
    );
    let prompt = "\
#scribe/global preferences: squash-merge agent PRs
#scribe preferences: keep merge commits for releases
#scribe/global add short commit subjects to preferences
#scribe add long commit subjects to preferences
#scribe/global add this to preferences
#scribe add this to preferences";
    let out = hook_with_path(&fx, &referent_payload(&claude, prompt), &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        !message.contains("refused") && !message.contains("could not record"),
        "{message}"
    );

    let global = listed(&fx, "global");
    let local = listed(&fx, "scribe");
    assert_eq!(global.len(), 3, "{global:?}");
    assert_eq!(local.len(), 3, "{local:?}");
    assert_eq!(
        claude_calls(&fx).len(),
        2,
        "one model call per referent line"
    );

    let span = "squash-merge agent PRs and keep merge commits for releases.";
    let expect = [
        (
            &global,
            "#scribe/global preferences: squash-merge agent PRs",
            "preferences",
            "squash-merge agent PRs",
            "global",
        ),
        (
            &local,
            "#scribe preferences: keep merge commits for releases",
            "preferences",
            "keep merge commits for releases",
            "scribe",
        ),
        (
            &global,
            "#scribe/global add short commit subjects to preferences",
            "preferences",
            "short commit subjects",
            "global",
        ),
        (
            &local,
            "#scribe add long commit subjects to preferences",
            "preferences",
            "long commit subjects",
            "scribe",
        ),
        (
            &global,
            "#scribe/global add this to preferences",
            "preferences",
            span,
            "global",
        ),
        (
            &local,
            "#scribe add this to preferences",
            "preferences",
            span,
            "scribe",
        ),
    ];
    for (entries, said, tag, content, project) in expect {
        let entry = by_said(entries, said);
        assert_eq!(entry["tag"], tag);
        assert_eq!(entry["content"], content);
        assert_eq!(entry["project"], project);
        assert_eq!(entry["author"]["kind"], "daniel");
        let front = frontmatter(&fx, entry);
        assert!(front.contains(&format!("project: {project}")), "{front}");
        assert!(front.contains("kind: daniel"), "{front}");
        assert!(!front.contains("kind: agent"), "{front}");
    }
}

#[test]
fn a_malformed_marker_or_a_marker_on_a_refused_line_writes_no_entry() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "#scribe/all preferences: hello\n\
         #scribe/global rituals: stand-up\n\
         #scribe/global add this to preferences"
    );
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert_eq!(message.lines().count(), 3, "{message}");
    for line in message.lines() {
        let reason = line
            .split(" — ")
            .nth(1)
            .unwrap_or_else(|| panic!("no reason: {line}"));
        assert!(
            reason.contains("#scribe/global"),
            "forms text omits the marker: {reason}"
        );
    }
    assert!(
        !fx.root().join("entries").exists(),
        "a refused marker wrote an entry"
    );

    payload["prompt"] = json!("#scribe preferences: keep settling unmarked");
    hook_ok(&fx, "user-prompt-submit", &payload);
    fx.ok(&[
        "proposal",
        "add",
        "--kind",
        "stale",
        "--older",
        "preferences-1",
        "--reason",
        "Review this",
        "--author",
        "opus",
    ]);
    payload["prompt"] = json!("#scribe/global approve p-1");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    let reason = message.split(" — ").nth(1).expect("reason");
    assert!(reason.contains("#scribe/global"), "{message}");
    assert_eq!(entries(&fx)[0]["status"], "active");
    assert!(listed(&fx, "global").is_empty());
}

const CURRENT: &str = "3d6a8c12-4e5f-4061-c8d9-0e1f2a3b4c5d";
const PREVIOUS: &str = "1b4e6a90-2c3d-4e5f-a6b7-8c9d0e1f2a3b";
const NEWER: &str = "2c5f7b01-3d4e-4f60-b7c8-9d0e1f2a3b4c";

/// A non-git directory whose project name is its basename, `scribe`.
fn project_cwd(fx: &Fixture) -> PathBuf {
    let cwd = fx.elsewhere("work/scribe");
    let git = cwd.join(".git");
    std::fs::create_dir_all(&git).unwrap();
    std::fs::write(
        git.join("config"),
        "[remote \"origin\"]\n\turl = https://example.com/scribe.git\n",
    )
    .unwrap();
    cwd
}

fn start_payload(cwd: Option<&Path>, session: &str) -> Value {
    let mut payload = json!({
        "session_id": session,
        "hook_event_name": "SessionStart",
        "source": "startup"
    });
    if let Some(cwd) = cwd {
        payload["cwd"] = json!(cwd);
    }
    payload
}

fn continuation_line(id: &str) -> String {
    format!(
        "scribe: this may continue {id} (recorded) — type /scribe on --from {id} to keep recording"
    )
}

/// One recorded session. `started` names the `sessions/<yyyy>/<mm>` directory.
fn write_session(
    fx: &Fixture,
    id: &str,
    project: &str,
    started: &str,
    updated: &str,
    active: bool,
) {
    let year = &started[0..4];
    let month = &started[5..7];
    let dir = fx.root().join(format!("sessions/{year}/{month}"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut cursor = fx.root();
    make_private(&cursor);
    for part in ["sessions", year, month] {
        cursor = cursor.join(part);
        make_private(&cursor);
    }
    let active = if active { "true" } else { "false" };
    let body = format!(
        "---
format: 4
kind: session
id: {id}
status: open
active: {active}
keep_active: false
active_change: null
cataloged_by: null
project: {project}
topics: []
related: []
links: []
terms: []
provider: claude-code
host: host
created: {started}
updated: {updated}
title: Continue the work
cwd: /work/scribe
started: {started}
on_since: {started}
scribed_through: 9a1b2c3d-0000-4000-8000-0000000000aa
passes: 1
next_open: 0
log:
  - {{n: 1, heading: Continue the work, first_message: 9a1b2c3d-0000-4000-8000-000000000001, last_message: 9a1b2c3d-0000-4000-8000-000000000002, from: {started}, to: {updated}, state: open, windows: 1, origin: pass, topics: [], related: []}}
---
## Summary
Recorded work.

## Goals
Keep going.

## Next
- keep recording

## Log
### 1. Continue the work
Recorded work.
"
    );
    let path = dir.join(format!("{id}.md"));
    std::fs::write(&path, body).unwrap();
    make_private(&path);
}

fn write_switch_on(fx: &Fixture, session: &str) {
    std::fs::create_dir_all(fx.root()).unwrap();
    make_private(&fx.root());
    let path = fx.root().join("control.json");
    let body = format!(
        r#"{{
  "format": 1,
  "sessions": {{
    "{session}": {{"on": true, "since": "2026-11-16T12:00:00Z", "due": false, "final": false}}
  }}
}}
"#
    );
    std::fs::write(&path, body).unwrap();
    make_private(&path);
}

fn write_open_proposal(fx: &Fixture) {
    std::fs::create_dir_all(fx.root()).unwrap();
    make_private(&fx.root());
    let path = fx.root().join("proposals.json");
    std::fs::write(
        &path,
        r#"{
  "format": 1,
  "proposals": [
    {
      "id": "p-1",
      "kind": "stale",
      "older": "rituals-1",
      "newer": null,
      "reason": "Past its review date.",
      "quotes": [{"entry": "rituals-1", "text": "run the full gate"}],
      "state": "open",
      "opened_at": "2026-11-16T12:00:00Z",
      "settled_at": null,
      "settled_by": null
    }
  ]
}
"#,
    )
    .unwrap();
    make_private(&path);
}

fn start_message(fx: &Fixture, payload: &Value) -> String {
    let out = hook_ok(fx, "session-start", payload);
    let stdout = text(&out.stdout);
    if stdout.is_empty() {
        return String::new();
    }
    reply(&out)["systemMessage"]
        .as_str()
        .unwrap_or_else(|| panic!("no systemMessage: {stdout}"))
        .to_owned()
}

#[test]
fn session_start_names_the_newest_recorded_session_and_the_command() {
    let fx = Fixture::new();
    let cwd = project_cwd(&fx);
    let now_dt = time::OffsetDateTime::now_utc();
    let earlier_dt = now_dt - time::Duration::hours(1);
    let format = &time::format_description::well_known::Rfc3339;
    let now = now_dt.format(format).unwrap();
    let earlier = earlier_dt.format(format).unwrap();
    write_session(&fx, PREVIOUS, "scribe", &earlier, &earlier, true);
    write_session(&fx, NEWER, "scribe", &now, &now, true);
    let alone = hook_ok(&fx, "session-start", &start_payload(Some(&cwd), CURRENT));
    let alone_reply = reply(&alone);
    assert_eq!(
        alone_reply["systemMessage"].as_str(),
        Some(continuation_line(NEWER).as_str())
    );
    assert!(
        alone_reply.get("hookSpecificOutput").is_none(),
        "the agent is not told: {alone_reply}"
    );

    write_open_proposal(&fx);
    let before = snapshot(&fx.root());
    let message = start_message(&fx, &start_payload(Some(&cwd), CURRENT));
    assert_eq!(
        message,
        format!(
            "scribe: 1 proposal to review — scribe proposal list\n{}",
            continuation_line(NEWER)
        )
    );
    let out = hook_ok(&fx, "session-start", &start_payload(Some(&cwd), CURRENT));
    let parsed = reply(&out);
    assert!(
        parsed.get("hookSpecificOutput").is_none(),
        "the agent is not told: {parsed}"
    );
    assert!(audit(&fx).is_empty(), "a reminder is not audited");
    assert_eq!(
        snapshot(&fx.root()),
        before,
        "session-start wrote the store"
    );
}

#[test]
fn session_start_skips_the_reminder_when_this_session_is_already_opted_in() {
    let fx = Fixture::new();
    let cwd = project_cwd(&fx);
    let now = now_rfc3339();
    write_session(&fx, PREVIOUS, "scribe", &now, &now, true);
    write_switch_on(&fx, CURRENT);

    assert_eq!(
        start_message(&fx, &start_payload(Some(&cwd), CURRENT)),
        "",
        "a session that is already on is not reminded"
    );

    // A record of this session, even beside a newer one, means it is already cataloged.
    let resumed = Fixture::new();
    let cwd = project_cwd(&resumed);
    write_session(&resumed, CURRENT, "scribe", &now, &now, true);
    write_session(&resumed, NEWER, "scribe", &now, &now, true);
    assert_eq!(
        start_message(&resumed, &start_payload(Some(&cwd), CURRENT)),
        ""
    );
}

#[test]
fn session_start_skips_the_reminder_when_no_recent_session_matches() {
    let now = now_rfc3339();
    let cases: &[(&str, &str, &str, bool, bool)] = &[
        ("stale", PREVIOUS, "scribe", true, false),
        ("other project", PREVIOUS, "other", true, true),
        ("inactive", PREVIOUS, "scribe", false, true),
    ];
    for (name, id, project, active, recent) in cases {
        let fx = Fixture::new();
        let cwd = project_cwd(&fx);
        let started = if *recent {
            now.clone()
        } else {
            String::from("2020-01-02T00:00:00Z")
        };
        let updated = started.clone();
        write_session(&fx, id, project, &started, &updated, *active);
        assert_eq!(
            start_message(&fx, &start_payload(Some(&cwd), CURRENT)),
            "",
            "{name}"
        );
    }

    let fx = Fixture::new();
    write_session(&fx, PREVIOUS, "scribe", &now, &now, true);
    assert_eq!(
        start_message(&fx, &start_payload(None, CURRENT)),
        "",
        "no cwd, no project to match"
    );
}

#[cfg(unix)]
#[test]
fn session_start_prints_no_reminder_when_the_sessions_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;

    let fx = Fixture::new();
    let cwd = project_cwd(&fx);
    let now = now_rfc3339();
    write_session(&fx, PREVIOUS, "scribe", &now, &now, true);
    write_open_proposal(&fx);
    let sessions = fx.root().join("sessions");
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o000)).unwrap();
    struct Restore<'a>(&'a Path);
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    let _restore = Restore(&sessions);

    let message = start_message(&fx, &start_payload(Some(&cwd), CURRENT));
    assert_eq!(
        message, "scribe: 1 proposal to review — scribe proposal list",
        "an unreadable session store adds no continuation, and the hook still starts"
    );
    assert!(audit(&fx).is_empty(), "a failed read is not audited");
}

/// Write `body` as the fixture store's `config.toml`.
fn write_config(fx: &Fixture, body: &str) {
    std::fs::create_dir_all(fx.root()).unwrap();
    make_private(&fx.root());
    let config = fx.root().join("config.toml");
    std::fs::write(&config, body).unwrap();
    make_private(&config);
}

#[test]
fn decisions_and_its_old_aliases_are_unknown_tags_without_config() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    for (n, word) in ["decisions", "decision", "dec"].into_iter().enumerate() {
        let mut payload = claude.payload("user-prompt-submit.json");
        let line = format!("#scribe {word}: squash-merge agent PRs");
        payload["prompt"] = json!(line);
        let out = hook(&fx, "user-prompt-submit", &payload);
        assert_eq!(out.status.code(), Some(0), "{word}: {}", text(&out.stderr));
        assert_eq!(
            reply(&out)["systemMessage"],
            format!(
                "scribe: refused \"{line}\" — no tag \"{word}\"; tags: finding, idea, preferences, remind, standard"
            ),
            "{word}"
        );
        let lines = audit(&fx);
        assert_eq!(lines.len(), n + 1, "{word}: one audit line per refusal");
        assert_eq!(lines[n]["error_code"], "unknown_tag", "{word}");
    }
    assert!(
        !fx.root().join("entries").exists(),
        "a refused line writes no entry"
    );
}

#[test]
fn a_config_that_declares_decisions_makes_it_work_like_any_config_tag() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_config(
        &fx,
        "[tags.decisions]\naliases = [\"decision\", \"dec\"]\ndescription = \"choices Daniel has made\"\n",
    );
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "#scribe decisions: squash-merge agent PRs\n#scribe dec: keep merge commits for releases"
    );
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: added decisions-1 to decisions — \"squash-merge agent PRs\"\n\
         scribe: added decisions-2 to decisions — \"keep merge commits for releases\""
    );
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "decisions",
        "--author",
        "opus",
        "a note",
    ]);
    let list = fx.ok(&["entry", "list", "--tag", "decisions", "--json"]);
    let list: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list["total"], 3, "{list}");
    let tags = fx.ok(&["tag", "list", "--json"]);
    let tags: Value = serde_json::from_slice(&tags.stdout).unwrap();
    let names: Vec<&str> = tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"decisions"), "{names:?}");
}

#[test]
fn entries_under_decisions_in_a_store_without_that_tag_stay_readable_and_settleable() {
    // A store from before the tag stopped shipping: entries under
    // `decisions`, no config naming it.
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    fx.seed();
    write_config(&fx, "# no tag table: the defaults apply\n");
    let tags = fx.ok(&["tag", "list", "--json"]);
    let tags: Value = serde_json::from_slice(&tags.stdout).unwrap();
    let row = tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "decisions")
        .unwrap();
    assert_eq!(row["entries"], 2, "{row}");
    assert_eq!(row["configured"], false, "{row}");

    let list = fx.ok(&["entry", "list", "--tag", "decisions", "--json"]);
    let list: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list["total"], 2, "{list}");
    let shown = fx.ok(&["entry", "show", "decisions-2", "--json"]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert!(
        shown["content"]
            .as_str()
            .unwrap()
            .contains("squash-merge agent PRs"),
        "{shown}"
    );
    let found = fx.ok(&["record", "search", "squash", "--json"]);
    assert!(
        text(&found.stdout).contains("decisions-2"),
        "search finds it: {}",
        text(&found.stdout)
    );

    // New writes under the old name are refused; settling the old entry works.
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe decisions: a new one");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(
        text(&out.stdout).contains("no tag"),
        "{}",
        text(&out.stdout)
    );
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe retire decisions-2");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(
        text(&out.stdout).contains("retired decisions-2"),
        "{}",
        text(&out.stdout)
    );
    let shown = fx.ok(&["entry", "show", "decisions-2", "--json"]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["status"], "retired");
}
