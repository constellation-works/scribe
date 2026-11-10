//! End-to-end tests of S4 through the built binary
//! (docs/design/scribe/2_design.md §5, §10 S4): `/scribe on|off|status`
//! switching a session and blocking the prompt, the `Stop` and `SessionEnd`
//! hooks marking a pass due without a model, and `scribe session pass`
//! writing one record per due session through a fake `claude`.
//!
//! Every run is the built binary with a temp `HOME` whose
//! `.claude/projects/` holds the test transcripts; nothing touches the real
//! home.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use scribe_core::pass;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use support::{Fixture, make_private, text};

const ON: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
const NEVER: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e60";
const DEFINED: &str = "by graduate to level 4 I mean a crew that merges without review";
const NEVER_SAID: &str = "a session never switched on, which scribe must never read";

/// A Claude Code layout under the fixture's home, a fake `claude` on PATH
/// and helpers to run the binary with them.
struct Setup {
    fx: Fixture,
    project: PathBuf,
    cwd: PathBuf,
    path: OsString,
}

fn line(kind: &str, uuid: &str, text: &str) -> Value {
    json!({"type": kind, "uuid": uuid, "timestamp": "2026-11-03T06:00:00Z",
           "cwd": "/nonexistent/work/scribe",
           "message": {"role": kind, "content": text}})
}

