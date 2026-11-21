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

#[test]
fn pass_candidate_waits_for_typed_approval_and_keeps_original_user_cite() {
    let s = Setup::new();
    let quote = "I prefer short progress updates while an agent is working.";
    s.write(ON, &[line("user", "preference-turn", quote)]);
    let mut body = answer();
    body["candidates"] =
        json!([{"quote": quote, "message": "preference-turn", "tag": "preferences"}]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let proposals: Value =
        serde_json::from_slice(&s.fx.ok(&["proposal", "list", "--json"]).stdout).unwrap();
    let p = &proposals["proposals"][0];
    assert_eq!(p["kind"], "candidate");
    assert_eq!(p["candidate"]["quote"], quote);
    assert_eq!(p["candidate"]["cite"]["message"], "preference-turn");
    assert!(
        !s.fx
            .root()
            .join("entries/preferences/preferences-1.md")
            .exists()
    );
    let id = p["id"].as_str().unwrap();
    let prompt = format!("#scribe approve {id}");
    let mut payload = s.payload("UserPromptSubmit");
    payload["prompt"] = json!(prompt);
    let first = s.hook("user-prompt-submit", &payload);
    assert!(
        text(&first.stdout).contains("recorded"),
        "{}",
        text(&first.stdout)
    );
    s.hook("user-prompt-submit", &payload);
    let entries: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "list", "--tag", "preferences", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entries["entries"].as_array().unwrap().len(), 1);
    let entry: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entry["content"], quote);
    assert_eq!(entry["author"]["kind"], "daniel");
    assert_eq!(entry["cite"]["message"], "preference-turn");
    assert_eq!(entry["content_from"]["role"], "user");

    // What admitted the claim is printed by `proposal list` and `entry show`
    // in every form.
    let criteria = "A standing rule from Daniel that applies beyond the work at hand.";
    let admitted = json!({"trigger": "I prefer", "matched": "I prefer", "criteria": criteria});
    assert_eq!(entry["admitted"], admitted);
    assert_eq!(p["candidate"]["admitted"], admitted);
    let plain = text(&s.fx.ok(&["entry", "show", "preferences-1"]).stdout);
    assert!(
        plain
            .lines()
            .any(|l| l == format!("admitted\tI prefer\tI prefer\t{criteria}")),
        "{plain}"
    );
    let table = text(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--format", "table"])
            .stdout,
    );
    assert!(
        table.contains(&format!(
            "admitted    trigger \"I prefer\" matched \"I prefer\"; criteria: {criteria}"
        )),
        "{table}"
    );
    let listed = text(&s.fx.ok(&["proposal", "list"]).stdout);
    assert!(
        listed.ends_with(&format!(
            "\ttrigger \"I prefer\" matched \"I prefer\"; criteria: {criteria}\t-\n"
        )),
        "{listed}"
    );
    // An entry not approved from a claim has no admission.
    let typed: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "Keep it short.",
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    let id = typed["id"].as_str().unwrap();
    let shown: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", id, "--json"]).stdout).unwrap();
    assert_eq!(shown["admitted"], Value::Null);
}

#[test]
fn a_stored_assistant_finding_candidate_still_approves_into_one_agent_entry() {
    // The pass no longer opens assistant-turn candidates (capture rules read
    // Daniel's turns only), but one opened before, in a format-10 store,
    // still settles: approval writes the agent's entry, never Daniel's.
    let s = Setup::new();
    let quote = "Measured: the reviewer mix alone predicts a 7-point first-pass gap.";
    s.write(
        ON,
        &[
            line("user", "u-1", "Please measure the reviewer mix effect."),
            line("assistant", "a-find", quote),
        ],
    );
    s.answer(&answer());
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let cite = json!({"session": ON, "message": "a-find", "at": "2026-11-03T06:00:00Z",
        "prompt_sha256": "0".repeat(64), "line": 1, "cwd": "/nonexistent/work/scribe"});
    let proposal = json!({
        "id": "p-1", "kind": "candidate", "older": "finding-1", "newer": null,
        "candidate": {"tag": "finding", "role": "assistant", "quote": quote,
            "evidence": "exp-5342a4d6", "cite": cite, "project": "scribe",
            "provider": "claude-code", "host": "host"},
        "reason": "Assistant finding statement", "quotes": [], "state": "open",
        "opened_at": "2026-11-03T06:01:00Z", "settled_at": null, "settled_by": null,
        "author": {"kind": "scribe", "name": "session-pass"}, "updated_by": null,
        "settled_author": null
    });
    let path = s.fx.root().join("proposals.json");
    std::fs::write(
        &path,
        json!({"format": 10, "proposals": [proposal]}).to_string(),
    )
    .unwrap();
    make_private(&path);
    let ps = proposals(&s);
    assert_eq!(ps["proposals"][0]["candidate"]["admitted"], Value::Null);
    assert_eq!(ps["proposals"][0]["closed_reason"], Value::Null);

    let approved = type_settlement(&s, "#scribe approve p-1");
    assert!(
        text(&approved.stdout).contains("recorded finding-1 as the agent's entry"),
        "{}",
        text(&approved.stdout)
    );
    type_settlement(&s, "#scribe approve p-1");
    let entries: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "list", "--tag", "finding", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entries["entries"].as_array().unwrap().len(), 1);
    let entry = &entries["entries"][0];
    assert_eq!(entry["content"], quote);
    assert_eq!(entry["author"]["kind"], "agent", "never Daniel's");
    assert_eq!(entry["author"]["name"], "assistant");
    assert_eq!(entry["content_from"]["role"], "assistant");
    assert_eq!(entry["evidence"], "exp-5342a4d6");
    assert_eq!(entry["as_of"], "2026-11-03");
}

#[test]
fn pass_opens_no_candidate_from_an_assistant_turn() {
    // Daniel's turn fires a trigger, so claims are requested, but a claim
    // on an assistant turn (a measured result or a future consideration) is
    // refused: no tag's capture rules read assistant turns, while Daniel's
    // turns are still read.
    for (tag, quote) in [
        (
            "finding",
            "Measured: the reviewer mix alone predicts a 7-point gap.",
        ),
        (
            "remind",
            "The flaky cache test is not verified yet on this host.",
        ),
        (
            "idea",
            "What if we cache the reviewer mix per crew instead?",
        ),
    ] {
        let s = Setup::new();
        s.write(
            ON,
            &[
                line("user", "u-1", "Always measure before you change the mix."),
                line("assistant", "a-1", quote),
            ],
        );
        s.answer(&candidate_answer(quote, "a-1", tag));
        s.scribe("/scribe on");
        s.run(&["session", "pass", "--session", ON, "--json"], b"");
        assert_eq!(proposals(&s)["total"], 0, "{tag}");
        assert_eq!(s.record(ON).unwrap()["scribed_through"], "a-1", "{tag}");
    }

    // Daniel's own firing turn in the same conversation still opens a claim.
    let s = Setup::new();
    let daniel_quote = "Always measure before you change the mix.";
    s.write(
        ON,
        &[
            line("user", "u-1", daniel_quote),
            line(
                "assistant",
                "a-1",
                "Measured: the reviewer mix alone predicts a 7-point gap.",
            ),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": "Measured: the reviewer mix alone predicts a 7-point gap.", "message": "a-1", "tag": "finding"},
        {"quote": daniel_quote, "message": "u-1", "tag": "preferences"}
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["proposals"][0]["candidate"]["tag"], "preferences");
    assert_eq!(listed["proposals"][0]["candidate"]["quote"], daniel_quote);
}

#[test]
fn pass_asks_for_standing_preferences_only_and_never_proposes_a_decision() {
    let s = Setup::new();
    let choice = "For this migration we will go with the blue-green rollout.";
    let standing = "I always want short progress updates while an agent is working.";
    s.write(
        ON,
        &[
            line("user", "u-choice", choice),
            line("user", "u-standing", standing),
        ],
    );
    let mut body = answer();
    // The model offers both; the one-time choice under the retired tag is
    // dropped locally, the standing preference stays.
    body["candidates"] = json!([
        {"quote": choice, "message": "u-choice", "tag": "decisions"},
        {"quote": standing, "message": "u-standing", "tag": "preferences"}
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["proposals"][0]["candidate"]["tag"], "preferences");
    assert_eq!(listed["proposals"][0]["candidate"]["quote"], standing);

    let prompts = s.prompts();
    assert!(!prompts.is_empty());
    for prompt in &prompts {
        assert!(
            !prompt.contains("tag decisions"),
            "the pass prompt never offers decisions: {prompt}"
        );
        assert!(!prompt.contains("message u-choice"), "{prompt}");
        assert!(
            prompt.contains("A standing rule from Daniel that applies beyond the work at hand."),
            "{prompt}"
        );
    }
    // The admission is recorded with the candidate.
    let admitted = &listed["proposals"][0]["candidate"]["admitted"];
    assert_eq!(admitted["trigger"], "always", "{admitted}");
    assert_eq!(admitted["matched"], "always");
    assert_eq!(
        admitted["criteria"],
        "A standing rule from Daniel that applies beyond the work at hand."
    );
}

#[test]
fn a_window_where_no_daniel_turn_fires_a_trigger_requests_no_claims() {
    let s = Setup::new();
    let quote = "Let us use the blue-green rollout for this migration.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            // An assistant turn's trigger words never count.
            line("assistant", "a-1", "I always prefer to check first."),
        ],
    );
    // A model that ignores the request still opens nothing.
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(proposals(&s)["total"], 0);
    let prompts = s.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("Return candidates: [] (none are requested in this window)."),
        "{}",
        prompts[0]
    );
    assert!(!prompts[0].contains("Already recorded or proposed"));
}

