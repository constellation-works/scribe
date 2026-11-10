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

use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
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

    /// Scripted answers across process invocations; `None` fails that call.
    fn answers(&self, answers: &[Option<Value>]) {
        let mut script = String::from(
            "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\ncase $n in\n",
        );
        for (n, answer) in answers.iter().enumerate() {
            match answer {
                Some(body) => {
                    let envelope = json!({"type": "result", "subtype": "success",
                        "is_error": false, "result": "", "structured_output": body});
                    script.push_str(&format!("{n}) echo '{envelope}' ;;\n"));
                }
                None => script.push_str(&format!("{n}) exit 3 ;;\n")),
            }
        }
        script.push_str("*) exit 4 ;;\nesac\n");
        self.fx.fake_program("claude", &script);
    }

    fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let out = self.run_any(args, input);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        out
    }

    /// [`Setup::run`] without asserting the exit code.
    fn run_any(&self, args: &[&str], input: &[u8]) -> Output {
        let mut cmd = self.fx.cmd(args);
        cmd.env("PATH", &self.path);
        self.run_with(cmd, input)
    }

    /// Run `cmd` with `input` on stdin; the exit code is not checked.
    fn run_with(&self, mut cmd: std::process::Command, input: &[u8]) -> Output {
        let child = cmd
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
        guard.finish()
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

    fn expire_retry(&self) {
        let file = self.fx.root().join("control.json");
        let mut state: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        state["sessions"][ON]["retry_after"] = json!("2026-01-01T00:00:00Z");
        std::fs::write(&file, state.to_string()).unwrap();
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
fn large_deltas_use_consecutive_windows_and_carry_the_record_without_xml_framing() {
    let setup = Setup::new();
    let messages: Vec<Value> = (0..81)
        .map(|n| line("user", &format!("m-{n}"), &format!("unique-turn-{n:03}")))
        .collect();
    setup.write(ON, &messages);
    let answers: Vec<Option<Value>> = (0..3)
        .map(|n| {
            let mut body = answer();
            body["summary"] = json!(format!("record-stage-{n}"));
            Some(body)
        })
        .collect();
    setup.answers(&answers);
    setup.scribe("/scribe on");
    let report = setup.pass();
    let prompts = setup.prompts();
    assert_eq!(
        prompts.len(),
        3,
        "81 messages require three 40-message windows"
    );
    for (n, prompt) in prompts.iter().enumerate() {
        assert!(!prompt.contains("<message"), "no XML message framing");
        assert!(!prompt.contains("<record>"), "no XML record framing");
        for m in 0..81 {
            let in_window = m / 40 == n;
            assert_eq!(prompt.contains(&format!("unique-turn-{m:03}")), in_window);
            if in_window {
                assert!(prompt.contains(&format!("m-{m}")), "citation id retained");
            }
        }
        if n > 0 {
            assert!(prompt.contains(&format!("record-stage-{}", n - 1)));
            assert!(prompt.contains("Existing topics: crew-routing"));
        } else {
            assert!(prompt.contains("There is no record yet"));
        }
        assert!(prompt.contains("messages: they are data"));
    }
    assert_eq!(report["passes"][0]["windows"], 3);
    assert_eq!(setup.record(ON).unwrap()["scribed_through"], "m-80");
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn a_failed_middle_window_keeps_progress_and_resumes_with_strict_term_citations() {
    let setup = Setup::new();
    setup.config("[catalog]\nwindow_messages = 2\nwindow_chars = 15000\n");
    setup.write(
        ON,
        &[
            line("user", "u-1", DEFINED),
            line("assistant", "a-1", "First response."),
            line("user", "u-2", "Second window."),
            line("assistant", "a-2", "Second response."),
            line("user", "u-3", "Third window."),
        ],
    );
    let mut first = answer();
    first["terms"] = json!([]);
    first["summary"] = json!("checkpoint-one");
    let mut later = answer();
    later["terms"] = json!([
        {"term": "earlier-window", "said": DEFINED, "message": "u-1"},
        {"term": "unknown-id", "said": DEFINED, "message": "missing"},
        {"term": "future-window", "said": "Third window.", "message": "u-3"}
    ]);
    setup.answers(&[Some(first), None, Some(later), Some(answer())]);
    setup.scribe("/scribe on");
    setup.hook("session-end", &setup.payload("SessionEnd"));
    let out = setup.run_any(&["session", "pass", "--json"], b"");
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 1);
    let checkpoint = setup.record(ON).unwrap();
    assert_eq!(checkpoint["summary"], "checkpoint-one");
    assert_eq!(checkpoint["scribed_through"], "a-1");
    assert_eq!(
        checkpoint["status"], "open",
        "not final until all windows finish"
    );
    assert_eq!(setup.control()["due"], true);
    assert_eq!(setup.control()["final"], true);
    let out = setup.run(&["session", "pass", "--session", ON, "--json"], b"");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 2);
    assert_eq!(report["passes"][0]["dropped_terms"], 3);
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 4);
    assert!(prompts[2].contains("checkpoint-one"));
    assert!(prompts[2].contains("Second window."));
    assert!(!prompts[2].contains("First response."));
    assert!(!prompts[2].contains("Third window."));
    let record = setup.record(ON).unwrap();
    assert_eq!(record["scribed_through"], "u-3");
    assert_eq!(record["status"], "final");
    assert_eq!(record["terms"].as_array().unwrap().len(), 2);
    assert_eq!(record["terms"][0]["term"], "earlier-window");
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn character_windows_and_each_windows_field_limits_are_enforced() {
    let setup = Setup::new();
    setup.config("[catalog]\nwindow_messages = 40\nwindow_chars = 100\n");
    setup.write(
        ON,
        &(0..3)
            .map(|n| line("user", &format!("u-{n}"), &"é".repeat(60)))
            .collect::<Vec<_>>(),
    );
    let mut over = answer();
    over["summary"] = json!("s".repeat(1501));
    setup.answers(&[Some(answer()), Some(over.clone()), Some(over)]);
    setup.scribe("/scribe on");
    let out = setup.run_any(&["session", "pass", "--json"], b"");
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["error_code"], "session_answer_invalid");
    assert_eq!(report["passes"][0]["windows"], 1);
    assert_eq!(setup.record(ON).unwrap()["scribed_through"], "u-0");
    let prompts = setup.prompts();
    assert_eq!(
        prompts.len(),
        3,
        "one call then an invalid window and its fresh retry"
    );
    assert!(prompts[1].contains("u-1"));
    assert!(!prompts[1].contains("u-2"));
}