impl Setup {
    fn new() -> Self {
        let fx = Fixture::new();
        let project = fx.home().join(".claude/projects/-work-scribe");
        std::fs::create_dir_all(&project).unwrap();
        let cwd = fx.elsewhere("work/scribe");
        std::fs::create_dir_all(&cwd).unwrap();
        let bin = fx.elsewhere("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let mut path = bin.into_os_string();
        path.push(":/usr/bin:/bin");
        let setup = Self {
            fx,
            project,
            cwd,
            path,
        };
        setup.answer(&answer());
        setup.write(ON, &[line("user", "u-1", &format!("Crews. {DEFINED}."))]);
        setup.append(
            ON,
            &[line(
                "assistant",
                "a-1",
                "Level 4 crews merge their own pull requests after green CI.",
            )],
        );
        setup.write(NEVER, &[line("user", "n-1", NEVER_SAID)]);
        setup
    }

    fn transcript(&self, session: &str) -> PathBuf {
        self.project.join(format!("{session}.jsonl"))
    }

    fn write(&self, session: &str, lines: &[Value]) {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(self.transcript(session), text).unwrap();
    }

    fn append(&self, session: &str, lines: &[Value]) {
        let mut text = std::fs::read_to_string(self.transcript(session)).unwrap_or_default();
        text.extend(lines.iter().map(|l| format!("{l}\n")));
        std::fs::write(self.transcript(session), text).unwrap();
    }

    /// The fake `claude`: saves its stdin (the prompt) and answers `body`
    /// as `claude -p --output-format json` does.
    fn answer(&self, body: &Value) {
        let envelope = json!({"type": "result", "subtype": "success", "is_error": false,
                              "result": "", "structured_output": body});
        let script = format!(
            "#!/bin/sh\n\
             n=$(ls \"$HOME\" | grep -c '^claude-stdin')\n\
             cat > \"$HOME/claude-stdin-$n\"\n\
             echo '{envelope}'\n"
        );
        let file = self.fx.elsewhere("bin/claude");
        std::fs::write(&file, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    /// Every prompt the fake `claude` received, oldest first.
    fn prompts(&self) -> Vec<String> {
        (0..)
            .map(|n| self.fx.home().join(format!("claude-stdin-{n}")))
            .take_while(|p| p.exists())
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect()
    }

    fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let child = self
            .fx
            .cmd(args)
            .env("PATH", &self.path)
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
            .write_all(input)
            .unwrap();
        let out = guard.finish();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        out
    }

    fn hook(&self, event: &str, payload: &Value) -> Output {
        self.run(&["hook", event], payload.to_string().as_bytes())
    }

    fn payload(&self, event: &str) -> Value {
        json!({"session_id": ON,
               "transcript_path": self.transcript(ON),
               "cwd": self.cwd,
               "permission_mode": "default",
               "hook_event_name": event})
    }

    /// `/scribe …` typed in the `ON` session: the hook's block reason.
    fn scribe(&self, prompt: &str) -> String {
        let mut payload = self.payload("UserPromptSubmit");
        payload["prompt"] = json!(prompt);
        let out = self.hook("user-prompt-submit", &payload);
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(reply["decision"], "block", "{reply}");
        reply["reason"].as_str().unwrap().to_owned()
    }

    fn pass(&self) -> Value {
        let out = self.run(&["session", "pass", "--json"], b"");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn control(&self) -> Value {
        let text = std::fs::read_to_string(self.fx.root().join("control.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["sessions"][ON].clone()
    }

    fn control_bytes(&self) -> Vec<u8> {
        std::fs::read(self.fx.root().join("control.json")).unwrap()
    }

    fn record(&self, session: &str) -> Option<Value> {
        let out = self.fx.run(&["session", "show", session, "--json"]);
        (out.status.code() == Some(0)).then(|| serde_json::from_slice(&out.stdout).unwrap())
    }

    fn record_file(&self) -> PathBuf {
        self.fx.root().join(format!("sessions/2026/11/{ON}.md"))
    }

    fn audit_lines(&self) -> usize {
        std::fs::read_to_string(self.fx.root().join("audit.jsonl")).map_or(0, |t| t.lines().count())
    }

    fn config(&self, toml: &str) {
        let root = self.fx.root();
        if !root.exists() {
            std::fs::create_dir(&root).unwrap();
            make_private(&root);
        }
        let file = root.join("config.toml");
        std::fs::write(&file, toml).unwrap();
        make_private(&file);
    }
}

fn answer() -> Value {
    json!({
        "title": "Defining crew levels",
        "summary": "Daniel defined what level 4 means for crews.",
        "goals": "Name the crew levels.",
        "outcome": "Level 4 is defined.",
        "next": ["write it into the routing doc"],
        "topics": ["crew-routing"],
        "related": [],
        "terms": [
            {"term": "graduate to level 4", "said": DEFINED, "message": "u-1"},
            {"term": "level 4 crew", "said": "Level 4 crews merge their own pull requests",
             "message": "a-1"}
        ]
    })
}

#[test]
fn scribe_on_off_and_status_switch_the_session_and_block_the_prompt() {
    let setup = Setup::new();
    let reason = setup.scribe("/scribe status");
    assert!(reason.contains("never switched on"), "{reason}");
    assert!(!setup.fx.root().join("control.json").exists());

    let reason = setup.scribe("/scribe on");
    assert!(reason.starts_with("scribe: on"), "{reason}");
    let switch = setup.control();
    assert_eq!(switch["on"], true);
    assert_eq!(switch["due"], true);
    assert_eq!(setup.audit_lines(), 1);

    // On twice: a no-op that says so.
    let before = setup.control_bytes();
    let reason = setup.scribe("/scribe ON");
    assert!(reason.starts_with("scribe: already on since "), "{reason}");
    assert!(reason.ends_with("nothing changed"), "{reason}");
    assert_eq!(setup.control_bytes(), before);
    assert_eq!(setup.audit_lines(), 1);

    let reason = setup.scribe("/scribe");
    assert!(reason.starts_with("scribe: on since "), "{reason}");
    assert!(reason.contains("a pass is due; no record yet"), "{reason}");

    let reason = setup.scribe("/scribe off");
    assert!(reason.starts_with("scribe: off"), "{reason}");
    assert_eq!(setup.control()["on"], false);
    assert_eq!(setup.control()["due"], false);
    assert_eq!(setup.audit_lines(), 2);

    let reason = setup.scribe("/scribe maybe");
    assert!(reason.contains("unknown command"), "{reason}");
    assert_eq!(setup.control()["on"], false);

    // No pass runs for a session switched off.
    assert_eq!(setup.pass()["passes"], json!([]));
    assert!(setup.prompts().is_empty());
}

#[test]
fn stop_marks_due_by_turns_and_minutes_and_session_end_marks_final_without_a_model() {
    let setup = Setup::new();
    setup.config("[catalog]\nevery_turns = 2\nevery_minutes = 600\n");
    setup.scribe("/scribe on");
    assert_eq!(setup.pass()["passes"][0]["result"], "written");
    assert_eq!(setup.control()["due"], false);
    let calls = setup.prompts().len();

    setup.append(ON, &[line("user", "u-2", "One more turn.")]);
    setup.hook("stop", &setup.payload("Stop"));
    assert_eq!(setup.control()["due"], false, "one turn of two");
    setup.append(ON, &[line("user", "u-3", "And another.")]);
    let out = setup.hook("stop", &setup.payload("Stop"));
    assert!(out.stdout.is_empty());
    assert_eq!(setup.control()["due"], true, "two turns since the pass");

    setup.hook("session-end", &setup.payload("SessionEnd"));
    assert_eq!(setup.control()["final"], true);
    assert_eq!(setup.prompts().len(), calls, "the hooks call no model");

    // The final pass writes a final record and settles the switch.
    let report = setup.pass();
    assert_eq!(report["passes"][0]["result"], "written");
    assert_eq!(setup.record(ON).unwrap()["status"], "final");
    assert_eq!(setup.control()["due"], false);
    assert_eq!(setup.control()["final"], false);

    // By minutes: on since long ago, never passed, no new turns needed.
    let minutes = Setup::new();
    minutes.config("[catalog]\nevery_turns = 50\nevery_minutes = 5\n");
    let control = json!({"format": 1, "sessions": {ON: {
        "on": true, "since": "2026-01-01T00:00:00Z", "due": false, "final": false}}});
    let file = minutes.fx.root().join("control.json");
    std::fs::write(&file, format!("{control}\n")).unwrap();
    make_private(&file);
    minutes.hook("stop", &minutes.payload("Stop"));
    assert_eq!(minutes.control()["due"], true);
    assert!(minutes.prompts().is_empty());
}

#[test]
fn a_pass_refuses_when_a_configured_redact_name_is_unset() {
    // STD-05 R13: [redact] env names a value scribe must mask, but the
    // routine's environment (the plugin's env_pass, here the cleared test
    // environment) does not carry it. The pass refuses before any model
    // call or write, rather than storing text unmasked without warning.
    let setup = Setup::new();
    setup.config("[redact]\nenv = [\"SCRIBE_UNSET_REDACT_TOKEN\"]\n");
    setup.scribe("/scribe on");
    let out = setup
        .fx
        .cmd(&["session", "pass", "--json"])
        .env("PATH", &setup.path)
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0), "{}", text(&out.stdout));
    assert!(
        text(&out.stderr).contains("SCRIBE_UNSET_REDACT_TOKEN"),
        "{}",
        text(&out.stderr)
    );
    assert!(
        text(&out.stderr).contains("redact_env_unset")
            || text(&out.stderr).contains("[redact] env"),
        "{}",
        text(&out.stderr)
    );
    assert!(setup.record(ON).is_none(), "nothing was written");
    assert_eq!(setup.control()["due"], true, "the pass stays due");
    assert!(setup.prompts().is_empty(), "no model call before refusal");

    // With the name supplied, the same pass writes as usual.
    let out = setup
        .fx
        .cmd(&["session", "pass", "--json"])
        .env("PATH", &setup.path)
        .env("SCRIBE_UNSET_REDACT_TOKEN", "a-value-that-must-be-masked")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "written");
    assert!(setup.record(ON).is_some());
}

#[test]
fn a_pass_writes_one_record_of_daniels_terms_and_never_reads_a_session_never_switched_on() {
    let setup = Setup::new();
    let never_before = std::fs::read(setup.transcript(NEVER)).unwrap();
    // A Stop and a SessionEnd for the never-on session touch nothing.
    let mut never = setup.payload("Stop");
    never["session_id"] = json!(NEVER);
    never["transcript_path"] = json!(setup.transcript(NEVER));
    setup.hook("stop", &never);
    never["hook_event_name"] = json!("SessionEnd");
    setup.hook("session-end", &never);
    assert!(!setup.fx.root().join("control.json").exists());

    setup.scribe("/scribe on");
    let report = setup.pass();
    let passes = report["passes"].as_array().unwrap();
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0]["session"], ON);
    assert_eq!(passes[0]["result"], "written");
    assert_eq!(passes[0]["dropped_terms"], 1, "the assistant's term");