#[test]
fn a_firing_turn_is_named_with_its_tag_and_trigger_and_idea_still_proposes() {
    let s = Setup::new();
    let idea = "What if we cached the routing table per host?";
    s.write(
        ON,
        &[
            line("user", "u-plain", "Please look at the routing table."),
            line("user", "u-idea", idea),
        ],
    );
    s.answer(&candidate_answer(idea, "u-idea", "idea"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains(
            "- message u-idea: tag idea (trigger \"what if we\", matched \"What if we\")"
        ),
        "{prompt}"
    );
    assert!(!prompt.contains("message u-plain"), "{prompt}");
    assert!(
        prompt.contains("- idea: qualifies: Daniel puts forward something to consider"),
        "{prompt}"
    );
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    let p = &listed["proposals"][0];
    assert_eq!(p["kind"], "candidate", "the propose route");
    assert_eq!(p["state"], "open");
    assert_eq!(p["candidate"]["tag"], "idea");
    assert_eq!(p["candidate"]["admitted"]["trigger"], "what if we");
    assert_eq!(p["candidate"]["admitted"]["matched"], "What if we");
}

#[test]
fn a_claim_whose_turn_fires_no_trigger_of_its_tag_is_refused() {
    // The model names a firing turn under another tag than the one fired.
    let s = Setup::new();
    let quote = "I always keep the release notes short.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "idea"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(proposals(&s)["total"], 0);
}

#[test]
fn the_trigger_is_checked_again_under_the_lock_against_the_config_then() {
    // The config changes while the model runs: the turn fired `always` when
    // the request was built, but under the lock `preferences` no longer has
    // that trigger, so the claim is refused and nothing is opened.
    let s = Setup::new();
    let quote = "I always keep the release notes short.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.scribe("/scribe on");
    let config = s.fx.root().join("config.toml");
    let mut script = format!(
        "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\numask 077\nprintf '%s\\n' '[tags.preferences.capture]' 'triggers = [\"from now on\"]' > '{}'\n",
        config.display()
    );
    script.push_str(&response(&candidate_answer(quote, "u-1", "preferences")));
    s.fx.fake_program("claude", &script);
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert!(s.prompts()[0].contains("message u-1: tag preferences"));
    assert!(config.exists(), "the fake model rewrote the config");
    assert_eq!(proposals(&s)["total"], 0);
    assert_eq!(s.record(ON).unwrap()["scribed_through"], "u-1");
}

#[test]
fn pass_proposes_no_candidate_under_task_without_a_config_declaring_it() {
    let s = Setup::new();
    let quote = "The migration still has to be filed as work.";
    s.write(
        ON,
        &[
            line("user", "u-1", "Please plan the migration."),
            line("assistant", "a-work", quote),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([{"quote": quote, "message": "a-work", "tag": "task"}]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(proposals(&s)["total"], 0, "task is not a shipped tag");
}

#[test]
fn no_candidate_is_opened_from_a_tool_payload() {
    let s = Setup::new();
    let payload_text = "Measured: the cache holds 42 entries after warmup.";
    let tool_line = json!({
        "type": "assistant", "uuid": "a-tool", "timestamp": "2026-11-03T06:00:00Z",
        "cwd": "/nonexistent/work/scribe",
        "message": {"role": "assistant", "content": [
            {"type": "text", "text": "Running the check now."},
            {"type": "tool_use", "name": "Bash", "input": {"command": payload_text}}
        ]}
    });
    s.write(
        ON,
        &[line("user", "u-1", "Please check the cache."), tool_line],
    );
    let mut body = answer();
    body["candidates"] = json!([{
        "quote": payload_text, "message": "a-tool", "tag": "preferences"
    }]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(proposals(&s)["total"], 0, "tool payloads are not turns");
}

#[test]
fn session_start_output_is_the_same_with_many_findings() {
    let s = Setup::new();
    s.fx.create_private_dir_all(&s.fx.root());
    let config = s.fx.root().join("config.toml");
    std::fs::write(&config, "[tags.finding]\nrequire_evidence = true\n").unwrap();
    make_private(&config);
    let before = s.hook("session-start", &s.payload("SessionStart")).stdout;
    for n in 0..6 {
        s.fx.ok(&[
            "entry",
            "add",
            "--tag",
            "finding",
            "--author",
            "opus",
            "--related",
            &format!("session:s-{n}"),
            "--evidence",
            "exp-1",
            &format!("Measured widget result {n} holds under load"),
            "--json",
        ]);
    }
    let after = s.hook("session-start", &s.payload("SessionStart")).stdout;
    assert_eq!(before, after, "findings never grow the start line");

    // An intake's pickup refuses a finding: it has no worker.
    let out =
        s.fx.run(&["entry", "pickup", "finding-1", "--author", "opus", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "not_action_tag", "{err}");
}

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
        let script = format!(
            "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\n{}",
            response(body)
        );
        self.fx.fake_program("claude", &script);
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
                    script.push_str(&format!("{n}) {} ;;\n", response(body)));
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

/// Generate an answer for the actual request. Compression returns only the
/// prose header. Explicit S9 window answers are passed through unchanged;
/// the generic fixture rewrites an open segment or starts a new one at the
/// first message when scribe has sealed the preceding one.
fn response(body: &Value) -> String {
    let envelope = |body: &Value| {
        json!({"type":"result", "subtype":"success", "is_error":false, "result":"", "structured_output":body}).to_string()
    };
    if body.get("new").is_some() {
        let header = json!({"title": body["title"], "summary": body["summary"],
                            "goals": body["goals"], "next": body["next"]});
        let mut script = format!(
            "if grep -q '^Compress only' \"$HOME/claude-stdin-$n\"; then\nprintf '%s\\n' '{}'\nelse\n",
            envelope(&header)
        );
        script.push_str(&session_response(body));
        script.push_str("\nfi");
        script
    } else {
        format!("printf '%s\\n' '{}'", envelope(body))
    }
}

fn session_response(body: &Value) -> String {
    let envelope = |body: &Value| {
        json!({"type":"result", "subtype":"success", "is_error":false, "result":"", "structured_output":body}).to_string()
    };
    if body["new"][0]["starts_at"] == "__window_first__" {
        let mut open = body.clone();
        let mut segment = open["new"][0].clone();
        segment.as_object_mut().unwrap().remove("starts_at");
        open["open"] = segment;
        open["new"] = json!([]);
        format!(
            "if grep -q '^Open segment ' \"$HOME/claude-stdin-$n\"; then\nprintf '%s\\n' '{}'\nelse\nfirst=$(sed -n 's/^--- .* · \\(.*\\) ---$/\\1/p' \"$HOME/claude-stdin-$n\" | head -1)\nprintf '%s\\n' '{}' | sed \"s/__window_first__/$first/g\"\nfi",
            envelope(&open),
            envelope(body)
        )
    } else {
        format!("printf '%s\\n' '{}'", envelope(body))
    }
}

fn answer() -> Value {
    json!({
        "title": "Defining crew levels",
        "summary": "Daniel defined what level 4 means for crews.",
        "goals": "Name the crew levels.",
        "open": null,
        "new": [{"starts_at": "__window_first__", "heading": "Crew levels", "text": "Level 4 is defined.", "topics": [], "related": []}],
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
fn header_overruns_retry_with_only_the_header_and_preserve_the_segments() {
    let setup = Setup::new();
    let mut over = answer();
    over["summary"] = json!("é".repeat(1201));
    let compressed = json!({
        "title": "Defining crew levels",
        "summary": "A compact summary.",
        "goals": "Name the crew levels.",
        "next": ["write it into the routing doc"]
    });
    setup.answers(&[Some(over), Some(compressed)]);
    setup.scribe("/scribe on");

    let report = setup.pass();

    assert_eq!(report["passes"][0]["windows"], 1);
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0].contains(DEFINED));
    assert!(
        prompts[0].contains("summary: at most 900 characters"),
        "ORB-23950: leave summary headroom"
    );
    assert!(
        prompts[1].contains("Compress only"),
        "ORB-23950: compress rather than resend the window"
    );
    assert!(
        prompts[1].contains("Current summary length: 1201 characters"),
        "ORB-23950: count Unicode characters for the model"
    );
    assert!(prompts[1].contains(&"é".repeat(1201)));
    for excluded in [
        DEFINED,
        "Level 4 is defined.",
        "--- user",
        "--- assistant",
        "terms:",
    ] {
        assert!(!prompts[1].contains(excluded), "{excluded} resent");
    }
    let record = setup.record(ON).unwrap();
    assert_eq!(record["summary"], "A compact summary.");
    assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
    assert_eq!(record["terms"].as_array().unwrap().len(), 1);
    assert_eq!(setup.control()["due"], false);
}

/// A line Claude Code wrote again on resume: the same uuid, timestamp and
/// text as the first copy, from another working directory.
fn replayed(kind: &str, uuid: &str, text: &str) -> Value {
    let mut line = line(kind, uuid, text);
    line["cwd"] = json!("/nonexistent/work/elsewhere");
    line
}

/// The fake `claude` answers `body` with every `__window_first__` replaced
/// by the id of the first message in the window it was sent, as a model
/// opening a new segment at the window's first message does.
fn answer_at_window_first(setup: &Setup, body: &Value) {
    let envelope = json!({"type":"result", "subtype":"success", "is_error":false,
                          "result":"", "structured_output":body});
    setup.fx.fake_program(
        "claude",
        &format!(
            "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\nfirst=$(sed -n 's/^--- .* · \\(.*\\) ---$/\\1/p' \"$HOME/claude-stdin-$n\" | head -1)\nprintf '%s\\n' '{envelope}' | sed \"s/__window_first__/$first/g\"\n"
        ),
    );
}

#[test]
fn a_replayed_transcript_is_read_once_and_a_segment_after_the_replay_is_accepted() {
    // ORB-24918: Claude Code wrote earlier lines into the transcript again on
    // resume. The pass resumed after the first copy of `scribed_through`,
    // re-read the replay, and refused every start in it as not following the
    // open segment.
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    assert_eq!(setup.record(ON).unwrap()["scribed_through"], "a-1");
    setup.append(
        ON,
        &[
            replayed("user", "u-1", &format!("Crews. {DEFINED}.")),
            replayed(
                "assistant",
                "a-1",
                "Level 4 crews merge their own pull requests after green CI.",
            ),
            line("user", "u-2", "Fresh work after the resume."),
        ],
    );
    let mut body = answer();
    body["terms"] = json!([]);
    body["new"][0]["heading"] = json!("After the resume");
    body["new"][0]["text"] = json!("Fresh work.");
    answer_at_window_first(&setup, &body);

    let out = setup.run(&["session", "pass", "--session", ON, "--json"], b"");

    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "written", "{report}");
    assert_eq!(report["passes"][0]["windows"], 1);
    assert_eq!(report["passes"][0]["warnings"], json!([]), "{report}");
    assert!(
        !text(&out.stderr).contains("refused"),
        "{}",
        text(&out.stderr)
    );
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 2, "one call per pass, no retry");
    assert!(prompts[1].contains("· u-2 ---"));
    for replay in ["· u-1 ---", "· a-1 ---"] {
        assert!(!prompts[1].contains(replay), "{replay} read again");
    }
    let record = setup.record(ON).unwrap();
    let log = record["log"].as_array().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0]["state"], "sealed");
    assert_eq!(log[0]["last_message"], "a-1");
    assert_eq!(log[1]["first_message"], "u-2");
    assert_eq!(log[1]["heading"], "After the resume");
    assert_eq!(record["scribed_through"], "u-2");

    // A second replay with nothing new needs no model call.
    setup.append(
        ON,
        &[
            replayed("user", "u-1", &format!("Crews. {DEFINED}.")),
            replayed("user", "u-2", "Fresh work after the resume."),
        ],
    );
    setup.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(setup.prompts().len(), 2);
    assert_eq!(setup.record(ON).unwrap()["passes"], record["passes"]);
}

#[test]
fn next_and_segment_text_get_a_target_and_their_current_length() {
    // ORB-24918: `next` and segment text had only their hard limits, so a
    // record carrying ~900 characters of `next` forward overshot 1000 when
    // the model added an item.
    let setup = Setup::new();
    let mut first = answer();
    let items: Vec<String> = (0..9)
        .map(|n| format!("item {n} {}", "x".repeat(100)))
        .collect();
    first["next"] = json!(items);
    first["new"][0]["text"] = json!("é".repeat(650));
    setup.answer(&first);
    setup.scribe("/scribe on");
    setup.pass();
    let record = setup.record(ON).unwrap();
    let next_chars = record["next"].as_str().unwrap().chars().count();
    assert!(next_chars > 900, "{next_chars}");

    setup.append(ON, &[line("user", "u-2", "One more step.")]);
    let mut over = answer();
    over["new"] = json!([]);
    over["open"] = json!(null);
    let mut more = items.clone();
    more.push(format!("item 9 {}", "x".repeat(100)));
    over["next"] = json!(more);
    let compressed = json!({
        "title": "Defining crew levels",
        "summary": "Daniel defined what level 4 means for crews.",
        "goals": "Name the crew levels.",
        "next": ["write it into the routing doc"]
    });
    // The fake numbers calls across runs: the first pass was call 0.
    setup.answers(&[None, Some(over), Some(compressed)]);

    let out = setup.run(&["session", "pass", "--session", ON], b"");

    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 3);
    let window = &prompts[1];
    assert!(
        window.contains(
            "next: the open next steps, one short item each, at most 750 characters in all \
             (hard limit 1000), counting a `- ` prefix per item and a newline between items"
        ),
        "{window}"
    );
    assert!(window.contains(&format!("Current next length: {next_chars} characters.")));
    assert!(window.contains("Segment text is at most 700 characters (hard limit 900)"));
    assert!(window.contains("text length: 650 characters\n"));
    let compress = &prompts[2];
    assert!(compress.starts_with("Compress only"));
    assert!(compress.contains("next: at most 750 characters total (hard limit 1000)"));
    assert!(compress.contains("Current next length: 1099 characters."));
    // The retry resolved the refusal: nothing at warn level.
    assert!(
        !text(&out.stderr).contains("refused"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        setup.record(ON).unwrap()["next"],
        "- write it into the routing doc"
    );
}

#[test]
fn omitted_or_null_open_keeps_its_content_and_still_advances_and_seals() {
    for omitted in [true, false] {
        let setup = Setup::new();
        setup.scribe("/scribe on");
        setup.pass();
        let before = setup.record(ON).unwrap()["log"][0].clone();
        setup.append(ON, &[line("user", "u-2", "Continue the thread.")]);
        setup.hook("session-end", &setup.payload("SessionEnd"));
        let mut body = answer();
        body["new"] = json!([]);
        if omitted {
            body.as_object_mut().unwrap().remove("open");
        }
        setup.answer(&body);

        setup.pass();

        let record = setup.record(ON).unwrap();
        let segment = &record["log"][0];
        for field in [
            "heading",
            "text",
            "topics",
            "related",
            "first_message",
            "from",
        ] {
            assert_eq!(segment[field], before[field], "{field}");
        }
        assert_eq!(segment["last_message"], "u-2");
        assert_eq!(segment["windows"], 2);
        assert_eq!(segment["state"], "sealed");
        assert_eq!(record["scribed_through"], "u-2");
        assert_eq!(setup.prompts().len(), 2, "no retry needed");
        assert!(
            setup.prompts()[1].contains(&format!(
                "Current summary length: {} characters",
                answer()["summary"].as_str().unwrap().chars().count()
            )),
            "ORB-23950: tell the model the previous summary length"
        );
    }
}

#[test]
fn unexpected_open_becomes_the_first_new_segment() {
    let setup = Setup::new();
    let mut body = answer();
    body["open"] = json!({"heading": "Opening thread", "text": "Opening details.", "topics": [], "related": []});
    body["new"][0]["starts_at"] = json!("a-1");
    setup.answer(&body);
    setup.scribe("/scribe on");

    setup.pass();

    let record = setup.record(ON).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 2);
    assert_eq!(record["log"][0]["text"], "Opening details.");
    assert_eq!(record["log"][0]["first_message"], "u-1");
    assert_eq!(record["log"][0]["last_message"], "u-1");
    assert_eq!(record["log"][0]["state"], "sealed");
    assert_eq!(record["log"][1]["first_message"], "a-1");
    assert_eq!(setup.prompts().len(), 1);
}

#[test]
fn repeated_header_budget_misses_are_repaired_after_retry_and_reported() {
    for (field, value, expected, boundary) in [
        (
            "summary",
            json!(format!("Keep this sentence. {}", "é".repeat(1200))),
            "Keep this sentence.".to_owned(),
            "sentence",
        ),
        (
            "goals",
            json!(format!("Keep these words {}", "é".repeat(600))),
            "Keep these words…".to_owned(),
            "word",
        ),
        (
            "summary",
            json!(format!("Keep this sentence.\" {}", "é".repeat(1200))),
            "Keep this sentence.\"".to_owned(),
            "sentence",
        ),
        (
            "title",
            json!("é".repeat(121)),
            format!("{}…", "é".repeat(119)),
            "word",
        ),
        (
            "next",
            json!(["Keep this bullet", "é".repeat(1000)]),
            "- Keep this bullet".to_owned(),
            "bullet",
        ),
        (
            "next",
            json!([format!("Keep these words {}", "é".repeat(1000))]),
            "- Keep these words…".to_owned(),
            "word",
        ),
    ] {
        let setup = Setup::new();
        let mut over = answer();
        over[field] = value;
        setup.answers(&[Some(over.clone()), Some(over)]);
        setup.scribe("/scribe on");

        let out = setup.run_any(&["session", "pass", "--json"], b"");

        assert_eq!(out.status.code(), Some(0), "{field}: {}", text(&out.stderr));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["passes"][0]["windows"], 1);
        let warning = report["passes"][0]["warnings"][0].as_str().unwrap();
        assert!(
            warning.contains(field) && warning.contains(boundary),
            "{warning}"
        );
        assert!(text(&out.stderr).contains(warning));
        assert_eq!(setup.prompts().len(), 2, "repair follows the retry");
        assert!(!setup.prompts()[1].contains(DEFINED), "header-only retry");
        let record = setup.record(ON).unwrap();
        assert_eq!(record[field], expected, "{field}");
        assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
        assert_eq!(record["scribed_through"], "a-1");
        assert_eq!(setup.control()["due"], false);
    }
}

#[test]
fn segment_text_repairs_keep_new_segments_and_redact_before_writing() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.append(
        ON,
        &[
            line("user", "u-2", "Second."),
            line("assistant", "a-2", "Third."),
        ],
    );
    let mut over = answer();
    over["open"] = json!({"heading": "First", "text": format!("Keep first. {}", "é".repeat(900)), "topics": [], "related": []});
    over["new"] = json!([
        {"starts_at": "u-2", "heading": "Second", "text": format!("Keep sensitive-value-123. {}", "é".repeat(900)), "topics": [], "related": []},
        {"starts_at": "a-2", "heading": "Third", "text": format!("Keep third. {}", "é".repeat(900)), "topics": [], "related": []}
    ]);
    setup.answer(&over);
    setup.config("[redact]\nenv = [\"TEST_SECRET\"]\n");
    let mut cmd = setup
        .fx
        .cmd(&["session", "pass", "--session", ON, "--json"]);
    cmd.env("PATH", &setup.path)
        .env("TEST_SECRET", "sensitive-value-123");

    let out = setup.run_with(cmd, b"");

    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["warnings"].as_array().unwrap().len(), 3);
    assert_eq!(report["passes"][0]["segments_sealed"], 2);
    assert_eq!(setup.prompts().len(), 3, "initial call, miss and one retry");
    // Reading the stored record does not require loading the model config.
    let record = setup.record(ON).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 3);
    assert_eq!(record["log"][0]["text"], "Keep first.");
    assert_eq!(record["log"][0]["last_message"], "a-1");
    assert_eq!(record["log"][1]["text"], "Keep [redacted:TEST_SECRET].");
    assert_eq!(record["log"][1]["first_message"], "u-2");
    assert_eq!(record["log"][2]["text"], "Keep third.");
    assert_eq!(record["log"][2]["state"], "open");
    assert!(
        !std::fs::read_to_string(setup.record_file())
            .unwrap()
            .contains("sensitive-value-123")
    );
}

