//! End-to-end tests of `scribe hook`, the Claude Code hook entry points:
//! `#scribe` markers echoed from a fixture `UserPromptSubmit` payload and
//! cataloged at `Stop` with a fake `claude` (docs/design/scribe/2_design.md
//! §14), settling lines (§4.4), the reply shown to Daniel and the agent, the
//! transcript provenance check, the audit lines, the `Stop` hook filling an
//! older entry's cite message uuid (§4.1, §10 S2), and the `SessionStart`
//! reminder to keep recording a continuation (§5.4, §7).
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
        make_private(&self.transcript);
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
    support::audit_lines(&fx.root())
}

/// Every stored entry, in full (entries live in `scribe.db`).
fn entries_snapshot(fx: &Fixture) -> Vec<Value> {
    entries(fx)
}

#[test]
fn each_scribe_line_of_the_fixture_prompt_is_a_marker_cataloged_at_stop() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let payload = claude.payload("user-prompt-submit.json");
    let path = annotating_claude(&fx, &none_for(1..=2));
    let out = hook_path(&fx, "user-prompt-submit", &payload, &path);

    let reply_json = reply(&out);
    assert_eq!(
        reply_json["systemMessage"],
        "scribe: marked 2 times — cataloged after the reply"
    );
    assert_eq!(
        reply_json["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    assert_eq!(
        reply_json["hookSpecificOutput"]["additionalContext"],
        "scribe: marked 2 times — cataloged after the reply (scribe tries to catalog this #scribe marker after your reply and tells you if it gives up; record it yourself only then)"
    );
    assert!(
        reply_json.get("decision").is_none(),
        "the prompt is never blocked"
    );
    assert!(claude_calls(&fx).is_empty(), "no model call at submit");
    assert!(!fx.root().exists(), "nothing written at submit");

    // Stop: one call for the turn's two line-start markers, each with its
    // tag as the hint. Fenced, quoted, pasted and bare lines are not
    // markers.
    let prompt = payload["prompt"].as_str().unwrap();
    marked_turns(&claude, &[("u-1", prompt)]);
    let path = annotating_claude(
        &fx,
        &json!([
            {"marker": 1, "kind": "entry", "tag": "preferences",
             "statement": "Daniel squash-merges agent PRs and keeps merge commits for releases.",
             "subject": "merging agent PRs",
             "quote": "squash-merge agent PRs; keep merge commits for releases",
             "message": "u-1", "reason": ""},
            {"marker": 2, "kind": "entry", "tag": "preferences",
             "statement": "Daniel wants short commit subjects.", "subject": "commit subjects",
             "quote": "short commit subjects", "message": "u-1", "reason": ""}
        ]),
    );
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: added preferences-1 to preferences — \"Daniel squash-merges agent PRs and keeps merge commits for releases. The user and the agent discussed it in this session.\" · your words: \"squash-merge agent PRs; keep merge commits for releases\"\n\
         scribe: added preferences-2 to preferences — \"Daniel wants short commit subjects. The user and the agent discussed it in this session.\" · your words: \"short commit subjects\""
    );
    assert_eq!(claude_calls(&fx).len(), 1, "one call for the turn");
    let asked = claude_stdin(&fx, 0);
    assert_eq!(
        asked.matches("(hint: tag preferences)").count(),
        2,
        "{asked}"
    );
    assert!(!asked.contains("- marker 3"), "{asked}");

    let entries = entries(&fx);
    assert_eq!(entries.len(), 2, "{entries:?}");
    let first = &entries[0];
    assert_eq!(first["id"], "preferences-1");
    assert_eq!(
        first["said"],
        "#scribe preferences: squash-merge agent PRs; keep merge commits for releases #merge-policy"
    );
    assert_eq!(
        first["quote"],
        "squash-merge agent PRs; keep merge commits for releases"
    );
    assert_eq!(first["topics"], json!(["merge-policy"]));
    assert_eq!(first["status"], "active");
    assert_eq!(first["provider"], "claude-code");
    assert_eq!(first["project"], "scribe", "the cwd's basename outside git");
    assert_eq!(first["author"]["kind"], "user");
    assert!(first["review_after"].is_string(), "preferences go stale");
    let cite = &first["cite"];
    assert_eq!(cite["session"], SESSION);
    assert_eq!(cite["message"], "u-1");
    assert_eq!(cite["line"], 2);
    assert_eq!(cite["prompt_sha256"].as_str().unwrap().len(), 64);
    let second = &entries[1];
    assert_eq!(
        second["said"],
        "#scribe add short commit subjects to my preferences"
    );
    assert_eq!(second["cite"]["line"], 3);
    assert_eq!(second["cite"]["prompt_sha256"], cite["prompt_sha256"]);
    assert!(prompt.contains(first["said"].as_str().unwrap()));
}

/// Replace the transcript with `lines`, one JSON object per line.
fn write_lines(claude: &Claude, lines: &[Value]) {
    std::fs::write(
        &claude.transcript,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
    make_private(&claude.transcript);
}

#[test]
fn a_prompt_without_an_instruction_line_writes_and_prints_nothing() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "```\n#scribe preferences: fenced\n```\n> #scribe preferences: quoted\n\
         <pasted_content>\n#scribe preferences: pasted\n</pasted_content>\n\
         #decision bare marker\n#preferences another\nplain `#scribe` mention\n\
         a \"#scribe\" in quotes"
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
    write_verbatim(
        &fx,
        1,
        "#scribe preferences: squash-merge agent PRs; keep merge commits for releases",
        "an older prompt",
        1,
        Some("u-0"),
    );
    assert_eq!(entries(&fx).len(), 1);
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
        !support::has_control(&fx.root()),
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
fn an_unknown_tag_the_model_chooses_is_refused_at_stop_and_audited_by_code() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_config(
        &fx,
        "[tags.preferences]\naliases = [\"preference\"]\n\n[tags.rituals]\naliases = [\"ritual\"]\n",
    );
    let typed = "#scribe add stand-up at nine to my habits\n#scribe ritual: stand-up at nine";
    let path = annotating_claude(
        &fx,
        &json!([
            {"marker": 1, "kind": "entry", "tag": "habits", "statement": "Daniel stands up at nine.",
             "subject": "stand-up", "quote": "stand-up at nine", "message": "u-1", "reason": ""},
            {"marker": 2, "kind": "entry", "tag": "rituals", "statement": "Daniel holds stand-up at nine.",
             "subject": "stand-up", "quote": "stand-up at nine", "message": "u-1", "reason": ""}
        ]),
    );
    let out = hook_path(
        &fx,
        "user-prompt-submit",
        &prompt_payload(&claude, typed),
        &path,
    );
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: marked 2 times — cataloged after the reply",
        "no tag is refused at submit: markers are cataloged after the reply"
    );
    marked_turns(&claude, &[("u-1", typed)]);
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: nothing recorded for #scribe in your message — the model chose a tag that is not configured or not the one you named\n\
         scribe: added rituals-1 to rituals — \"Daniel holds stand-up at nine. The user and the agent discussed it in this session.\" · your words: \"stand-up at nine\""
    );
    let asked = claude_stdin(&fx, 0);
    assert!(asked.contains("- rituals:"), "the configured tags: {asked}");
    assert!(asked.contains("- marker 2 (hint: tag rituals)"), "{asked}");
    assert!(!asked.contains("hint: tag habits"), "{asked}");
    let lines = audit(&fx);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["command"], "hook stop");
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
    payload["prompt"] = json!("#scribe/all preferences: hello");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let reply = reply(&out);
    let message = reply["systemMessage"].as_str().unwrap();
    assert!(message.starts_with("scribe: refused"), "{message}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(
        context,
        format!("{message} (not recorded by scribe; relay this to the user)")
    );
    assert!(!context.contains("no action needed"), "{context}");
}