    let record = setup.record(ON).unwrap();
    assert_eq!(record["scribed_through"], "a-1");
    assert_eq!(record["passes"], 1);
    assert_eq!(
        record["terms"],
        json!([{"term": "graduate to level 4", "said": DEFINED, "message": "u-1"}])
    );
    assert!(!setup.fx.root().join("entries").exists(), "no entries");

    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains(DEFINED));
    assert!(!prompts[0].contains(NEVER_SAID));
    assert!(setup.record(NEVER).is_none());
    assert_eq!(
        std::fs::read(setup.transcript(NEVER)).unwrap(),
        never_before
    );

    // An invalid answer keeps the previous record and leaves the pass due.
    let before = std::fs::read(setup.record_file()).unwrap();
    setup.append(ON, &[line("user", "u-2", "More on levels.")]);
    setup.scribe("/scribe off");
    setup.scribe("/scribe on");
    let mut over = answer();
    over["summary"] = json!("s".repeat(1501));
    setup.answer(&over);
    let report = setup.pass();
    assert_eq!(report["passes"][0]["result"], "failed");
    assert_eq!(report["passes"][0]["error_code"], "session_answer_invalid");
    assert_eq!(std::fs::read(setup.record_file()).unwrap(), before);
    assert_eq!(setup.control()["due"], true);
}