#[test]
fn starts_at_repairs_warn_in_plain_output_and_preserve_sealed_segments() {
    for overflow in [false, true] {
        let setup = Setup::new();
        setup.scribe("/scribe on");
        let mut initial = answer();
        initial["new"] = json!([
            {"starts_at": "u-1", "heading": "Sealed", "text": "Sealed history.", "topics": [], "related": []},
            {"starts_at": "a-1", "heading": "Open", "text": "Open history.", "topics": [], "related": []}
        ]);
        setup.answer(&initial);
        setup.pass();
        let before = setup.record(ON).unwrap();
        setup.append(ON, &[line("user", "u-2", "More.")]);
        let mut bad = answer();
        bad["new"][0]["starts_at"] = json!("a-1");
        bad["new"][0]["text"] = json!(if overflow {
            "é".repeat(900)
        } else {
            "Continued history.".to_owned()
        });
        setup.answer(&bad);

        let out = setup.run(&["session", "pass", "--session", ON], b"");

        assert!(text(&out.stderr).contains("window 1: repaired new segment 1 starts_at"));
        assert!(text(&out.stdout).contains("written"));
        assert_eq!(setup.prompts().len(), 3);
        let record = setup.record(ON).unwrap();
        assert_eq!(record["log"][0], before["log"][0]);
        assert_eq!(record["scribed_through"], "u-2");
        if overflow {
            assert_eq!(record["log"].as_array().unwrap().len(), 3);
            assert_eq!(record["log"][2]["first_message"], "u-2");
            assert_eq!(record["log"][2]["text"], "é".repeat(900));
        } else {
            assert_eq!(record["log"].as_array().unwrap().len(), 2);
            assert_eq!(
                record["log"][1]["text"],
                "Open history.\n\nContinued history."
            );
        }
    }
}