#[test]
fn a_failed_line_tells_the_agent_to_relay_it_to_daniel_not_that_no_action_is_needed() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe forget preferences-1");
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
        format!("{message} (not recorded by scribe; relay this to the user)")
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
        format!("{message} (not recorded by scribe; relay this to the user)")
    );
    assert!(!context.contains("no action needed"), "{context}");
}

#[test]
fn a_mixed_prompt_notes_each_line_by_whether_it_was_recorded() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_verbatim(
        &fx,
        1,
        "#scribe preferences: stand-up at nine",
        "an older prompt",
        1,
        Some("u-0"),
    );
    propose_stale(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "#scribe approve p-1\n#scribe approve p-99\nstand-up moves to ten #scribe preferences"
    );
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let reply = reply(&out);
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let lines: Vec<&str> = context.lines().collect();
    assert_eq!(lines.len(), 3, "{context}");
    assert!(
        lines[0].ends_with("(recorded by scribe; no action needed)"),
        "{context}"
    );
    assert!(
        lines[1].starts_with("scribe: refused")
            && lines[1].ends_with("(not recorded by scribe; relay this to the user)"),
        "{context}"
    );
    assert_eq!(
        lines[2],
        "scribe: marked — cataloged after the reply (scribe tries to catalog this #scribe marker after your reply and tells you if it gives up; record it yourself only then)"
    );
}

#[test]
fn the_audit_log_holds_ids_and_codes_never_what_was_said() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe approve marmalade");
    hook_ok(&fx, "user-prompt-submit", &payload);
    let typed = "the secret phrase is marmalade #scribe preferences";
    marked_turns(&claude, &[("u-1", typed)]);
    let path = annotating_claude(
        &fx,
        &json!([{"marker": 1, "kind": "entry", "tag": "preferences",
                 "statement": "Daniel set a secret phrase.", "subject": "the phrase",
                 "quote": "the secret phrase is marmalade", "message": "u-1", "reason": ""}]),
    );
    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let log = support::audit_text(&fx.root());
    for needle in [
        "marmalade",
        "secret phrase",
        "#scribe",
        claude.cwd.to_str().unwrap(),
    ] {
        assert!(!log.contains(needle), "{needle} leaked into {log}");
    }
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(codes, [json!("unparsed_instruction"), Value::Null]);
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
                "instruction_message",
                "instruction_session",
                "issue",
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
    payload["prompt"] = json!("#scribe forget preferences-1\n#scribe approve p-1");
    let elsewhere = fx.elsewhere(&format!("{SESSION}.jsonl"));
    payload["transcript_path"] = json!(elsewhere.to_str().unwrap());
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.starts_with("scribe: could not record \"#scribe forget preferences-1\""),
        "{message}"
    );
    assert!(message.contains("transcript check"), "{message}");
    assert!(entries(&fx).is_empty(), "nothing written");
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(
        codes,
        [json!("provenance_failed"), json!("provenance_failed")]
    );

    // A transcript named for another session fails too.
    let mut other = payload.clone();
    other["transcript_path"] = json!(claude.transcript.to_str().unwrap());
    other["session_id"] = json!("another-session");
    let out = hook_ok(&fx, "user-prompt-submit", &other);
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("not named for the payload's session_id")
    );

    // Stop checks the transcript before it catalogs anything.
    marked_turns(&claude, &[("u-1", MARKED)]);
    let path = annotating_claude(&fx, &none_for(1..=1));
    let mut stop = claude.payload("stop.json");
    stop["transcript_path"] = json!(elsewhere.to_str().unwrap());
    let out = hook_path(&fx, "stop", &stop, &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.starts_with("scribe: could not check for #scribe markers — "),
        "{message}"
    );
    assert!(claude_calls(&fx).is_empty());
    assert_eq!(
        audit(&fx).last().unwrap()["error_code"],
        "provenance_failed"
    );
    assert!(entries(&fx).is_empty());
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
        &prompt_payload(&claude, "#scribe forget preferences-1"),
    );
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("not a regular file")
    );
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("not a regular file")
    );
    assert!(entries(&fx).is_empty());
}

#[test]
fn a_bad_config_is_reported_in_the_reply_and_nothing_is_written() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_config(&fx, "[tags.preferences]\nalias = [\"typo\"]\n");
    marked_turns(&claude, &[("u-1", MARKED)]);
    let path = annotating_claude(&fx, &none_for(1..=1));
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.contains("is not a valid scribe config")
            && message.ends_with("; will retry at the next reply"),
        "{message}"
    );
    assert!(claude_calls(&fx).is_empty());
    // A bad config fails `entry list` itself, so look at the export.
    let export = tempfile::tempdir().unwrap();
    scribe_core::Store::open(fx.root())
        .export(
            &export.path().join("x"),
            scribe_core::ExportFormat::Markdown,
        )
        .unwrap();
    assert!(
        std::fs::read_dir(export.path().join("x/entries")).map_or(true, |mut d| d.next().is_none()),
        "no entry written"
    );
    assert!(marks_json(&fx).is_empty(), "the marker stays unsettled");
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(codes, [json!("config_invalid")]);
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

/// The fixture prompt's two `#scribe` lines as a store from before
/// markers recorded them (2_design.md §14.6), their message ids waiting.
fn write_fixture_entries(fx: &Fixture, prompt: &str) {
    write_verbatim(
        fx,
        1,
        "#scribe preferences: squash-merge agent PRs; keep merge commits for releases #merge-policy",
        prompt,
        2,
        None,
    );
    write_verbatim(
        fx,
        2,
        "#scribe add short commit subjects to my preferences",
        prompt,
        3,
        None,
    );
}

#[test]
fn the_stop_hook_fills_the_cite_message_uuid_from_the_transcript() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let payload = claude.payload("user-prompt-submit.json");
    let prompt = payload["prompt"].as_str().unwrap();
    write_fixture_entries(&fx, prompt);
    claude.write_transcript(prompt, "u-prompt");

    // The lines already hold entries, so they are not cataloged again.
    let path = fake_claude(&fx, "exit 1");
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(claude_calls(&fx).is_empty(), "no line cataloged twice");
    let entries = entries(&fx);
    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert_eq!(entry["cite"]["message"], "u-prompt", "{}", entry["id"]);
    }
    let lines = audit(&fx);
    let filled: Vec<&Value> = lines.iter().map(|l| &l["target"]).collect();
    assert_eq!(filled, [&json!("preferences-1"), &json!("preferences-2")]);
    assert!(lines.iter().all(|l| l["command"] == "hook stop"));

    // Nothing waits any more: the next Stop changes nothing at all.
    let before = snapshot(&fx.root());
    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(snapshot(&fx.root()), before);
}