/// The routine's call as Orbit makes it: `scribe plugin call` for
/// `scribe.session-pass`, from the cleared environment the Orbit clock gives
/// (launchd's PATH, which holds no `claude`), with `claude_program` set when
/// given. Returns the reply envelope.
fn routine_pass(setup: &Setup, claude_program: Option<&Path>) -> Value {
    let mut config = json!({
        "root": setup.fx.root(),
        "transcripts": setup.fx.home().join(".claude/projects"),
    });
    if let Some(program) = claude_program {
        config["claude_program"] = json!(program);
    }
    let envelope = json!({"schema_version": 1, "tool": "scribe.session-pass", "input": {},
                          "context": {"config": config}});
    let mut cmd = setup.fx.cmd(&["plugin", "call"]);
    cmd.env_remove("SCRIBE_ROOT")
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = support::ChildGuard::new(cmd.spawn().unwrap());
    child
        .child()
        .stdin
        .take()
        .unwrap()
        .write_all(envelope.to_string().as_bytes())
        .unwrap();
    let out = child.finish();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn the_routine_writes_a_due_session_through_an_absolute_claude_program() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    let program = setup.fx.elsewhere("bin/claude");
    let reply = routine_pass(&setup, Some(&program));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["output"]["passes"][0]["result"], "written", "{reply}");
    assert!(setup.record(ON).is_some());
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn a_session_whose_program_cannot_start_stays_due_and_a_later_run_retries_it() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    let missing = setup.fx.elsewhere("bin/no-such-claude");
    let reply = routine_pass(&setup, Some(&missing));
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error"]["code"], "session_pass_failed", "{reply}");
    assert_eq!(reply["error"]["retryable"], true, "{reply}");
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(message.contains("model_unavailable"), "{message}");
    assert!(message.contains(missing.to_str().unwrap()), "{message}");
    assert!(setup.record(ON).is_none(), "nothing was written");
    assert_eq!(setup.control()["due"], true, "the pass stays due");

    let program = setup.fx.elsewhere("bin/claude");
    let reply = routine_pass(&setup, Some(&program));
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(setup.record(ON).is_some(), "the later run wrote the record");
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn a_permanent_failure_clears_due_at_the_third_run() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup
        .fx
        .fake_program("claude-broken", "#!/bin/sh\ncat >/dev/null\nexit 3\n");
    let broken = setup.fx.elsewhere("bin/claude-broken");
    for run in 1..pass::MAX_FAILURES {
        let reply = routine_pass(&setup, Some(&broken));
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(
            setup.control()["due"],
            true,
            "failure {run} of a permanent error"
        );
    }
    routine_pass(&setup, Some(&broken));
    assert_eq!(
        setup.control()["due"],
        false,
        "given up at the third failure"
    );
}

#[test]
fn an_outage_keeps_due_until_the_retry_bound() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    let missing = setup.fx.elsewhere("bin/no-such-claude");
    for run in 1..pass::MAX_RETRYABLE_FAILURES {
        routine_pass(&setup, Some(&missing));
        assert_eq!(setup.control()["due"], true, "failure {run} of an outage");
    }
    routine_pass(&setup, Some(&missing));
    assert_eq!(setup.control()["due"], false, "given up at the retry bound");
}

#[test]
fn a_relative_claude_program_is_not_configured() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    let reply = routine_pass(&setup, Some(Path::new("claude")));
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error"]["code"], "not_configured", "{reply}");
    assert_eq!(setup.control()["due"], true);
}