#[test]
fn header_length_retries_cover_every_prose_field_and_recheck_the_result() {
    for (field, value) in [
        ("title", json!("t".repeat(121))),
        ("goals", json!("g".repeat(601))),
        ("next", json!(["n".repeat(999)])),
    ] {
        let setup = Setup::new();
        let mut over = answer();
        over[field] = value;
        over["summary"] = json!("é".repeat(1201));
        setup.answers(&[Some(over), Some(answer())]);
        setup.scribe("/scribe on");

        assert_eq!(setup.pass()["passes"][0]["result"], "written", "{field}");
        assert_eq!(setup.prompts().len(), 2);
        assert!(
            !setup.prompts()[1].contains(DEFINED),
            "window resent for {field}"
        );
        assert_eq!(
            setup.record(ON).unwrap()[field],
            match field {
                "next" => json!("- write it into the routing doc"),
                _ => answer()[field].clone(),
            }
        );
    }

    // A shorter header can still be invalid; it receives no third attempt.
    for (field, value, reason) in [
        ("summary", json!(""), "summary is empty"),
        (
            "goals",
            json!("## Injected"),
            "Goals holds a section heading",
        ),
        (
            "title",
            json!("two\nlines"),
            "title must be one non-empty line",
        ),
        ("model_failure", Value::Null, "the model failed"),
    ] {
        let setup = Setup::new();
        setup.scribe("/scribe on");
        setup.pass();
        let before = std::fs::read(setup.record_file()).unwrap();
        setup.append(ON, &[line("user", "u-2", "A new turn.")]);
        setup.hook("session-end", &setup.payload("SessionEnd"));
        let mut over = answer();
        over["summary"] = json!("s".repeat(1201));
        let mut invalid = answer();
        invalid[field] = value;
        // Script indices include the already successful initial pass.
        setup.answers(&[
            Some(answer()),
            Some(over),
            (field != "model_failure").then_some(invalid),
        ]);

        let out = setup.run_any(&["session", "pass", "--json"], b"");

        assert_eq!(out.status.code(), Some(1));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            report["passes"][0]["error_code"],
            if field == "model_failure" {
                "model_failed"
            } else {
                "session_answer_invalid"
            }
        );
        let error = report["passes"][0]["error"].as_str().unwrap();
        assert!(error.contains("Summary has 1201 characters"), "{error}");
        assert!(error.contains(reason), "{error}");
        assert_eq!(setup.prompts().len(), 3, "only one retry");
        assert_eq!(std::fs::read(setup.record_file()).unwrap(), before);
        assert_eq!(setup.control()["due"], true);
    }
}

#[test]
fn header_overruns_with_other_defects_use_the_full_window_retry() {
    for defect in ["segment", "topics", "heading"] {
        let setup = Setup::new();
        let mut over = answer();
        over["summary"] = json!("s".repeat(1201));
        match defect {
            "segment" => over["new"][0]["text"] = json!("s".repeat(901)),
            "topics" => over["topics"] = json!(["Invalid Topic"]),
            "heading" => over["summary"] = json!(format!("{}\n## Injected", "s".repeat(1201))),
            _ => unreachable!(),
        }
        setup.answers(&[Some(over), Some(answer())]);
        setup.scribe("/scribe on");

        assert_eq!(setup.pass()["passes"][0]["result"], "written");

        let prompts = setup.prompts();
        assert_eq!(prompts.len(), 2);
        assert!(
            prompts[1].contains(DEFINED),
            "{defect}: full window required"
        );
        assert!(prompts[1].contains("Summary has 1201 characters") || defect == "heading");
        assert!(!prompts[1].starts_with("Compress only"), "{defect}");
    }
}