#[test]
fn a_line_recorded_verbatim_in_one_block_of_a_message_is_not_cataloged_again() {
    // The prompt hook may have had one text block, not the joined message:
    // its cite lines count from that block.
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let block = "#scribe preferences: use rg\nuse fd for finding files #scribe preferences";
    write_verbatim(&fx, 1, "#scribe preferences: use rg", block, 1, Some("u-1"));
    write_lines(
        &claude,
        &[
            json!({"type": "session-owner", "sessionId": SESSION}),
            json!({"type": "user", "uuid": "u-1", "sessionId": SESSION, "timestamp": now_rfc3339(),
                   "message": {"role": "user", "content": [
                       {"type": "text", "text": "Two tools."},
                       {"type": "text", "text": block}]}}),
            json!({"type": "assistant", "uuid": "a-1", "sessionId": SESSION, "timestamp": now_rfc3339(),
                   "message": {"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}}),
        ],
    );
    let path = annotating_claude(
        &fx,
        &json!([{"marker": 2, "kind": "entry", "tag": "preferences",
                 "statement": "Daniel finds files with fd.", "subject": "finding files",
                 "quote": "use fd for finding files", "message": "u-1", "reason": ""}]),
    );
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let echo = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        echo.starts_with("scribe: added preferences-2 to preferences"),
        "{echo}"
    );
    let asked = claude_stdin(&fx, 0);
    assert!(!asked.contains("- marker 1"), "{asked}");
    assert!(
        asked.contains("- marker 2 (hint: tag preferences)"),
        "{asked}"
    );
    let marks = marks_json(&fx);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0]["marker"], 2);
    assert_eq!(entries(&fx).len(), 2);
}

#[test]
fn stop_never_replays_historical_settling_commands() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_verbatim(
        &fx,
        1,
        "#scribe preferences: keep this entry",
        "an older prompt",
        1,
        Some("u-0"),
    );
    claude.write_transcript("#scribe forget preferences-1", "u-settle");
    let before = entries_snapshot(&fx);
    let audit_before = audit(&fx);
    let path = fake_claude(&fx, "exit 1");
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(
        claude_calls(&fx).is_empty(),
        "a settling line is not a marker"
    );
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
    let payload = claude.payload("user-prompt-submit.json");
    write_fixture_entries(&fx, payload["prompt"].as_str().unwrap());
    claude.write_transcript("a different prompt", "u-other");
    hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let entries = entries(&fx);
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert_eq!(entry["cite"]["message"], Value::Null);
    }
}

// --- a fake `claude` on PATH ----------------------------------------------

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
         cat > \"$HOME/claude-stdin-$n\"\n\
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

fn prompt_payload(claude: &Claude, prompt: &str) -> Value {
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(prompt);
    payload
}

/// What the fake `claude`'s call `n` read on stdin: the prompt.
fn claude_stdin(fx: &Fixture, n: usize) -> String {
    std::fs::read_to_string(fx.home().join(format!("claude-stdin-{n}"))).unwrap()
}

/// An entry recorded verbatim before markers took the line over
/// (2_design.md §14.6): `preferences-<n>`, saying `said`, citing `line` of
/// `prompt` in this session, its message id still waiting when `message`
/// is `None`.
fn write_verbatim(
    fx: &Fixture,
    n: usize,
    said: &str,
    prompt: &str,
    line: u32,
    message: Option<&str>,
) {
    let dir = fx.root().join("entries/preferences");
    std::fs::create_dir_all(&dir).unwrap();
    for d in [fx.root(), fx.root().join("entries"), dir.clone()] {
        make_private(&d);
    }
    let content = said.split_once(": ").map_or(said, |(_, c)| c);
    let sha = scribe_core::capture::prompt_sha256(prompt);
    let message = message.map_or_else(|| String::from("null"), |m| format!("\"{m}\""));
    let text = format!(
        "---\nformat: 1\nkind: entry\nid: preferences-{n}\ntag: preferences\nstatus: active\n\
         project: scribe\ntopics: []\nrelated: []\nprovider: claude-code\nhost: host\n\
         created: 2026-01-02T03:04:05Z\nupdated: 2026-01-02T03:04:05Z\nsaid: \"{said}\"\n\
         content_from: {{kind: instruction}}\n\
         cite: {{session: {SESSION}, message: {message}, at: 2026-01-02T03:04:05Z, prompt_sha256: \"{sha}\", line: {line}, cwd: /work}}\n\
         superseded_by: null\nreview_after: null\n---\n{content}\n"
    );
    let path = dir.join(format!("preferences-{n}.md"));
    std::fs::write(&path, text).unwrap();
    make_private(&path);
}

/// An open stale proposal `p-1` on `preferences-1`.
fn propose_stale(fx: &Fixture) {
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
}

#[test]
fn a_line_start_form_gives_the_model_its_tag_as_the_hint_through_a_safe_mode_claude() {
    let fx = Fixture::new();
    fx.config(support::IDEA_CATCHER);
    let claude = Claude::new(&fx);
    let typed =
        "#scribe idea: a dry-run flag for prune\n#scribe add this to preferences #merge-policy";
    marked_turns(
        &claude,
        &[("u-0", "We should squash-merge agent PRs."), ("u-1", typed)],
    );
    let path = annotating_claude(
        &fx,
        &json!([
            {"marker": 1, "kind": "entry", "tag": "idea", "statement": "Prune could take a dry-run flag.",
             "subject": "prune", "quote": "a dry-run flag for prune", "message": "u-1", "reason": ""},
            {"marker": 2, "kind": "entry", "tag": "preferences",
             "statement": "Daniel squash-merges agent PRs.", "subject": "merging agent PRs",
             "quote": "We should squash-merge agent PRs", "message": "u-0", "reason": ""}
        ]),
    );
    let stop = claude.payload("stop.json");
    let out = hook_path_env(
        &fx,
        "stop",
        &stop,
        &path,
        &[("GITHUB_TOKEN", "not-for-the-model")],
    );
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: added idea-1 to idea — \"Prune could take a dry-run flag. The user and the agent discussed it in this session.\" · your words: \"a dry-run flag for prune\"\n\
         scribe: added preferences-1 to preferences — \"Daniel squash-merges agent PRs. The user and the agent discussed it in this session.\" · your words: \"We should squash-merge agent PRs\""
    );
    let asked = claude_stdin(&fx, 0);
    assert!(
        asked.contains("- marker 1 (hint: tag idea): #scribe idea:"),
        "{asked}"
    );
    assert!(
        asked.contains("- marker 2 (hint: tag preferences): #scribe add this to preferences"),
        "{asked}"
    );
    let stored = entries(&fx);
    let entry = by_said(&stored, "#scribe add this to preferences #merge-policy");
    assert_eq!(entry["tag"], "preferences");
    assert_eq!(entry["quote"], "We should squash-merge agent PRs");
    assert_eq!(entry["topics"], json!(["merge-policy"]));
    assert_eq!(entry["cite"]["line"], 2);
    assert_eq!(entry["cite"]["message"], "u-1");
    let targets: Vec<Value> = audit(&fx).iter().map(|l| l["target"].clone()).collect();
    assert_eq!(targets, [json!("idea-1"), json!("preferences-1")]);

    // The call: safe mode, no tools, the configured annotation model.
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

    // The next Stop neither calls nor echoes.
    let out = hook_path(&fx, "stop", &stop, &path);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert_eq!(claude_calls(&fx).len(), 1);
    assert_eq!(entries(&fx).len(), 2);
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
    let typed = "#scribe keep this note to note";
    marked_turns(&claude, &[("u-note", typed)]);
    let path = annotating_claude(
        &fx,
        &json!([{"marker": 1, "kind": "entry", "tag": "notes",
                 "statement": "Daniel wants this note kept.", "subject": "the note",
                 "quote": "keep this note", "message": "u-note", "reason": ""}]),
    );
    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert!(
        claude_stdin(&fx, 0).contains("(hint: tag notes)"),
        "an alias at the line's end is the hint"
    );
    let recorded = entries(&fx);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["tag"], "notes");
    assert_eq!(recorded[0]["author"]["kind"], "user");
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
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe approve p-1");
    claude.write_transcript("#scribe approve p-1", "u-approve");
    hook_ok(&fx, "user-prompt-submit", &payload);
    assert_eq!(entries(&fx)[0]["status"], "retired");
    let proposals: Value =
        serde_json::from_slice(&fx.ok(&["proposal", "list", "--json"]).stdout).unwrap();
    assert_eq!(proposals["proposals"][0]["settled_author"]["kind"], "user");
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
    support::exported(&fx.root(), &format!("entries/{tag}/{id}.md")).unwrap()
}

