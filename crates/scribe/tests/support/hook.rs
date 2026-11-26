use crate::support;
use crate::support::command::Exit::{Capture, Success};
use crate::support::command::Input::Cli;
use crate::support::command::with_env;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, make_private, text};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Output;

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["hook"], expect)
    }
}

pub(super) const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hooks")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

/// A Claude Code layout under the fixture's home: the transcript path for
/// `SESSION`, and a cwd that is not a git repository.
pub(super) struct Claude {
    pub(super) transcript: PathBuf,
    pub(super) cwd: PathBuf,
}

impl Claude {
    pub(super) fn new(fx: &Fixture) -> Self {
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
    pub(super) fn payload(&self, name: &str) -> Value {
        let text = fixture(name)
            .replace("$TRANSCRIPT", self.transcript.to_str().unwrap())
            .replace("$CWD", self.cwd.to_str().unwrap());
        serde_json::from_str(&text).unwrap()
    }

    /// Write the transcript as Claude Code would after the prompt: the user
    /// message carrying `prompt`, then the reply.
    pub(super) fn write_transcript(&self, prompt: &str, uuid: &str) {
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

pub(super) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

/// Run `scribe hook <event>` with `payload` on stdin.
#[track_caller]
pub(super) fn hook(fx: &Fixture, event: &str, payload: &Value) -> Output {
    fx.check(&[event], Capture.stdin(payload.to_string().as_bytes()))
}

pub(super) fn run_with_stdin(fx: &Fixture, args: &[&str], input: &[u8]) -> Output {
    fx.check(Cli(args), Capture.stdin(input))
}

#[track_caller]
pub(super) fn hook_ok(fx: &Fixture, event: &str, payload: &Value) -> Output {
    let out = hook(fx, event, payload);
    assert_eq!(
        out.status.code(),
        Some(0),
        "hook {event} failed: {}",
        text(&out.stderr)
    );
    out
}

pub(super) fn reply(out: &Output) -> Value {
    let stdout = text(&out.stdout);
    assert_eq!(stdout.lines().count(), 1, "one line of hook JSON: {stdout}");
    serde_json::from_str(&stdout).unwrap()
}

pub(super) fn entries(fx: &Fixture) -> Vec<Value> {
    let out = fx.check(Cli(&["entry", "list", "--json"]), Success);
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

pub(super) fn audit(fx: &Fixture) -> Vec<Value> {
    support::audit_lines(&fx.root())
}

/// Every stored entry, in full (entries live in `scribe.db`).
pub(super) fn entries_snapshot(fx: &Fixture) -> Vec<Value> {
    entries(fx)
}

/// Replace the transcript with `lines`, one JSON object per line.
pub(super) fn write_lines(claude: &Claude, lines: &[Value]) {
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

/// The fixture prompt's two `#scribe` lines as a store from before
/// markers recorded them (2_design.md §14.6), their message ids waiting.
pub(super) fn write_fixture_entries(fx: &Fixture, prompt: &str) {
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

/// A fake `claude` in the fixture: records each call's argv and environment
/// under the `HOME` scribe gives it, then runs `body`.
pub(super) fn fake_claude(fx: &Fixture, body: &str) -> std::ffi::OsString {
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
pub(super) fn claude_calls(fx: &Fixture) -> Vec<Vec<String>> {
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

pub(super) fn prompt_payload(claude: &Claude, prompt: &str) -> Value {
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(prompt);
    payload
}

/// An entry recorded verbatim before markers took the line over
/// (2_design.md §14.6): `preferences-<n>`, saying `said`, citing `line` of
/// `prompt` in this session, its message id still waiting when `message`
/// is `None`.
pub(super) fn write_verbatim(
    fx: &Fixture,
    n: usize,
    said: &str,
    prompt: &str,
    line: u32,
    message: Option<&str>,
) {
    let content = said.split_once(": ").map_or(said, |(_, c)| c);
    let sha = scribe_core::capture::prompt_sha256(prompt);
    let message = message.map_or_else(|| String::from("null"), |m| format!("\"{m}\""));
    let text = format!(
        "---\nformat: 31\nkind: entry\nid: preferences-{n}\ntag: preferences\nstatus: active\n\
         project: scribe\ntopics: []\nrelated: []\nprovider: claude-code\nhost: host\n\
         author: {{kind: user, name: user}}\nupdated_by: null\n\
         created: 2026-01-02T03:04:05Z\nupdated: 2026-01-02T03:04:05Z\ninactive_since: null\n\
         said: \"{said}\"\n\
         content_from: {{kind: instruction}}\n\
         cite: {{session: {SESSION}, message: {message}, at: 2026-01-02T03:04:05Z, prompt_sha256: \"{sha}\", line: {line}, cwd: /work}}\n\
         retirement_reason: null\nsuperseded_by: null\nreview_after: null\nquote: null\n\
         window: null\nevidence: null\nas_of: null\napproved_by: null\ninstruction: null\n\
         removal_instruction: null\nstated_by: null\noutcome: null\n---\n{content}\n"
    );
    support::seed_records(&fx.root(), &[&text]);
}

/// An open stale proposal `p-1` on `preferences-1`.
pub(super) fn propose_stale(fx: &Fixture) {
    fx.check(
        Cli(&[
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
        ]),
        Success,
    );
}

pub(super) fn listed(fx: &Fixture, project: &str) -> Vec<Value> {
    let out = fx.check(
        Cli(&["entry", "list", "--project", project, "--json"]),
        Success,
    );
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

pub(super) const CURRENT: &str = "3d6a8c12-4e5f-4061-c8d9-0e1f2a3b4c5d";

pub(super) const PREVIOUS: &str = "1b4e6a90-2c3d-4e5f-a6b7-8c9d0e1f2a3b";

pub(super) const NEWER: &str = "2c5f7b01-3d4e-4f60-b7c8-9d0e1f2a3b4c";

/// A non-git directory whose project name is its basename, `scribe`.
pub(super) fn project_cwd(fx: &Fixture) -> PathBuf {
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

pub(super) fn start_payload(cwd: Option<&Path>, session: &str) -> Value {
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

pub(super) fn continuation_line(id: &str) -> String {
    format!(
        "scribe: this may continue {id} (recorded) — type /scribe on --from {id} to keep recording"
    )
}

/// One recorded session. `started` names the `sessions/<yyyy>/<mm>` directory.
pub(super) fn write_session(
    fx: &Fixture,
    id: &str,
    project: &str,
    started: &str,
    updated: &str,
    active: bool,
) {
    let active = if active { "true" } else { "false" };
    let body = format!(
        "---
format: 31
kind: session
id: {id}
status: open
active: {active}
keep_active: false
active_change: null
cataloged_by: null
rebuilt_by: null
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
    support::seed_records(&fx.root(), &[&body]);
}

pub(super) fn write_switch_on(fx: &Fixture, session: &str) {
    let body = format!(
        r#"{{
  "format": 31,
  "sessions": {{
    "{session}": {{"provider": "claude-code", "on": true, "since": "2026-11-16T12:00:00Z", "due": false, "final": false, "consecutive_failures": 0, "retry_after": null, "pending_links": [], "cataloged_by": null}}
  }}
}}
"#
    );
    support::seed_records(&fx.root(), &[&body]);
}

pub(super) fn write_open_proposal(fx: &Fixture) {
    support::seed_records(
        &fx.root(),
        &[r#"{
  "format": 31,
  "proposals": [
    {
      "id": "p-1",
      "kind": "stale",
      "older": "rituals-1",
      "candidate": null,
      "newer": null,
      "reason": "Past its review date.",
      "quotes": [{"entry": "rituals-1", "text": "run the full gate"}],
      "state": "open",
      "opened_at": "2026-11-16T12:00:00Z",
      "settled_at": null,
      "settled_by": null,
      "author": {"kind": "scribe", "name": "consistency"},
      "updated_by": null,
      "settled_author": null,
      "closed_reason": null,
      "instruction": null
    }
  ]
}
"#],
    );
}

pub(super) fn start_message(fx: &Fixture, payload: &Value) -> String {
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

/// Write `body` as the fixture store's `config.toml`.
pub(super) fn write_config(fx: &Fixture, body: &str) {
    std::fs::create_dir_all(fx.root()).unwrap();
    make_private(&fx.root());
    let config = fx.root().join("config.toml");
    std::fs::write(&config, body).unwrap();
    make_private(&config);
}

pub(super) const MARKED: &str = "I always want short status updates #scribe preferences";

pub(super) fn hook_path(
    fx: &Fixture,
    event: &str,
    payload: &Value,
    path: &std::ffi::OsString,
) -> Output {
    fx.check(
        fx.cmd(&["hook", event])
            .env("PATH", path)
            .env("CLAUDE_CODE_OAUTH_TOKEN", "oauth-test-value"),
        Success.stdin(payload.to_string().as_bytes()),
    )
}

/// The transcript: one typed turn per `(uuid, prompt)`, each with a reply.
pub(super) fn marked_turns(claude: &Claude, turns: &[(&str, &str)]) {
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

pub(super) fn marks_json(fx: &Fixture) -> Vec<Value> {
    match support::exported(&fx.root(), "marks.json") {
        Some(doc) => serde_json::from_str::<Value>(&doc).unwrap()["marks"]
            .as_array()
            .unwrap()
            .clone(),
        None => Vec::new(),
    }
}

/// Run `hook session-start` as Claude Code starts a session from
/// `entrypoint` (`cli` is interactive).
pub(super) fn start_from(fx: &Fixture, entrypoint: &str, payload: &Value) -> Value {
    let out = fx.check(
        with_env(
            &["hook", "session-start"],
            "CLAUDE_CODE_ENTRYPOINT",
            entrypoint,
        ),
        Success.stdin(payload.to_string().as_bytes()),
    );
    reply(&out)
}

/// A Claude desktop transcript of a session that was never switched on: a
/// slash-command turn whose args carry a marker, then a line Daniel typed
/// while the agent was busy (a `queued_command` attachment), both marked.
pub(super) fn typed_forms_transcript(claude: &Claude, slash: &str, queued: &str) {
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