#[test]
fn impossible_unexpected_open_conversions_are_refused_with_a_reason() {
    for (starts, reason) in [
        (vec!["u-1"], "distinct and in transcript order"),
        (vec!["a-1", "u-2", "a-2"], "at most 3 segments"),
    ] {
        let setup = Setup::new();
        setup.append(
            ON,
            &[
                line("user", "u-2", "More."),
                line("assistant", "a-2", "Done."),
            ],
        );
        let mut body = answer();
        body["open"] =
            json!({"heading": "Opening", "text": "Details.", "topics": [], "related": []});
        body["new"] = json!(starts.iter().map(|id| json!({
            "starts_at": id, "heading": "Another", "text": "Details.", "topics": [], "related": []
        })).collect::<Vec<_>>());
        setup.answer(&body);
        setup.scribe("/scribe on");

        let out = setup.run_any(&["session", "pass", "--json"], b"");

        assert_eq!(out.status.code(), Some(1));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["passes"][0]["error_code"], "session_answer_invalid");
        let error = report["passes"][0]["error"].as_str().unwrap();
        assert!(error.contains(reason), "{error}");
        assert_eq!(setup.prompts().len(), 2);
        assert!(setup.record(ON).is_none());
        assert_eq!(setup.control()["due"], true);
    }
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
    over["summary"] = json!(format!("{}\n## Injected", "s".repeat(1201)));
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
    assert_eq!(current["format"], 12);
    assert_eq!(current["sessions"][ON]["consecutive_failures"], 0);
    assert_eq!(current["sessions"][ON]["retry_after"], Value::Null);
    assert_eq!(current["sessions"][ON]["pending_links"], json!([]));
    for missing in ["consecutive_failures", "retry_after", "pending_links"] {
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

/// The audit log's lines, oldest first.
fn audit_log(setup: &Setup) -> Vec<Value> {
    std::fs::read_to_string(setup.fx.root().join("audit.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_failed_session_catalog_writes_one_failure_audit_line() {
    let setup = Setup::new();
    setup.fx.fake_program("claude", BROKEN);
    let out = setup.run_any(&["session", "catalog", NEVER, "--author", "opus"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    // The switch was written and stays due, yet the command is one outcome.
    assert!(setup.fx.root().join("control.json").exists());
    let audit = audit_log(&setup);
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["command"], "session catalog");
    assert_eq!(audit[0]["status"], "failure");
    assert_eq!(audit[0]["error_code"], "model_failed");
    assert_eq!(audit[0]["target"], format!("session:{NEVER}"));
    assert_eq!(audit[0]["author"], "opus");

    // A catalog that succeeds still logs its switch, then its window.
    setup.answer(&answer());
    let out = setup.run_any(&["session", "catalog", NEVER, "--author", "opus"], b"");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let audit = audit_log(&setup);
    assert_eq!(audit.len(), 2, "a resume wrote no switch: {audit:?}");
    assert_eq!(audit[1]["status"], "success");
}

#[test]
fn scribe_catalog_in_a_typed_prompt_catalogs_a_past_session_as_daniel() {
    let setup = Setup::new();
    // An id that names no transcript is refused, audited by code, and
    // writes no switch.
    let reason = setup.scribe("/scribe catalog 0a1b2c3d-0000-4000-8000-00000000dead");
    assert!(
        reason.starts_with("scribe: /scribe catalog 0a1b2c3d-0000-4000-8000-00000000dead refused"),
        "{reason}"
    );
    assert!(!setup.fx.root().join("control.json").exists());

    // Typed by Daniel in another session: the switch is written as his,
    // and no window runs in the hook.
    let reason = setup.scribe(&format!("/scribe catalog {NEVER}"));
    assert!(
        reason.starts_with(&format!("scribe: cataloging {NEVER} as yours")),
        "{reason}"
    );
    let text = std::fs::read_to_string(setup.fx.root().join("control.json")).unwrap();
    let switch = serde_json::from_str::<Value>(&text).unwrap()["sessions"][NEVER].clone();
    assert_eq!(switch["on"], true);
    assert_eq!(switch["due"], true);
    assert_eq!(switch["cataloged_by"]["by"]["kind"], "daniel");
    assert!(setup.prompts().is_empty());
    assert!(setup.record(NEVER).is_none());
    let audited = setup.audit_lines();

    // Again: nothing changes and nothing is audited.
    let before = setup.control_bytes();
    let reason = setup.scribe(&format!("/scribe catalog {NEVER}"));
    assert!(reason.contains("already being cataloged"), "{reason}");
    assert_eq!(setup.control_bytes(), before);
    assert_eq!(setup.audit_lines(), audited);

    // The next session pass runs its windows as a catalog and keeps
    // Daniel's attribution; the transcript is days old, so it ends final
    // and off.
    let pass = setup.pass();
    assert_eq!(pass["passes"][0]["result"], "written", "{pass}");
    let record = setup.record(NEVER).unwrap();
    assert_eq!(record["status"], "final", "{record}");
    assert_eq!(record["cataloged_by"]["by"]["kind"], "daniel", "{record}");
    let text = std::fs::read_to_string(setup.fx.root().join("control.json")).unwrap();
    let switch = serde_json::from_str::<Value>(&text).unwrap()["sessions"][NEVER].clone();
    assert_eq!(switch["on"], false);
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
    // The record is written at format 12: a header and one open segment
    // holding this pass's segment text.
    let file = std::fs::read_to_string(setup.record_file()).unwrap();
    assert!(file.starts_with("---\nformat: 12\n"), "{file}");
    assert!(file.contains("\n## Log\n### 1. "), "{file}");
    assert!(!file.contains("## Outcome"), "{file}");
    assert_eq!(record["active"], true);
    assert_eq!(record["outcome"], "");
    assert_eq!(record["log"][0]["state"], "open");
    assert_eq!(record["log"][0]["origin"], "pass");
    assert_eq!(record["log"][0]["last_message"], "a-1");

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
    over["summary"] = json!(format!("{}\n## Injected", "s".repeat(1201)));
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
    let reason = report["passes"][0]["error"].as_str().unwrap();
    assert!(reason.contains("model failed"), "{reason}");
    assert!(reason.chars().count() <= 300, "{reason}");
    // The model's failure is logged first; the error object is the last line.
    let stderr = text(&out.stderr);
    let error: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(error["code"], "session_pass_failed", "{stderr}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains(&format!("{ON} model_failed: {reason}")),
        "{}",
        error["error"]
    );
    let audit = std::fs::read_to_string(setup.fx.root().join("audit.jsonl")).unwrap();
    assert!(
        !audit.contains(reason),
        "audit exposed the failure reason: {audit}"
    );

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
    assert!(report["passes"][0].get("error").is_none());
    assert!(setup.record(ON).is_some());
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn a_failed_pass_redacts_the_model_reason_in_json_and_the_aggregate_error() {
    const SECRET: &str = "pass-error-secret-value-123";
    const SECRET_NAME: &str = "SCRIBE_PASS_ERROR_SECRET";
    let setup = Setup::new();
    setup.config(&format!("[redact]\nenv = [\"{SECRET_NAME}\"]\n"));
    setup.scribe("/scribe on");
    let script =
        format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' 'model error: {SECRET}' >&2\nexit 3\n");
    setup.fx.fake_program("claude", &script);
    let mut command = setup
        .fx
        .cmd(&["session", "pass", "--session", ON, "--json"]);
    command.env("PATH", &setup.path).env(SECRET_NAME, SECRET);

    let out = setup.run_with(command, b"");

    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let reason = report["passes"][0]["error"].as_str().unwrap();
    assert!(
        reason.contains("[redacted:SCRIBE_PASS_ERROR_SECRET]"),
        "{reason}"
    );
    assert!(!reason.contains(SECRET), "{reason}");
    assert!(reason.chars().count() <= 300, "{reason}");
    let stderr = text(&out.stderr);
    let error: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(error["code"], "session_pass_failed", "{stderr}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains(&format!("{ON} model_failed: {reason}")),
        "{}",
        error["error"]
    );
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
    assert!(
        message.contains(&format!("{ON} model_failed: the model failed")),
        "{message}"
    );
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

/// A second session, spawned from `ON`.
const CHILD: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61";
/// A well-formed id scribe has never seen.
const UNKNOWN: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e62";

impl Setup {
    /// `/scribe …` typed in `session`: the hook's block reason.
    fn scribe_in(&self, session: &str, prompt: &str) -> String {
        let payload = json!({"session_id": session,
               "transcript_path": self.transcript(session),
               "cwd": self.cwd,
               "permission_mode": "default",
               "hook_event_name": "UserPromptSubmit",
               "prompt": prompt});
        let out = self.hook("user-prompt-submit", &payload);
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(reply["decision"], "block", "{reply}");
        reply["reason"].as_str().unwrap().to_owned()
    }

    /// The `error_code` of every audit line, oldest first.
    fn audit_codes(&self) -> Vec<Value> {
        std::fs::read_to_string(self.fx.root().join("audit.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap()["error_code"].clone())
            .collect()
    }

    fn switch_of(&self, session: &str) -> Value {
        let text = std::fs::read_to_string(self.fx.root().join("control.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["sessions"][session].clone()
    }
}

#[test]
fn a_session_links_to_its_parent_and_its_pass_reads_the_parent_as_context() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    assert_eq!(setup.pass()["passes"][0]["result"], "written");
    setup.write(
        CHILD,
        &[line(
            "user",
            "c-1",
            "Side session: fix the flaky test only.",
        )],
    );

    // Refused, each with its code, and nothing switched or linked.
    let reason = setup.scribe_in(CHILD, &format!("/scribe link {ON}"));
    assert!(reason.contains("refused"), "{reason}");
    let reason = setup.scribe_in(CHILD, &format!("/scribe on --from {UNKNOWN}"));
    assert!(reason.contains("refused"), "{reason}");
    let reason = setup.scribe_in(CHILD, &format!("/scribe on --from {CHILD}"));
    assert!(reason.contains("cannot link to itself"), "{reason}");
    assert_eq!(
        setup.switch_of(CHILD),
        Value::Null,
        "a refusal switches nothing"
    );
    assert_eq!(
        setup.audit_codes()[2..],
        [
            json!("link_session_off"),
            json!("link_unknown_session"),
            json!("link_self")
        ]
    );

    // Switched on and linked in one command; the link waits on the switch
    // until the first pass writes the record.
    let reason = setup.scribe_in(CHILD, &format!("/scribe on --from {ON}"));
    assert!(
        reason.starts_with(&format!(
            "scribe: on — this session is cataloged from now on and linked: spawned_from {ON}"
        )),
        "{reason}"
    );
    assert_eq!(setup.switch_of(CHILD)["on"], true);
    assert_eq!(
        setup.switch_of(CHILD)["pending_links"],
        json!([{"type": "spawned_from", "session": ON}])
    );
    let reason = setup.scribe_in(CHILD, &format!("/scribe link {ON}"));
    assert!(reason.ends_with("nothing changed"), "{reason}");

    // The parent links back to its child: a cycle, refused.
    let audit = setup.audit_lines();
    let reason = setup.scribe_in(ON, &format!("/scribe link {CHILD} --type continues"));
    assert!(reason.contains("cycle"), "{reason}");
    assert_eq!(setup.audit_codes()[audit..], [json!("link_cycle")]);

    // The child's pass carries the parent's latest record as context.
    let before = setup.prompts().len();
    let report = setup.pass();
    assert_eq!(report["passes"][0]["session"], CHILD, "{report}");
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), before + 1);
    let prompt = &prompts[before];
    for part in [
        format!("--- Linked session (data): this session was spawned from session {ON} ---"),
        String::from("title: Defining crew levels"),
        String::from("Daniel defined what level 4 means for crews."),
        String::from("do not retell the linked session"),
    ] {
        assert!(prompt.contains(&part), "{part:?} missing from:\n{prompt}");
    }

    // The link moved onto the child's record; the parent shows the child.
    assert_eq!(setup.switch_of(CHILD)["pending_links"], json!([]));
    let child = setup.record(CHILD).unwrap();
    assert_eq!(
        child["links"],
        json!([{"type": "spawned_from", "session": ON, "title": "Defining crew levels"}])
    );
    assert_eq!(child["related"], json!([]), "related is unchanged");
    let parent = setup.record(ON).unwrap();
    assert_eq!(parent["links"], json!([]));
    assert_eq!(
        parent["children"],
        json!([{"type": "spawned_from", "session": CHILD, "title": "Defining crew levels"}])
    );
    let tree: Value = serde_json::from_slice(
        &setup
            .run(&["session", "list", "--tree", "--json"], b"")
            .stdout,
    )
    .unwrap();
    let order: Vec<&str> = tree["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(order, [ON, CHILD]);
    let plain = setup.run(&["session", "list", "--tree"], b"");
    let rows: Vec<Vec<String>> = text(&plain.stdout)
        .lines()
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect();
    assert_eq!(
        rows[1][..2],
        [CHILD.to_owned(), ON.to_owned()],
        "PARENT column"
    );
}

#[test]
fn s9_answers_write_bounded_segments_instead_of_one_outcome() {
    let setup = Setup::new();
    setup.append(
        ON,
        &[
            line("user", "u-2", "A second thread."),
            line("assistant", "a-2", "A third thread."),
        ],
    );
    setup.scribe("/scribe on");
    let mut body = answer();
    body.as_object_mut().unwrap().remove("outcome");
    body["open"] = Value::Null;
    body["new"] = json!(["u-1", "u-2", "a-2"].map(|id| json!({
        "starts_at": id, "heading": format!("Thread {id}"), "text": "é".repeat(900),
        "topics": [], "related": []
    })));
    setup.answer(&body);
    let out = setup.run_any(&["session", "pass", "--json"], b"");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let record = setup.record(ON).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 3);
    assert_eq!(record["log"][0]["state"], "sealed");
    assert_eq!(record["log"][0]["last_message"], "a-1");
    assert_eq!(record["log"][1]["last_message"], "u-2");
    assert_eq!(record["log"][2]["state"], "open");
}

#[test]
fn a_pass_refuses_when_the_record_changed_since_it_was_loaded() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    // A rebuilt record, as `finish_rebuild` would publish it: same record
    // with a new `updated` revision and a rebuilt summary.
    let original = std::fs::read_to_string(setup.record_file()).unwrap();
    let rebuilt_text: String = original
        .lines()
        .map(|line| {
            if line.starts_with("updated: ") {
                "updated: 2030-01-01T00:00:00Z".to_owned()
            } else if line == "Daniel defined what level 4 means for crews." {
                "Rebuilt summary.".to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert_ne!(rebuilt_text, original);
    let rebuilt_file = setup.fx.home().join("rebuilt.md");
    std::fs::write(&rebuilt_file, &rebuilt_text).unwrap();
    make_private(&rebuilt_file);
    setup.append(
        ON,
        &[
            line("user", "u-2", "More work."),
            line("assistant", "a-2", "Done."),
        ],
    );
    // The fake model deterministically publishes the rebuilt record before
    // returning its answer for the pass's first window, as a rebuild that
    // finished while the pass was running would. No timing race is involved.
    let quote = |path: PathBuf| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let record = quote(setup.record_file());
    let rebuilt = quote(rebuilt_file);
    let script = format!(
        "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\nif [ \"$n\" = 1 ]; then\n  cp {} {}\nfi\n{}",
        rebuilt,
        record,
        response(&answer())
    );
    setup.fx.fake_program("claude", &script);
    let out = setup.run_any(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "failed");
    assert_eq!(report["passes"][0]["error_code"], "record_changed");
    assert!(
        report["passes"][0]["error"]
            .as_str()
            .unwrap()
            .contains("changed since it was loaded"),
        "{}",
        report["passes"][0]["error"]
    );
    assert_eq!(
        std::fs::read(setup.record_file()).unwrap(),
        rebuilt_text.as_bytes(),
        "the rebuilt record is left byte-identical"
    );
    assert_eq!(setup.control()["due"], true, "the session stays due");
}

fn rebuild_args(confirm: bool) -> Vec<&'static str> {
    let mut args = vec!["session", "rebuild", ON, "--author", "test-agent", "--json"];
    if confirm {
        args.push("--confirm");
    }
    args
}

#[test]
fn rebuild_refusals_are_audited_and_leave_the_record_unchanged() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    let before = std::fs::read(setup.record_file()).unwrap();
    let out = setup.run_any(&rebuild_args(false), b"");
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "confirm_required");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("1 segment(s) and 1 pass(es)")
    );
    setup.scribe("/scribe off");
    setup.scribe("/scribe on");
    let out = setup.run_any(&rebuild_args(true), b"");
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stderr).unwrap()["code"],
        "session_busy"
    );
    setup.scribe("/scribe off");
    std::fs::remove_file(setup.transcript(ON)).unwrap();
    let out = setup.run_any(&rebuild_args(true), b"");
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stderr).unwrap()["code"],
        "transcript_missing"
    );
    assert_eq!(std::fs::read(setup.record_file()).unwrap(), before);
    assert_eq!(setup.prompts().len(), 1, "no model calls for refusals");
    let audit = std::fs::read_to_string(setup.fx.root().join("audit.jsonl")).unwrap();
    let rebuilds: Vec<Value> = audit
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|l| l["command"] == "session rebuild")
        .collect();
    assert_eq!(rebuilds.len(), 3);
    for line in rebuilds {
        assert_eq!(line["target"], format!("session:{ON}"));
        assert_eq!(line["status"], "failure");
    }
}