fn by_said<'a>(entries: &'a [Value], said: &str) -> &'a Value {
    entries
        .iter()
        .find(|entry| entry["said"] == said)
        .unwrap_or_else(|| panic!("no entry said {said}: {entries:?}"))
}

#[test]
fn each_marker_records_global_or_the_cwd_project() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let typed = "\
#scribe/global preferences: squash-merge agent PRs
#scribe preferences: keep merge commits for releases
#scribe/global add this to preferences";
    marked_turns(
        &claude,
        &[("u-0", "I want short commit subjects."), ("u-1", typed)],
    );
    let answer = |marker: u32, quote: &str, message: &str, statement: &str| {
        json!({"marker": marker, "kind": "entry", "tag": "preferences", "statement": statement,
               "subject": "agent PRs", "quote": quote, "message": message, "reason": ""})
    };
    let path = annotating_claude(
        &fx,
        &json!([
            answer(
                1,
                "squash-merge agent PRs",
                "u-1",
                "Daniel squash-merges agent PRs."
            ),
            answer(
                2,
                "keep merge commits for releases",
                "u-1",
                "Releases keep merge commits."
            ),
            answer(
                3,
                "short commit subjects",
                "u-0",
                "Daniel wants short commit subjects."
            ),
        ]),
    );
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert_eq!(message.matches("scribe: added").count(), 3, "{message}");

    let global = listed(&fx, "global");
    let local = listed(&fx, "scribe");
    assert_eq!(global.len(), 2, "{global:?}");
    assert_eq!(local.len(), 1, "{local:?}");
    let expect = [
        (
            &global,
            "#scribe/global preferences: squash-merge agent PRs",
            "global",
        ),
        (
            &local,
            "#scribe preferences: keep merge commits for releases",
            "scribe",
        ),
        (&global, "#scribe/global add this to preferences", "global"),
    ];
    for (entries, said, project) in expect {
        let entry = by_said(entries, said);
        assert_eq!(entry["tag"], "preferences");
        assert_eq!(entry["project"], project);
        assert_eq!(entry["author"]["kind"], "user");
        let front = frontmatter(&fx, entry);
        assert!(front.contains(&format!("project: {project}")), "{front}");
        assert!(front.contains("kind: user"), "{front}");
        assert!(!front.contains("kind: agent"), "{front}");
    }
}