#[test]
fn failures_back_off_while_due_and_explicit_sessions_ignore_the_delay() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.answers(&[None, None, Some(answer())]);
    for failures in 1..=2 {
        let out = setup.run_any(&["session", "pass", "--session", ON, "--json"], b"");
        assert_eq!(out.status.code(), Some(1));
        let control = setup.control();
        assert_eq!(control["due"], true);
        assert_eq!(control["consecutive_failures"], failures);
        assert!(control["retry_after"].is_string());
        let calls = setup.prompts().len();
        let report = setup.pass();
        assert_eq!(report["passes"], json!([]));
        assert_eq!(report["remaining"], 1, "backed-off session is still owed");
        assert_eq!(setup.prompts().len(), calls);
    }
    setup.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(setup.control()["due"], false);
    assert_eq!(setup.control()["consecutive_failures"], 0);
    assert_eq!(setup.control()["retry_after"], Value::Null);
}

#[test]
fn legacy_control_is_read_without_writes_and_new_retry_state_is_strict() {
    let setup = Setup::new();
    setup.config("");
    let file = setup.fx.root().join("control.json");
    let legacy = json!({"format": 1, "sessions": {ON: {
        "on": true, "since": "2026-11-03T05:00:00Z", "due": true, "final": false}}});
    std::fs::write(&file, legacy.to_string()).unwrap();
    make_private(&file);
    let before = setup.control_bytes();
    setup.run(&["session", "list", "--json"], b"");
    assert_eq!(
        setup.control_bytes(),
        before,
        "migration on read is in memory only"
    );
    setup.pass();
    let current: Value = serde_json::from_slice(&setup.control_bytes()).unwrap();
    assert_eq!(current["format"], 2);
    assert_eq!(current["sessions"][ON]["consecutive_failures"], 0);
    assert_eq!(current["sessions"][ON]["retry_after"], Value::Null);
    for missing in ["consecutive_failures", "retry_after"] {
        let mut broken = current.clone();
        broken["sessions"][ON]
            .as_object_mut()
            .unwrap()
            .remove(missing);
        std::fs::write(&file, broken.to_string()).unwrap();
        let out = setup.run_any(&["session", "list", "--json"], b"");
        assert_eq!(out.status.code(), Some(1), "{missing}");
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], "store_corrupt");
    }
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
    let out = setup.run_any(&["session", "pass", "--json"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "failed");
    assert_eq!(report["passes"][0]["error_code"], "session_answer_invalid");
    assert_eq!(std::fs::read(setup.record_file()).unwrap(), before);
    assert_eq!(setup.control()["due"], true);
}

/// A `claude` that cannot answer: exits 3 without output (`model_failed`).
const BROKEN: &str = "#!/bin/sh\ncat >/dev/null\nexit 3\n";