#[test]
fn rebuild_resumes_hidden_checkpoints_and_preserves_identity_links_and_activity() {
    use scribe_core::activity::CatalogedBy;
    use scribe_core::{Author, AuthorKind, LinkType, Redactor, SessionLink, Store};
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    let store = Store::open(setup.fx.root());
    let id = ON.parse().unwrap();
    let mut original = store.session(&id).unwrap().record;
    original.active = false;
    original.keep_active = true;
    original.active_change = Some(scribe_core::activity::ActiveChange {
        at: original.created,
        by: Author {
            kind: AuthorKind::Agent,
            name: "curator".parse().unwrap(),
        },
        route: scribe_core::activity::ActiveRoute::Manual,
        reason: String::from("No longer current"),
        cite: Some(String::from("session:source#1")),
    });
    original.links = vec![SessionLink {
        kind: LinkType::Continues,
        session: NEVER.parse().unwrap(),
    }];
    original.cataloged_by = Some(CatalogedBy {
        at: original.created,
        by: Author {
            kind: AuthorKind::Agent,
            name: "cataloger".parse().unwrap(),
        },
    });
    original.status = scribe_core::SessionStatus::Final;
    original.log[0].state = scribe_core::SegmentState::Sealed;
    original.log[0].origin = scribe_core::SegmentOrigin::Upgrade;
    let expected = original.updated;
    store
        .settle_pass(
            &id,
            Some(original.clone()),
            false,
            Some(expected),
            &Redactor::new([]),
        )
        .unwrap();
    let before = std::fs::read(setup.record_file()).unwrap();
    setup.config("[catalog]\nwindow_messages = 2\nsegment_windows = 2\n");
    setup.append(
        ON,
        &[
            line("user", "u-2", "New work."),
            line("assistant", "a-2", "Built it."),
            line("user", "u-3", "Finish."),
        ],
    );
    setup.answers(&[None, Some(answer()), None, Some(answer()), Some(answer())]);
    let out = setup.run_any(&rebuild_args(true), b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 1);
    assert_eq!(report["passes"][0]["error_code"], "model_failed");
    let checkpoint = setup
        .record_file()
        .with_file_name(format!(".{ON}.rebuild.md"));
    assert!(checkpoint.is_file());
    assert_eq!(std::fs::read(setup.record_file()).unwrap(), before);
    assert_eq!(
        store
            .list_sessions(&scribe_core::SessionFilter {
                include_inactive: true,
                ..Default::default()
            })
            .unwrap()
            .listing
            .items
            .len(),
        1
    );
    let out = setup.run(&rebuild_args(true), b"");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 2);
    assert_eq!(
        report["passes"][0]["author"],
        json!({"kind": "agent", "name": "test-agent"})
    );
    let shown: Value =
        serde_json::from_slice(&setup.run(&["session", "show", ON, "--json"], b"").stdout).unwrap();
    assert_eq!(
        shown["rebuilt_by"],
        json!({"kind": "agent", "name": "test-agent"})
    );
    let rebuild_line = std::fs::read_to_string(store.audit_path())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .rfind(|l| l["command"] == "session rebuild")
        .unwrap();
    assert_eq!(rebuild_line["status"], "success");
    assert_eq!(rebuild_line["author"], "test-agent");
    let rebuilt = store.session(&id).unwrap().record;
    assert_eq!(rebuilt.id, original.id);
    assert_eq!(rebuilt.created, original.created);
    assert_eq!(rebuilt.links, original.links);
    assert_eq!(rebuilt.active, original.active);
    assert_eq!(rebuilt.keep_active, original.keep_active);
    assert_eq!(rebuilt.active_change, original.active_change);
    assert_eq!(rebuilt.cataloged_by, original.cataloged_by);
    assert_eq!(rebuilt.scribed_through.as_deref(), Some("u-3"));
    assert_eq!(rebuilt.passes, 3);
    assert_eq!(rebuilt.status, scribe_core::SessionStatus::Final);
    assert!(
        rebuilt
            .log
            .iter()
            .all(|s| s.origin == scribe_core::SegmentOrigin::Rebuild
                && s.state == scribe_core::SegmentState::Sealed)
    );
    assert!(!checkpoint.exists());
    let prompts = setup.prompts();
    assert!(
        !prompts[3].contains("--- user · u-1 ---"),
        "resume starts at failed window"
    );
    assert!(prompts[3].contains("--- user · u-2 ---"));
    assert!(
        prompts[1].contains("There is no record yet"),
        "first rebuild window has no previous record"
    );
}