#[test]
fn a_malformed_opener_or_a_global_settling_line_is_refused_and_teaches_the_opener() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe/all preferences: hello");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert_eq!(message.lines().count(), 1, "{message}");
    let reason = message
        .split(" — ")
        .nth(1)
        .unwrap_or_else(|| panic!("no reason: {message}"));
    assert!(reason.contains("#scribe/global"), "{reason}");
    assert!(
        !fx.root().exists(),
        "a refused marker writes nothing at submit"
    );

    write_verbatim(
        &fx,
        1,
        "#scribe preferences: keep settling unmarked",
        "an older prompt",
        1,
        Some("u-0"),
    );
    propose_stale(&fx);
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
fn session_start_suggests_a_single_record_and_names_both_when_ambiguous() {
    let fx = Fixture::new();
    let cwd = project_cwd(&fx);
    let now_dt = time::OffsetDateTime::now_utc();
    let earlier_dt = now_dt - time::Duration::hours(1);
    let format = &time::format_description::well_known::Rfc3339;
    let now = now_dt.format(format).unwrap();
    let earlier = earlier_dt.format(format).unwrap();
    write_session(&fx, PREVIOUS, "scribe", &earlier, &earlier, true);
    let alone = hook_ok(&fx, "session-start", &start_payload(Some(&cwd), CURRENT));
    let alone_reply = reply(&alone);
    assert_eq!(
        alone_reply["systemMessage"].as_str(),
        Some(continuation_line(PREVIOUS).as_str())
    );
    assert!(
        alone_reply.get("hookSpecificOutput").is_none(),
        "the agent is not told: {alone_reply}"
    );

    write_session(&fx, NEWER, "scribe", &now, &now, true);
    write_open_proposal(&fx);
    let before = snapshot(&fx.root());
    let message = start_message(&fx, &start_payload(Some(&cwd), CURRENT));
    assert_eq!(
        message,
        format!(
            "scribe: 1 proposal to review — scribe proposal list\nscribe: possible recorded predecessors {PREVIOUS}, {NEWER} — type /scribe on --from <id> to choose one"
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
    // The export reads `sessions/` too, so make it readable first.
    drop(_restore);
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
fn decision_and_open_item_markers_are_refused_before_a_model_call() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    marked_turns(
        &claude,
        &[
            (
                "u-1",
                "#scribe decisions: squash-merge agent PRs\n#scribe decision: rebase never\n#scribe dec: keep merge commits",
            ),
            (
                "u-2",
                "#scribe open-item: Which check runs?\n#scribe item: Restart the server\n#scribe open-items: Which host?",
            ),
        ],
    );
    let path = annotating_claude(&fx, &json!([]));
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert_eq!(
        message
            .matches("record decisions and open items through scribe entry add")
            .count(),
        6,
        "{message}"
    );
    assert!(!fx.home().join("claude-stdin-0").exists());
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(codes, vec![json!("unknown_tag"); 6]);
    assert!(entries(&fx).is_empty());
}

#[test]
fn a_config_that_declares_decisions_makes_it_work_like_any_config_tag() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_config(
        &fx,
        "[tags.decisions]\naliases = [\"decision\", \"dec\"]\ndescription = \"choices Daniel has made\"\n",
    );
    let typed =
        "#scribe decisions: squash-merge agent PRs\n#scribe dec: keep merge commits for releases";
    marked_turns(&claude, &[("u-1", typed)]);
    let answer = |marker: u32, quote: &str, statement: &str| {
        json!({"marker": marker, "kind": "entry", "tag": "decisions", "statement": statement,
               "subject": "merging", "quote": quote, "message": "u-1", "reason": ""})
    };
    let path = annotating_claude(
        &fx,
        &json!([
            answer(
                1,
                "squash-merge agent PRs",
                "Daniel squash-merges agent PRs."
            ),
            answer(
                2,
                "keep merge commits for releases",
                "Releases keep merge commits."
            ),
        ]),
    );
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: added decisions-1 to decisions — \"Daniel squash-merges agent PRs. The user and the agent discussed it in this session.\" · your words: \"squash-merge agent PRs\"\n\
         scribe: added decisions-2 to decisions — \"Releases keep merge commits. The user and the agent discussed it in this session.\" · your words: \"keep merge commits for releases\""
    );
    assert_eq!(
        claude_stdin(&fx, 0)
            .matches("(hint: tag decisions)")
            .count(),
        2
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
fn a_config_naming_the_retired_referent_model_loads_with_a_warning() {
    let fx = Fixture::new();
    write_config(&fx, "[models]\nreferent = \"claude-haiku-5-5\"\n");
    let out = fx.ok(&["tag", "list", "--json"]);
    let err = text(&out.stderr);
    assert!(
        err.contains("config key models.referent is unused"),
        "{err}"
    );
    assert_eq!(
        err.matches("models.referent").count(),
        1,
        "warned once: {err}"
    );
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
    let found = fx.ok(&["search", "squash", "--json"]);
    assert!(
        text(&found.stdout).contains("decisions-2"),
        "search finds it: {}",
        text(&found.stdout)
    );

    // The alias requires typed fields for a new decision. The older entries
    // still settle by their original ids.
    let refused = fx.run(&[
        "entry",
        "add",
        "--tag",
        "decisions",
        "--author",
        "opus",
        "a new one",
    ]);
    assert_ne!(refused.status.code(), Some(0));
    assert!(
        text(&refused.stderr).contains("a decision requires --decided-by"),
        "{}",
        text(&refused.stderr)
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

#[test]
fn a_marker_quoting_a_stated_entrys_words_again_is_covered() {
    // A stated entry keeps scribe's statement as content and Daniel's
    // words in `quote` (2_design.md §11.1, §14.3); marking those words
    // again is covered by it.
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let quote = "always run the whole gate before a push";
    let entry_for = |message: &str| {
        json!([{"marker": 1, "kind": "entry", "tag": "preferences",
                "statement": "Daniel requires the complete check suite to pass ahead of every upload.",
                "subject": "the gate", "quote": quote, "message": message, "reason": ""}])
    };
    let first = format!("{quote} #scribe preferences");
    marked_turns(&claude, &[("u-1", &first)]);
    let path = annotating_claude(&fx, &entry_for("u-1"));
    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let shown = fx.ok(&["entry", "show", "preferences-1", "--json"]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["quote"], quote);
    assert_eq!(shown["stated_by"]["name"], "annotation");

    let again = format!("a later turn: {quote} #scribe preferences");
    marked_turns(&claude, &[("u-1", &first), ("u-2", &again)]);
    let path = annotating_claude(&fx, &entry_for("u-2"));
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: nothing recorded for #scribe in your message — already covered by preferences-1 in preferences"
    );
    assert_eq!(entries(&fx).len(), 1);
    assert_eq!(audit(&fx).last().unwrap()["error_code"], "covered");
}

// --- annotations: `#scribe` inside a sentence, cataloged at Stop (§14) ---

const MARKED: &str = "I always want short status updates #scribe preferences";

fn hook_path(fx: &Fixture, event: &str, payload: &Value, path: &std::ffi::OsString) -> Output {
    hook_path_env(fx, event, payload, path, &[])
}

/// [`hook_path`] with `extra` in the hook's environment.
fn hook_path_env(
    fx: &Fixture,
    event: &str,
    payload: &Value,
    path: &std::ffi::OsString,
    extra: &[(&str, &str)],
) -> Output {
    let mut cmd = fx.cmd(&["hook", event]);
    cmd.env("PATH", path)
        .env("CLAUDE_CODE_OAUTH_TOKEN", "oauth-test-value")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra {
        cmd.env(key, value);
    }
    let child = cmd.spawn().unwrap();
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

/// A fake `claude` answering every call with `marks`.
fn annotating_claude(fx: &Fixture, marks: &Value) -> std::ffi::OsString {
    let answer = json!({"type": "result", "subtype": "success", "is_error": false,
                        "result": "", "structured_output": {"marks": support::with_mark_contexts(marks)}});
    fake_claude(fx, &format!("echo '{answer}'"))
}

fn none_for(markers: std::ops::RangeInclusive<u32>) -> Value {
    json!(
        markers
            .map(
                |marker| json!({"marker": marker, "kind": "none", "tag": "", "statement": "",
                                 "subject": "", "quote": "", "message": "",
                                 "reason": "only a passing remark"})
            )
            .collect::<Vec<_>>()
    )
}

/// The transcript: one typed turn per `(uuid, prompt)`, each with a reply.
fn marked_turns(claude: &Claude, turns: &[(&str, &str)]) {
    let mut lines = vec![json!({"type": "session-owner", "sessionId": SESSION})];
    for (uuid, prompt) in turns {
        lines.push(json!({"type": "user", "uuid": uuid, "sessionId": SESSION,
                          "timestamp": now_rfc3339(),
                          "message": {"role": "user", "content": prompt}}));
        lines.push(json!({"type": "assistant", "uuid": format!("a-{uuid}"), "sessionId": SESSION,
                          "timestamp": now_rfc3339(),
                          "message": {"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}}));
    }
    write_lines(claude, &lines);
}

fn marks_json(fx: &Fixture) -> Vec<Value> {
    match support::exported(&fx.root(), "marks.json") {
        Some(doc) => serde_json::from_str::<Value>(&doc).unwrap()["marks"]
            .as_array()
            .unwrap()
            .clone(),
        None => Vec::new(),
    }
}

#[test]
fn a_marker_inside_a_sentence_is_echoed_at_submit_and_writes_nothing() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let path = annotating_claude(&fx, &none_for(1..=1));
    let out = hook_path(
        &fx,
        "user-prompt-submit",
        &prompt_payload(&claude, MARKED),
        &path,
    );
    let reply = reply(&out);
    assert_eq!(
        reply["systemMessage"],
        "scribe: marked — cataloged after the reply"
    );
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("tells you if it gives up; record it yourself only then"),
        "{context}"
    );
    assert!(claude_calls(&fx).is_empty(), "no model call at submit");
    assert!(!fx.root().exists(), "nothing written at submit");
}

#[test]
fn a_marker_after_a_stray_quote_or_ending_a_global_sentence_is_echoed_and_cataloged() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    // A lone `"` (inches) opens no quoted span, and sentence punctuation
    // may follow `#scribe/global`.
    let typed = "I buy the 12\" pipe from the corner shop #scribe/global.";
    let path = annotating_claude(
        &fx,
        &json!([{"marker": 1, "kind": "entry", "tag": "preferences",
                 "statement": "Daniel buys the 12-inch pipe from the corner shop.",
                 "subject": "pipe", "quote": "I buy the 12\" pipe from the corner shop",
                 "message": "u-1", "reason": ""}]),
    );
    let submitted = hook_path(
        &fx,
        "user-prompt-submit",
        &prompt_payload(&claude, typed),
        &path,
    );
    assert_eq!(
        reply(&submitted)["systemMessage"],
        "scribe: marked — cataloged after the reply"
    );
    marked_turns(&claude, &[("u-1", typed)]);
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(message.contains("scribe: added"), "{message}");
    let global = listed(&fx, "global");
    assert_eq!(global.len(), 1, "{global:?}");
    let marks = marks_json(&fx);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0]["outcome"], "entry");
}

#[test]
fn stop_catalogs_a_marked_turn_as_daniels_entry_with_scribes_context_once() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    marked_turns(&claude, &[("u-1", MARKED)]);
    let path = annotating_claude(
        &fx,
        &json!([{"marker": 1, "kind": "entry", "tag": "preferences",
                 "statement": "Daniel wants short status updates.", "subject": "status updates",
                 "quote": "I always want short status updates", "message": "u-1", "reason": ""}]),
    );
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: added preferences-1 to preferences — \"Daniel wants short status updates. The user and the agent discussed it in this session.\" · your words: \"I always want short status updates\""
    );
    let calls = claude_calls(&fx);
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].contains(&String::from("<claude-haiku-5-5>")),
        "{:?}",
        calls[0]
    );
    let stored = entries(&fx);
    assert_eq!(stored.len(), 1);
    let entry = &stored[0];
    assert_eq!(
        entry["content"],
        support::context_of("Daniel wants short status updates.")
    );
    assert_eq!(entry["quote"], "I always want short status updates");
    assert_eq!(entry["author"]["kind"], "user");
    assert_eq!(
        entry["stated_by"],
        json!({"kind": "scribe", "name": "annotation"})
    );
    assert_eq!(entry["said"], MARKED);
    assert_eq!(entry["cite"]["message"], "u-1");
    assert!(
        support::exported(&fx.root(), "proposals.json").is_none_or(|doc| {
            serde_json::from_str::<Value>(&doc).unwrap()["proposals"]
                .as_array()
                .is_none_or(Vec::is_empty)
        }),
        "no proposal"
    );
    let marks = marks_json(&fx);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0]["outcome"], "entry");
    assert_eq!(marks[0]["entry"], "preferences-1");
    assert!(
        audit(&fx)
            .iter()
            .any(|l| l["target"] == "preferences-1" && l["command"] == "hook stop"),
        "{:?}",
        audit(&fx)
    );

    // The next Stop neither calls nor echoes.
    let again = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert!(again.stdout.is_empty(), "{}", text(&again.stdout));
    assert_eq!(claude_calls(&fx).len(), 1);
    assert_eq!(entries(&fx).len(), 1);
}