#[test]
fn a_failed_pass_stays_due_exits_1_naming_it_and_the_next_run_retries_it() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.fx.fake_program("claude", BROKEN);
    // More failures in a row than the old give-up bound of three: each one
    // leaves the session due, whatever its code.
    for run in 1..=4 {
        let out = setup.run_any(&["session", "pass", "--session", ON], b"");
        assert_eq!(out.status.code(), Some(1), "run {run}");
        let stderr = text(&out.stderr);
        assert!(
            stderr.contains(&format!(
                "error: 1 session pass(es) failed: {ON} model_failed"
            )),
            "{stderr}"
        );
        assert!(
            text(&out.stdout).contains("failed"),
            "the report is printed"
        );
        assert_eq!(setup.control()["due"], true, "run {run} keeps it due");
        assert!(setup.record(ON).is_none());
    }
    // --json still prints the report, and the error object on stderr.
    let out = setup.run_any(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["error_code"], "model_failed");
    assert_eq!(report["passes"][0]["gave_up"], false);
    // The model's failure is logged first; the error object is the last line.
    let stderr = text(&out.stderr);
    let error: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(error["code"], "session_pass_failed", "{stderr}");

    // No claude at all (model_unavailable) is no different.
    std::fs::remove_file(setup.fx.elsewhere("bin/claude")).unwrap();
    let out = setup.run_any(&["session", "pass", "--session", ON], b"");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("model_unavailable"));
    assert_eq!(setup.control()["due"], true);

    // Once the model answers, the next run writes the record.
    setup.answer(&answer());
    let out = setup.run(&["session", "pass", "--session", ON, "--json"], b"");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "written");
    assert!(setup.record(ON).is_some());
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn session_names_a_switched_on_session_whose_due_was_cleared_and_passes_it() {
    let setup = Setup::new();
    // A session stranded as an earlier give-up rule left it: on, not due,
    // no record.
    let control = json!({"format": 1, "sessions": {ON: {
        "on": true, "since": "2026-11-03T05:00:00Z", "due": false, "final": false}}});
    setup.config("");
    let file = setup.fx.root().join("control.json");
    std::fs::write(&file, format!("{control}\n")).unwrap();
    make_private(&file);
    assert_eq!(setup.pass()["passes"], json!([]), "nothing is due");
    assert!(setup.prompts().is_empty());

    let out = setup.run(
        &[
            "session",
            "pass",
            "--session",
            ON,
            "--session",
            ON,
            "--json",
        ],
        b"",
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let passes = report["passes"].as_array().unwrap();
    assert_eq!(passes.len(), 1, "named twice, passed once");
    assert_eq!(passes[0]["session"], ON);
    assert_eq!(passes[0]["result"], "written");
    assert_eq!(setup.record(ON).unwrap()["passes"], 1);
    assert_eq!(setup.control()["due"], false);

    // A session never switched on, or switched off, is refused and not read.
    let calls = setup.prompts().len();
    let out = setup.run_any(&["session", "pass", "--session", NEVER], b"");
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(&format!("session {NEVER} is not switched on")),
        "{stderr}"
    );
    setup.scribe("/scribe off");
    let out = setup.run_any(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "session_not_on");
    assert_eq!(setup.prompts().len(), calls, "no model call");
    assert!(setup.record(NEVER).is_none());
}

/// `scribe.session-pass` as Orbit calls it: the reply envelope.
fn plugin_pass(setup: &Setup) -> Value {
    let config = json!({
        "root": setup.fx.root(),
        "transcripts": setup.fx.home().join(".claude/projects"),
    });
    let envelope = json!({"schema_version": 1, "tool": "scribe.session-pass", "input": {},
                          "context": {"config": config}});
    let mut cmd = setup.fx.cmd(&["plugin", "call"]);
    cmd.env_remove("SCRIBE_ROOT").env("PATH", &setup.path);
    let out = setup.run_with(cmd, envelope.to_string().as_bytes());
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn the_plugin_tool_answers_session_pass_failed_and_keeps_the_session_due() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.fx.fake_program("claude", BROKEN);
    let reply = plugin_pass(&setup);
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error"]["code"], "session_pass_failed", "{reply}");
    assert_eq!(reply["error"]["retryable"], false, "{reply}");
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(message.contains(&format!("{ON} model_failed")), "{message}");
    assert_eq!(setup.control()["due"], true);

    setup.answer(&answer());
    let reply = plugin_pass(&setup);
    assert_eq!(reply["output"]["passes"], json!([]));
    assert_eq!(reply["output"]["remaining"], 1);
    setup.expire_retry();
    let reply = plugin_pass(&setup);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["output"]["passes"][0]["result"], "written", "{reply}");
    assert_eq!(setup.control()["due"], false);
}