#[test]
fn rebuild_folds_newer_transcript_messages_after_a_visible_revision_changes() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    let extra = line("user", "u-later", "Work arriving during the rebuild.");
    let extra_file = setup.fx.home().join("later.jsonl");
    std::fs::write(&extra_file, format!("{extra}\n")).unwrap();
    // The fake model deterministically performs the competing writer's
    // publication before returning its first rebuild answer. No timing race,
    // sleeps or concurrent test load is involved.
    let quote = |path: PathBuf| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let record = quote(setup.record_file());
    let transcript = quote(setup.transcript(ON));
    let extra_file = quote(extra_file);
    let script = format!(
        r#"#!/bin/sh
n=$(ls "$HOME" | grep -c '^claude-stdin')
cat > "$HOME/claude-stdin-$n"
if [ "$n" = 1 ]; then
  sed 's/^updated: .*/updated: 2030-01-01T00:00:00Z/' {record} > "$HOME/revised.md"
  chmod 600 "$HOME/revised.md"
  mv "$HOME/revised.md" {record}
  cat {extra_file} >> {transcript}
fi
{response}
"#,
        response = response(&answer())
    );
    setup.fx.fake_program("claude", &script);
    let control_before = setup.control_bytes();
    let out = setup.run(&rebuild_args(true), b"");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 2);
    let record = setup.record(ON).unwrap();
    assert_eq!(record["scribed_through"], "u-later");
    assert_eq!(record["log"][0]["origin"], "rebuild");
    assert_eq!(setup.control_bytes(), control_before);
    let prompts = setup.prompts();
    assert!(prompts[2].contains("--- user · u-later ---"));
    assert!(!prompts[2].contains("--- user · u-1 ---"));
}

#[test]
fn rebuild_repairs_a_repeated_third_window_budget_miss_and_reports_its_window() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.config("[catalog]\nwindow_messages = 2\n");
    setup.append(
        ON,
        &[
            line("user", "u-2", "Second window."),
            line("assistant", "a-2", "Continue."),
            line("user", "u-3", "Third window."),
        ],
    );
    let mut over = answer();
    over["open"] = Value::Null;
    over["new"] = json!([]);
    over["next"] = json!(["Keep this bullet", "é".repeat(1000)]);
    setup.answers(&[
        None,
        Some(answer()),
        Some(answer()),
        Some(over.clone()),
        Some(over),
    ]);

    let out = setup.run(&rebuild_args(true), b"");

    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 3);
    assert_eq!(
        report["passes"][0]["warnings"],
        json!(["window 3: repaired next: cut at bullet boundary (budget 1000 characters)"])
    );
    assert!(text(&out.stderr).contains("window 3: repaired next"));
    assert_eq!(setup.prompts().len(), 5);
    let record = setup.record(ON).unwrap();
    assert_eq!(record["next"], "- Keep this bullet");
    assert_eq!(record["scribed_through"], "u-3");
    assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
    assert!(
        !setup
            .record_file()
            .with_file_name(format!(".{ON}.rebuild.md"))
            .exists()
    );
}

#[test]
fn rebuild_compresses_an_overlong_third_window_header_even_when_open_is_omitted() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.config("[catalog]\nwindow_messages = 2\n");
    setup.append(
        ON,
        &[
            line("user", "u-2", "Second window."),
            line("assistant", "a-2", "Continue."),
            line("user", "u-3", "Third window."),
        ],
    );
    let mut over = answer();
    over.as_object_mut().unwrap().remove("open");
    over["new"] = json!([]);
    over["summary"] = json!("s".repeat(1201));
    let compressed = json!({"title": "Rebuilt session", "summary": "All three windows.",
                            "goals": "Record the session.", "next": []});
    setup.answers(&[
        None,
        Some(answer()),
        Some(answer()),
        Some(over),
        Some(compressed),
    ]);

    let out = setup.run(&rebuild_args(true), b"");

    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 3);
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 5);
    assert!(prompts[3].contains("Third window."));
    assert!(!prompts[4].contains("Third window."), "window not resent");
    assert!(
        !prompts[4].contains("Level 4 is defined."),
        "segment not resent"
    );
    let record = setup.record(ON).unwrap();
    assert_eq!(record["summary"], "All three windows.");
    assert_eq!(record["scribed_through"], "u-3");
    assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
    assert_eq!(record["log"][0]["windows"], 3);
    assert_eq!(record["log"][0]["state"], "sealed");
    assert_eq!(record["log"][0]["origin"], "rebuild");
    assert!(
        !setup
            .record_file()
            .with_file_name(format!(".{ON}.rebuild.md"))
            .exists()
    );
}

fn candidate_answer(quote: &str, message: &str, tag: &str) -> Value {
    let mut body = answer();
    body["candidates"] = json!([{"quote": quote, "message": message, "tag": tag}]);
    body
}

fn proposals(s: &Setup) -> Value {
    serde_json::from_slice(&s.fx.ok(&["proposal", "list", "--json"]).stdout).unwrap()
}

fn type_settlement(s: &Setup, prompt: &str) -> Output {
    let mut payload = s.payload("UserPromptSubmit");
    payload["prompt"] = json!(prompt);
    s.hook("user-prompt-submit", &payload)
}

#[test]
fn candidate_sources_and_existing_statements_are_checked_at_the_binary_boundary() {
    let quote = "I prefer concise messages during lengthy builds.";
    let cases = [
        ("assistant", quote.to_owned(), "u-1", "preferences", false),
        (
            "user",
            format!("Read this: <pasted_content>{quote}</pasted_content>"),
            "u-1",
            "preferences",
            false,
        ),
        (
            "user",
            format!("```text\n{quote}\n```"),
            "u-1",
            "preferences",
            false,
        ),
        ("user", format!("> {quote}"), "u-1", "preferences", false),
        (
            "user",
            quote.to_owned(),
            "missing-turn",
            "preferences",
            false,
        ),
        (
            "user",
            "I never said the model's proposed sentence.".to_owned(),
            "u-1",
            "preferences",
            false,
        ),
        ("user", quote.to_owned(), "u-1", "task", false),
        (
            "user",
            format!("#scribe preferences: {quote}"),
            "u-1",
            "preferences",
            false,
        ),
        ("user", quote.to_owned(), "u-1", "preferences", true),
    ];
    for (role, text, message, tag, existing) in cases {
        let s = Setup::new();
        s.write(ON, &[line(role, "u-1", &text)]);
        if existing {
            let root = s.fx.root();
            let dir = root.join("entries/preferences");
            std::fs::create_dir_all(&dir).unwrap();
            for path in [&root, &root.join("entries"), &dir] {
                make_private(path);
            }
            let path = dir.join("preferences-1.md");
            std::fs::write(&path, support::entry_text(1, quote)).unwrap();
            make_private(&path);
        }
        s.answer(&candidate_answer(quote, message, tag));
        s.scribe("/scribe on");
        s.run(&["session", "pass", "--session", ON, "--json"], b"");
        assert_eq!(
            proposals(&s)["total"],
            0,
            "unsafe source or duplicate: {role} {text}"
        );
    }
}

#[test]
fn rejected_candidates_are_never_repeated_and_are_counted_at_session_start() {
    let s = Setup::new();
    let quote = "I prefer concise messages during lengthy builds.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let start = s.hook("session-start", &s.payload("SessionStart"));
    assert!(text(&start.stdout).contains("1 proposal to review"));
    type_settlement(&s, "#scribe reject p-1");
    type_settlement(&s, "#scribe reject p-1");
    // New turn and punctuation/case changes still name the same statement.
    let repeated = "I prefer concise messages during lengthy builds!";
    s.append(ON, &[line("user", "u-2", repeated)]);
    s.answer(&candidate_answer(repeated, "u-2", "preferences"));
    s.run(&["session", "pass", "--session", ON], b"");
    let ps = proposals(&s);
    assert_eq!(ps["total"], 1);
    assert_eq!(ps["proposals"][0]["state"], "rejected");
    let es: Value = serde_json::from_slice(&s.fx.ok(&["entry", "list", "--json"]).stdout).unwrap();
    assert_eq!(es["total"], 0);
    assert_eq!(
        s.hook("session-start", &s.payload("SessionStart")).stdout,
        b""
    );
}

#[test]
fn candidates_share_a_three_slot_budget_across_windows_and_keep_cataloging() {
    let s = Setup::new();
    let lines: Vec<_> = (0..5)
        .map(|n| {
            line(
                "user",
                &format!("u-{n}"),
                &format!("I always choose option {n}."),
            )
        })
        .collect();
    s.write(ON, &lines);
    let answers: Vec<_> = (0..5)
        .map(|n| {
            Some(candidate_answer(
                &format!("I always choose option {n}."),
                &format!("u-{n}"),
                "preferences",
            ))
        })
        .collect();
    s.answers(&answers);
    s.scribe("/scribe on");
    let config = s.fx.root().join("config.toml");
    std::fs::write(&config, "[catalog]\nwindow_messages = 1\n").unwrap();
    make_private(&config);
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 3);
    assert_eq!(s.record(ON).unwrap()["passes"], 5);
    assert!(!s.control()["due"].as_bool().unwrap());
}