#[test]
fn a_none_answer_is_echoed_recorded_and_never_asked_again() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    marked_turns(&claude, &[("u-1", MARKED)]);
    let path = annotating_claude(&fx, &none_for(1..=1));
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: nothing recorded for #scribe in your message — only a passing remark"
    );
    assert_eq!(marks_json(&fx)[0]["outcome"], "none");
    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(claude_calls(&fx).len(), 1);
    assert!(entries(&fx).is_empty());
}

#[test]
fn a_failing_model_stays_pending_for_three_attempts_then_refuses_model_failed() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    marked_turns(&claude, &[("u-1", MARKED)]);
    let path = fake_claude(&fx, "exit 1");
    for attempt in 1..=2 {
        let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
        let echo = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
        assert!(
            echo.starts_with("scribe: could not catalog #scribe in your message — "),
            "{echo}"
        );
        assert!(
            echo.ends_with(&format!("; will retry at the next reply ({attempt} of 3)")),
            "{echo}"
        );
        assert_eq!(marks_json(&fx)[0]["outcome"], "pending");
    }
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let reply = reply(&out);
    let echo = reply["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        echo.ends_with(&format!(
            "; gave up after 3 attempts — scribe mark retry {SESSION} tries again"
        )),
        "{echo}"
    );
    // The agent is told too, with the line, so it is not lost.
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "Stop");
    assert!(
        context.starts_with("scribe: could not catalog #scribe in your message — "),
        "{context}"
    );
    assert!(context.contains("gave up after 3 attempts"), "{context}");
    assert!(context.contains(MARKED), "{context}");
    assert!(context.contains("not cataloged by scribe"), "{context}");
    assert!(context.contains("scribe entry add --tag"), "{context}");
    let marks = marks_json(&fx);
    assert_eq!(marks[0]["outcome"], "refused");
    assert_eq!(marks[0]["code"], "model_failed");
    assert_eq!(marks[0]["attempts"], 3);
    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(claude_calls(&fx).len(), 3, "never a fourth call");
    assert!(entries(&fx).is_empty());
    assert!(audit(&fx).iter().any(|l| l["error_code"] == "model_failed"));
}

#[test]
fn a_fourth_marker_is_refused_too_many_marks_before_any_call() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let four = "a #scribe b #scribe c #scribe d #scribe";
    let path = annotating_claude(&fx, &none_for(1..=3));
    let submitted = hook_path(
        &fx,
        "user-prompt-submit",
        &prompt_payload(&claude, four),
        &path,
    );
    let echo = reply(&submitted)["systemMessage"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        echo.contains("scribe: marked 3 times — cataloged after the reply"),
        "{echo}"
    );
    assert!(
        echo.contains("at most 3 #scribe markers in one message"),
        "{echo}"
    );

    marked_turns(&claude, &[("u-1", four)]);
    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(claude_calls(&fx).len(), 1, "one call for the turn");
    let marks = marks_json(&fx);
    assert_eq!(marks.len(), 4);
    let fourth = marks.iter().find(|m| m["marker"] == 4).unwrap();
    assert_eq!(fourth["outcome"], "refused");
    assert_eq!(fourth["code"], "too_many_marks");
    assert!(
        audit(&fx)
            .iter()
            .any(|l| l["error_code"] == "too_many_marks")
    );
}

#[test]
fn one_stop_catalogs_at_most_two_marked_turns_oldest_first() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    marked_turns(
        &claude,
        &[
            ("u-1", "first thought #scribe"),
            ("u-2", "second thought #scribe"),
            ("u-3", "third thought #scribe"),
        ],
    );
    let path = annotating_claude(&fx, &none_for(1..=1));
    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(claude_calls(&fx).len(), 2);
    let before = marks_json(&fx);
    assert_eq!(before.len(), 3);
    assert_eq!(before[2]["message"], "u-3");
    assert_eq!(before[2]["outcome"], "pending");
    assert_eq!(before[2]["attempts"], 0);
    let audit_before = audit(&fx);
    let listed = mark_cmd(&fx, &["mark", "list", "--json"], &path);
    assert!(listed.status.success(), "{listed:?}");
    let doc: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(doc["total"], 1, "{doc}");
    assert_eq!(doc["marks"][0]["message"], "u-3");
    assert_eq!(marks_json(&fx), before, "list must not write marks");
    assert_eq!(audit(&fx), audit_before, "list must not audit");
    assert_eq!(claude_calls(&fx).len(), 2, "list must not call a model");
    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(claude_calls(&fx).len(), 3);
    assert_eq!(marks_json(&fx).len(), 3);
}

#[test]
fn a_mention_or_an_agent_turn_is_never_cataloged() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    marked_turns(
        &claude,
        &[
            ("u-1", "the `#scribe` marker and \"#scribe\" in quotes"),
            (
                "u-2",
                "<agent-message from=\"x\">\nplease note #scribe\n</agent-message>",
            ),
        ],
    );
    let path = annotating_claude(&fx, &none_for(1..=1));
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(claude_calls(&fx).is_empty());
    assert!(marks_json(&fx).is_empty());
}

#[test]
fn an_annotation_under_an_action_tag_keeps_its_window() {
    let fx = Fixture::new();
    fx.config(support::IDEA_CATCHER);
    let claude = Claude::new(&fx);
    let typed = "so here is what I am thinking: scribe could read annotations #scribe idea";
    marked_turns(&claude, &[("u-0", "an earlier turn"), ("u-1", typed)]);
    let path = annotating_claude(
        &fx,
        &json!([{"marker": 1, "kind": "entry", "tag": "idea",
                 "statement": "Scribe could read annotations.", "subject": "annotations",
                 "quote": "scribe could read annotations", "message": "u-1", "reason": ""}]),
    );
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let echo = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(echo.starts_with("scribe: added idea-1 to idea"), "{echo}");
    let stored = entries(&fx);
    let entry = by_said(&stored, typed);
    let shown = fx.ok(&[
        "entry",
        "show",
        entry["id"].as_str().unwrap(),
        "--window",
        "--json",
    ]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    let window = shown.to_string();
    assert!(window.contains("an earlier turn"), "{window}");
    assert_eq!(entry["stated_by"]["name"], "annotation");
}

/// Run `hook session-start` as Claude Code starts a session from
/// `entrypoint` (`cli` is interactive).
fn start_from(fx: &Fixture, entrypoint: &str, payload: &Value) -> Value {
    let child = fx
        .cmd(&["hook", "session-start"])
        .env("CLAUDE_CODE_ENTRYPOINT", entrypoint)
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
    reply(&out)
}

#[test]
fn session_start_gives_an_interactive_agent_the_brief() {
    let fx = Fixture::new();
    let cwd = project_cwd(&fx);
    write_session(
        &fx,
        PREVIOUS,
        "scribe",
        "2026-11-16T10:00:00Z",
        "2026-11-16T11:00:00Z",
        true,
    );
    write_session(
        &fx,
        NEWER,
        "orbit",
        "2026-11-17T10:00:00Z",
        "2026-11-17T11:00:00Z",
        true,
    );
    write_session(
        &fx,
        CURRENT,
        "scribe",
        "2026-11-18T10:00:00Z",
        "2026-11-18T11:00:00Z",
        true,
    );
    write_open_proposal(&fx);
    let before = snapshot(&fx.root());
    let payload = start_payload(Some(&cwd), CURRENT);

    let interactive = start_from(&fx, "cli", &payload);
    let context = interactive["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("{interactive}"));
    assert!(context.starts_with("# scribe brief\n"), "{context}");
    // This project's session first, then another project's; never the
    // session starting.
    assert!(
        context.contains(
            "## Recent sessions\n- s-1 · 2026-11-16 11:00Z · scribe · Continue the work\n- s-2 · 2026-11-17 11:00Z · orbit · Continue the work\n"
        ),
        "{context}"
    );
    assert!(!context.contains(CURRENT), "{context}");
    assert!(
        context.contains(
            "## Open claims\n- p-1 · 2026-11-16 12:00Z · rituals · \"run the full gate\"\n"
        ),
        "{context}"
    );
    // The user's count stays beside it.
    assert_eq!(
        interactive["systemMessage"],
        "scribe: 1 proposal to review — scribe proposal list"
    );

    let headless = start_from(&fx, "sdk-cli", &payload);
    assert_eq!(headless["hookSpecificOutput"], Value::Null, "{headless}");
    assert!(audit(&fx).is_empty(), "the brief is not audited");
    assert_eq!(
        snapshot(&fx.root()),
        before,
        "session-start wrote the store"
    );
}

#[test]
fn claude_desktop_origin_shapes_match_cli_verdicts_in_hooks() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);

    // Human desktop payload on `user-prompt-submit` echoes `#scribe` markers.
    let mut human = claude.payload("user-prompt-submit.json");
    human["prompt"] = json!("always prefer explicit dates #scribe preferences");
    human["origin"] = json!({"kind": "human"});
    human["promptSource"] = json!("sdk");
    human["turnOrigin"] = json!("human");
    let out = hook_ok(&fx, "user-prompt-submit", &human);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: marked — cataloged after the reply"
    );

    // Peer, task-notification, and host-synthetic payloads are skipped silently.
    for (name, extra) in [
        (
            "peer",
            json!({
                "origin": {"kind": "peer", "from": "local_1", "name": "peer", "body": "#scribe preferences: no", "hostInjected": true},
                "promptSource": "system",
                "turnOrigin": "peer",
                "isMeta": true
            }),
        ),
        (
            "task-notification",
            json!({
                "origin": {"kind": "task-notification", "producer": "session-task", "runId": "r-1"},
                "promptSource": "system",
                "turnOrigin": "task_notification"
            }),
        ),
        (
            "host-synthetic",
            json!({
                "promptSource": "sdk",
                "turnOrigin": "host_synthetic"
            }),
        ),
    ] {
        let mut payload = claude.payload("user-prompt-submit.json");
        payload["prompt"] = json!("#scribe preferences: agent wrote this");
        if let (Some(obj), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        let out = hook_ok(&fx, "user-prompt-submit", &payload);
        assert!(
            out.stdout.is_empty(),
            "{name} should be silent: {}",
            text(&out.stdout)
        );
    }

    // `hook stop` annotation cataloging accepts the human desktop transcript line
    // and skips the three non-human desktop transcript lines.
    let typed = "always prefer explicit dates #scribe preferences";
    let at = now_rfc3339();
    let make_line = |uuid: &str, text: &str, extra: Value| {
        let mut line = json!({
            "type": "user",
            "uuid": uuid,
            "sessionId": SESSION,
            "timestamp": at,
            "cwd": claude.cwd,
            "entrypoint": "claude-desktop",
            "message": {"role": "user", "content": text}
        });
        if let (Some(obj), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        format!("{line}\n")
    };
    let progress = format!(
        "{}\n",
        json!({
            "type": "progress",
            "uuid": "u-human",
            "origin": {"kind": "peer"},
            "promptSource": "system",
            "turnOrigin": "peer"
        })
    );
    let transcript = [
        progress,
        make_line(
            "u-human",
            typed,
            json!({"origin": {"kind": "human"}, "promptSource": "sdk", "turnOrigin": "human"}),
        ),
        make_line(
            "u-peer",
            "#scribe preferences: peer",
            json!({"origin": {"kind": "peer"}, "promptSource": "system", "turnOrigin": "peer", "isMeta": true}),
        ),
        make_line(
            "u-task",
            "#scribe preferences: task",
            json!({"origin": {"kind": "task-notification"}, "promptSource": "system", "turnOrigin": "task_notification"}),
        ),
        make_line(
            "u-synth",
            "#scribe preferences: synth",
            json!({"promptSource": "sdk", "turnOrigin": "host_synthetic"}),
        ),
    ]
    .concat();
    std::fs::write(&claude.transcript, transcript).unwrap();
    make_private(&claude.transcript);
    let path = annotating_claude(
        &fx,
        &json!([{
            "marker": 1,
            "kind": "entry",
            "tag": "preferences",
            "statement": "Daniel prefers explicit dates.",
            "subject": "dates",
            "quote": "always prefer explicit dates",
            "message": "u-human",
            "reason": ""
        }]),
    );
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let msg = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(msg.starts_with("scribe: added preferences-1"), "{msg}");
    assert_eq!(claude_calls(&fx).len(), 1, "only u-human sent to model");
    assert_eq!(entries(&fx).len(), 1);
}

#[test]
fn stop_uses_the_retained_user_row_instead_of_same_uuid_progress_origin() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let typed = "always prefer explicit dates #scribe preferences";
    let at = now_rfc3339();
    let progress = json!({
        "type": "progress",
        "uuid": "u-synth",
        "origin": {"kind": "human"},
        "promptSource": "sdk",
        "turnOrigin": "human"
    });
    let synthetic = json!({
        "type": "user",
        "uuid": "u-synth",
        "sessionId": SESSION,
        "timestamp": at,
        "cwd": claude.cwd,
        "entrypoint": "claude-desktop",
        "message": {"role": "user", "content": typed},
        "promptSource": "sdk",
        "turnOrigin": "host_synthetic"
    });
    write_lines(&claude, &[progress, synthetic]);
    let path = annotating_claude(
        &fx,
        &json!([{
            "marker": 1,
            "kind": "entry",
            "tag": "preferences",
            "statement": "Daniel prefers explicit dates.",
            "subject": "dates",
            "quote": "always prefer explicit dates",
            "message": "u-synth",
            "reason": ""
        }]),
    );

    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);

    assert!(
        claude_calls(&fx).is_empty(),
        "synthetic turn is not sent to the model"
    );
    assert!(entries(&fx).is_empty(), "synthetic turn creates no entry");
    assert!(
        marks_json(&fx).is_empty(),
        "synthetic turn writes no mark outcome"
    );
}