#[test]
fn candidate_open_cap_is_enforced_even_when_the_model_ignores_it() {
    let s = Setup::new();
    s.write(ON, &[line("user", "u-1", "I always choose option zero.")]);
    s.answer(&candidate_answer(
        "I always choose option zero.",
        "u-1",
        "preferences",
    ));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let template = proposals(&s)["proposals"][0].clone();
    let full: Vec<_> = (1..=19)
        .map(|n| {
            let mut p = template.clone();
            p["id"] = json!(format!("p-{n}"));
            p["older"] = json!(format!("preferences-{n}"));
            p["candidate"]["quote"] = json!(format!("Stored preference {n}."));
            p
        })
        .collect();
    let path = s.fx.root().join("proposals.json");
    std::fs::write(&path, json!({"format":12, "proposals":full}).to_string()).unwrap();
    make_private(&path);
    let quotes = [
        "I prefer faster incremental compilation.",
        "I always choose a blue interface.",
        "Always retain descriptive error messages.",
    ];
    let turns: Vec<_> = quotes
        .iter()
        .enumerate()
        .map(|(n, quote)| line("user", &format!("cap-{n}"), quote))
        .collect();
    s.append(ON, &turns);
    let mut body = answer();
    body["candidates"] = json!(quotes.iter().enumerate().map(|(n, quote)| json!({"quote":quote, "message":format!("cap-{n}"), "tag":"preferences"})).collect::<Vec<_>>());
    s.answer(&body);
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 20);
    s.append(
        ON,
        &[line("user", "u-2", "I prefer detailed release notes.")],
    );
    s.answer(&candidate_answer(
        "I prefer detailed release notes.",
        "u-2",
        "preferences",
    ));
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 20);
    assert_eq!(s.record(ON).unwrap()["scribed_through"], "u-2");
}

#[test]
fn candidate_ids_are_reserved_for_approval_and_agent_settlement_is_refused() {
    let s = Setup::new();
    let quote = "I always choose smaller pull requests for easier review.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let denied = s.fx.run(&[
        "proposal",
        "settle",
        "p-1",
        "--approve",
        "--author",
        "agent",
        "--json",
    ]);
    assert_eq!(denied.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&denied.stderr).unwrap()["code"],
        "daniel_only"
    );
    let direct: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "agent",
                "Agent analysis with a different content.",
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(direct["id"], "preferences-2");
    type_settlement(
        &s,
        "#scribe preferences: A separate decision Daniel explicitly marks.",
    );
    assert!(
        s.fx.root()
            .join("entries/preferences/preferences-3.md")
            .exists()
    );
    type_settlement(&s, "#scribe approve p-1");
    let entry: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entry["content"], quote);
    type_settlement(&s, "#scribe forget preferences-1");
    assert_eq!(proposals(&s)["proposals"][0]["candidate"]["quote"], "");
    type_settlement(&s, "#scribe approve p-1");
    let forgotten: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(forgotten["status"], "forgotten");
    assert_eq!(forgotten["content"], "");
}

#[test]
fn interrupted_candidate_approval_is_replayed_once_from_the_durable_proposal() {
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    // Approved journal durable; entry file not yet written at the crash point.
    let path = s.fx.root().join("proposals.json");
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let p = &mut doc["proposals"][0];
    p["state"] = json!("approved");
    p["settled_at"] = json!("2026-11-15T23:00:00Z");
    p["settled_author"] = json!({"kind":"daniel", "name":"daniel"});
    p["settled_by"] = p["candidate"]["cite"].clone();
    std::fs::write(&path, doc.to_string()).unwrap();
    s.run(&["consistency", "scan", "--json"], b"");
    s.run(&["consistency", "scan", "--json"], b"");
    let es: Value = serde_json::from_slice(&s.fx.ok(&["entry", "list", "--json"]).stdout).unwrap();
    assert_eq!(es["total"], 1);
    let entry: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entry["content"], quote);
    assert_eq!(entry["author"]["kind"], "daniel");
}

#[test]
fn pass_closes_retired_tag_candidates_before_cap_and_suppresses_their_quotes() {
    let s = Setup::new();
    s.scribe("/scribe on");
    let cite = |msg: &str| {
        json!({
            "session": ON,
            "message": msg,
            "at": "2026-11-15T00:00:00Z",
            "prompt_sha256": "0".repeat(64),
            "line": 1,
            "cwd": "/nonexistent/work/scribe"
        })
    };
    let mut initial = Vec::new();
    let tags = [
        ("decisions", 7),
        ("task", 7),
        ("finding", 2),
        ("preferences", 2),
        ("idea", 2),
    ];
    let mut n = 0usize;
    let mut retired_counts: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for (tag, count) in tags {
        for i in 1..=count {
            n += 1;
            let quote = if n == 1 {
                String::from("I always prefer retired rule one for builds.")
            } else {
                format!("I always keep {tag} item {i} in the store.")
            };
            let is_active = matches!(tag, "preferences" | "idea");
            if !is_active {
                *retired_counts.entry(tag).or_default() += 1;
            }
            initial.push(json!({
                "id": format!("p-{n}"),
                "kind": "candidate",
                "older": format!("{tag}-{i}"),
                "newer": null,
                "candidate": {
                    "tag": tag,
                    "role": "user",
                    "quote": quote,
                    "evidence": null,
                    "cite": cite(&format!("seed-{n}")),
                    "project": "scribe",
                    "provider": "claude-code",
                    "host": "host",
                    "admitted": null
                },
                "reason": format!("Unmarked {tag} statement: {quote}"),
                "quotes": [],
                "state": "open",
                "opened_at": "2026-11-15T00:01:00Z",
                "settled_at": null,
                "settled_by": null,
                "author": {"kind": "scribe", "name": "session-pass"},
                "updated_by": null,
                "settled_author": null,
                "closed_reason": null
            }));
        }
    }
    assert_eq!(initial.len(), 20);
    assert_eq!(retired_counts.values().sum::<usize>(), 16);
    let proposals_path = s.fx.root().join("proposals.json");
    std::fs::write(
        &proposals_path,
        json!({"format": 11, "proposals": initial}).to_string(),
    )
    .unwrap();
    make_private(&proposals_path);

    let new_quote = "I prefer concise summaries in every session pass.";
    s.write(ON, &[line("user", "u-new", new_quote)]);
    s.answer(&candidate_answer(new_quote, "u-new", "preferences"));
    let audit_before = s.audit_lines();
    s.run(&["session", "pass", "--session", ON, "--json"], b"");

    let listed = proposals(&s);
    assert_eq!(listed["total"], 21, "{listed}");
    let list = listed["proposals"].as_array().unwrap();
    for p in &list[..16] {
        assert_eq!(p["state"], "rejected", "{p}");
        assert_eq!(p["closed_reason"], "tag_retired", "{p}");
        assert_eq!(
            p["settled_author"],
            json!({"kind": "scribe", "name": "tag-retired"}),
            "{p}"
        );
        assert!(p["settled_by"].is_null(), "{p}");
        assert!(!p["settled_at"].is_null(), "{p}");
    }
    for p in &list[16..20] {
        assert_eq!(p["state"], "open", "{p}");
        assert!(p["closed_reason"].is_null(), "{p}");
        assert!(p["settled_author"].is_null(), "{p}");
    }
    assert_eq!(list[20]["id"], "p-21");
    assert_eq!(list[20]["older"], "preferences-3");
    assert_eq!(list[20]["state"], "open");
    assert_eq!(list[20]["candidate"]["quote"], new_quote);
    assert!(
        !s.fx.root().join("entries").exists(),
        "pass writes no entries"
    );

    let audit_text = std::fs::read_to_string(s.fx.root().join("audit.jsonl")).unwrap();
    let new_audit: Vec<Value> = audit_text
        .lines()
        .skip(audit_before)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let closed_audit_targets: Vec<&str> = new_audit
        .iter()
        .filter(|e| {
            e["command"] == "session pass"
                && e["status"] == "success"
                && e["target"].as_str().is_some_and(|t| t.starts_with("p-"))
        })
        .map(|e| e["target"].as_str().unwrap())
        .collect();
    assert_eq!(closed_audit_targets.len(), 16, "{new_audit:?}");
    for i in 1..=16 {
        assert!(closed_audit_targets.contains(&format!("p-{i}").as_str()));
    }

    // A closed candidate's quote stays in the suppression set, and a second
    // pass writes no more closes.
    let dup_quote = "I always prefer retired rule one for builds.";
    s.append(ON, &[line("user", "u-dup", dup_quote)]);
    s.answer(&candidate_answer(dup_quote, "u-dup", "preferences"));
    let audit_before_second = s.audit_lines();
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(
        proposals(&s)["total"],
        21,
        "closed candidate quote is still suppressed"
    );
    let second_audit: Vec<Value> = std::fs::read_to_string(s.fx.root().join("audit.jsonl"))
        .unwrap()
        .lines()
        .skip(audit_before_second)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(second_audit.len(), 1, "only the session pass line");
    assert_eq!(second_audit[0]["target"], format!("session:{ON}"));

    // `proposal list --state rejected` shows `tag_retired` in table and JSON.
    let rejected_json: Value = serde_json::from_slice(
        &s.fx
            .ok(&["proposal", "list", "--state", "rejected", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(rejected_json["total"], 16);
    assert!(
        rejected_json["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["closed_reason"] == "tag_retired")
    );
    let rejected_table = text(
        &s.fx
            .ok(&[
                "proposal", "list", "--state", "rejected", "--format", "table",
            ])
            .stdout,
    );
    assert!(rejected_table.contains("CLOSED_REASON"), "{rejected_table}");
    assert_eq!(
        rejected_table.matches("tag_retired").count(),
        16,
        "{rejected_table}"
    );
}