/// A Claude desktop transcript of a session that was never switched on: a
/// slash-command turn whose args carry a marker, then a line Daniel typed
/// while the agent was busy (a `queued_command` attachment), both marked.
fn typed_forms_transcript(claude: &Claude, slash: &str, queued: &str) {
    let at = now_rfc3339();
    let desktop = |line: Value| {
        let mut line = line;
        let obj = line.as_object_mut().unwrap();
        obj.insert("sessionId".into(), json!(SESSION));
        obj.insert("timestamp".into(), json!(at));
        obj.insert("cwd".into(), json!(claude.cwd));
        obj.insert("entrypoint".into(), json!("claude-desktop"));
        line
    };
    write_lines(
        claude,
        &[
            json!({"type": "session-owner", "sessionId": SESSION}),
            desktop(json!({"type": "user", "uuid": "u-slash",
            "origin": {"kind": "human"}, "turnOrigin": "human",
            "message": {"role": "user", "content": format!(
                "<command-message>design</command-message>\n<command-name>/design</command-name>\n<command-args>{slash}</command-args>"
            )}})),
            desktop(json!({"type": "assistant", "uuid": "a-1",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "Working on it."}]}})),
            desktop(json!({"type": "attachment", "uuid": "u-queued",
                "attachment": {"type": "queued_command", "prompt": queued,
                    "commandMode": "prompt", "origin": {"kind": "human"}, "humanTurn": true}})),
            desktop(json!({"type": "assistant", "uuid": "a-2",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}})),
        ],
    );
}

#[test]
fn markers_in_a_slash_command_turn_and_a_queued_line_of_an_off_session_are_cataloged_at_stop() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let slash = "draft the plan; #scribe preference: dates are always absolute";
    let queued = "#scribe preference: keep status updates under five lines";
    typed_forms_transcript(&claude, slash, queued);
    let path = annotating_claude(&fx, &none_for(1..=1));
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(claude_calls(&fx).len(), 2, "{}", text(&out.stdout));
    let marks = marks_json(&fx);
    let settled: Vec<&Value> = marks.iter().map(|m| &m["message"]).collect();
    assert_eq!(
        settled,
        vec![&json!("u-slash"), &json!("u-queued")],
        "{marks:?}"
    );
    assert!(marks.iter().all(|m| m["outcome"] == "none"), "{marks:?}");
}

/// Run `scribe <args>` with the fake `claude` on PATH and a credential set.
fn mark_cmd(fx: &Fixture, args: &[&str], path: &std::ffi::OsString) -> Output {
    let mut cmd = fx.cmd(args);
    cmd.env("PATH", path)
        .env("CLAUDE_CODE_OAUTH_TOKEN", "oauth-test-value");
    cmd.output().unwrap()
}

#[test]
fn a_given_up_marker_stays_listed_and_mark_retry_catalogs_it() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    // As Claude Code writes them: each line carries the session's cwd.
    write_lines(
        &claude,
        &[
            json!({"type": "session-owner", "sessionId": SESSION}),
            json!({"type": "user", "uuid": "u-1", "sessionId": SESSION, "cwd": claude.cwd,
                   "timestamp": now_rfc3339(),
                   "message": {"role": "user", "content": MARKED}}),
            json!({"type": "assistant", "uuid": "a-u-1", "sessionId": SESSION, "cwd": claude.cwd,
                   "timestamp": now_rfc3339(),
                   "message": {"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}}),
        ],
    );
    let failing = fake_claude(&fx, "exit 1");
    for _ in 1..=3 {
        hook_path(&fx, "stop", &claude.payload("stop.json"), &failing);
    }
    assert_eq!(marks_json(&fx)[0]["code"], "model_failed");

    let listed = mark_cmd(&fx, &["mark", "list", "--json"], &failing);
    assert!(listed.status.success(), "{listed:?}");
    let doc: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(doc["total"], 1, "{doc}");
    assert_eq!(doc["marks"][0]["message"], "u-1");
    assert_eq!(doc["marks"][0]["outcome"], "refused");
    assert_eq!(doc["marks"][0]["code"], "model_failed");

    // Still failing: exits 1 with model_failed, and the marker waits pending.
    let again = mark_cmd(&fx, &["mark", "retry", SESSION], &failing);
    assert_eq!(again.status.code(), Some(1), "{again:?}");
    assert!(
        String::from_utf8_lossy(&again.stderr).contains(
            "1 #scribe marker(s) of 5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f still not cataloged"
        ),
        "{again:?}"
    );
    assert_eq!(marks_json(&fx)[0]["outcome"], "pending");
    assert_eq!(claude_calls(&fx).len(), 4);

    let working = annotating_claude(&fx, &none_for(1..=1));
    let retried = mark_cmd(&fx, &["mark", "retry", SESSION, "--json"], &working);
    assert!(retried.status.success(), "{retried:?}");
    let doc: Value = serde_json::from_slice(&retried.stdout).unwrap();
    assert_eq!(doc["marks"][0]["outcome"], "none", "{doc}");
    assert_eq!(marks_json(&fx)[0]["outcome"], "none");
    assert_eq!(claude_calls(&fx).len(), 5);
    assert!(
        audit(&fx)
            .iter()
            .any(|l| l["command"] == "mark retry" && l["error_code"].is_null()),
        "{:?}",
        audit(&fx)
    );

    let empty = mark_cmd(&fx, &["mark", "list", "--json"], &working);
    let doc: Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(doc["total"], 0, "{doc}");
}
