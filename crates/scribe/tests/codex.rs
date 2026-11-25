//! Binary-level Codex integration using synthetic, provenance-tagged rollouts.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    process::{Output, Stdio},
};
use support::{Fixture, text};

const ID: &str = "11111111-2222-4333-8444-555555555555";

struct Codex {
    path: PathBuf,
    rows: Vec<Value>,
}
impl Codex {
    fn new(f: &Fixture) -> Self {
        let dir = f.home().join(".codex/sessions/2026/11/12");
        f.create_private_dir_all(&dir);
        let path = dir.join(format!("rollout-2026-11-12T00-00-00-{ID}.jsonl"));
        Self {
            path,
            rows: vec![
                json!({"type":"session_meta","payload":{"id":ID,"source":"vscode","thread_source":"user","cwd":f.home()}}),
            ],
        }
    }
    fn typed(&mut self, id: &str, turn: &str, body: &str) {
        self.rows.push(json!({"type":"response_item","timestamp":"2026-11-12T00:00:00Z","payload":{"type":"message","id":id,"role":"user","content":[{"type":"input_text","text":body}],"internal_chat_message_metadata_passthrough":{"turn_id":turn,"content_item_kinds":["user.text"],"content_item_metadata":[{"provenance":{"type":"user"}}]}}}));
    }
    fn write(&self) {
        std::fs::write(
            &self.path,
            self.rows
                .iter()
                .map(|r| format!("{r}\n"))
                .collect::<String>(),
        )
        .unwrap();
        support::make_private(&self.path);
    }
    fn payload(&self, event: &str, turn: &str, prompt: &str) -> Value {
        json!({"hook_event_name":event,"session_id":ID,"transcript_path":self.path,"turn_id":turn,"cwd":"/synthetic/project","prompt":prompt})
    }
    fn hook(&self, f: &Fixture, event: &str, turn: &str, prompt: &str) -> Output {
        let verb = match event {
            "UserPromptSubmit" => "user-prompt-submit",
            "Stop" => "stop",
            "SessionEnd" => "session-end",
            _ => panic!("fixture event"),
        };
        let mut c = f
            .cmd(&["--provider", "codex", "hook", verb])
            .env("GITHUB_TOKEN", "synthetic-fixture-token-value")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin
            .take()
            .unwrap()
            .write_all(self.payload(event, turn, prompt).to_string().as_bytes())
            .unwrap();
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success(), "{}", text(&o.stderr));
        o
    }
}

fn entries(f: &Fixture) -> Vec<Value> {
    serde_json::from_slice::<Value>(&f.ok(&["entry", "list", "--json"]).stdout).unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

fn fake_model(f: &Fixture) -> std::ffi::OsString {
    f.fake_program("codex", r##"#!/usr/bin/python3
import json,sys,pathlib,os
h=pathlib.Path(os.environ['HOME'])
args=sys.argv[1:]
with (h/'model-calls.jsonl').open('a') as out: out.write(json.dumps({'args':args,'env':dict(os.environ),'cwd':os.getcwd()})+'\n')
schema=json.loads(pathlib.Path(args[args.index('--output-schema')+1]).read_text())
assert 'open' in schema.get('required',[]) or 'open' not in schema.get('properties',{})
sys.stdin.read()
answer={'title':'Compact widgets','summary':'Chose compact widgets.','goals':'Choose widgets','next':[],'topics':['widgets'],'related':[],'terms':[],'open':{'heading':'Widget choice','text':'Chose compact widgets.','topics':['widgets'],'related':[]},'new':[]}
print(json.dumps({'type':'thread.started','thread_id':'synthetic'}))
print(json.dumps({'type':'item.completed','item':{'type':'agent_message','text':json.dumps(answer)}}))
print(json.dumps({'type':'turn.completed','usage':{}}))
"##)
}

#[test]
fn verified_skill_opt_in_capture_pass_citations_off_and_provider_isolation() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    // A settling command typed before this turn is history: it is not
    // replayed when scribe turns on.
    c.typed("old", "previous", "#scribe reject p-9");
    let on = "[$scribe:scribe](/installed/skills/scribe/SKILL.md) on";
    c.typed("on", "current", on);
    c.typed("choice", "current", "We choose compact widgets.");
    c.typed(
        "capture",
        "current",
        "#scribe preferences: We prefer compact widgets.",
    );
    c.typed("quoted","current","> #scribe preferences: quoted text\n```\n#scribe preferences: fenced text\n```\n<pasted_content>\n#scribe preferences: pasted text\n</pasted_content>");
    let mut injected = c.rows.last().unwrap().clone();
    injected["payload"]["id"] = json!("skill");
    injected["payload"]["content"] =
        json!([{"type":"input_text","text":"#scribe preferences: injected skill text"}]);
    injected["payload"]["internal_chat_message_metadata_passthrough"]["content_item_kinds"] =
        json!(["skills.selected_skill_instructions"]);
    injected["payload"]["internal_chat_message_metadata_passthrough"]["content_item_metadata"] =
        json!([{"provenance":{"type":"skills"}}]);
    c.rows.push(injected);
    c.write();
    // The typed `#scribe preferences:` line folds into the annotation marker
    // (2_design.md §14.6) and Codex's Stop hands it to the agent, with its
    // tag as the hint; the quoted, fenced, pasted and injected ones are not
    // markers, so the agent gets one marker and only one mark is written.
    let reply = c.hook(&f, "Stop", "current", "");
    let out = text(&reply.stdout);
    assert!(out.contains("scribe: on"), "{out}");
    assert!(
        out.contains(
            "scribe: #scribe in your message is the agent's to record — \\\"#scribe preferences: We prefer compact widgets.\\\""
        ),
        "{out}"
    );
    assert!(out.contains("scribe entry add --tag preferences "), "{out}");
    for excluded in [
        "historical",
        "quoted text",
        "fenced text",
        "pasted text",
        "injected skill",
    ] {
        assert!(
            !out.contains(excluded),
            "{excluded} reached the agent: {out}"
        );
    }
    assert!(entries(&f).is_empty());
    let marks = marks_json(&f);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0]["message"], "capture");
    assert_eq!(marks[0]["code"], "agent_handoff");
    let control: Value = support::control(&f.root());
    assert_eq!(control["sessions"][ID]["provider"], "codex");
    let before = support::snapshot(&f.root());
    c.hook(&f, "Stop", "current", "");
    assert_eq!(
        support::snapshot(&f.root()),
        before,
        "duplicate delivery is a no-op"
    );
    let passed = cli_pass(&f);
    assert_eq!(passed["passes"][0]["result"], "written");
    let record: Value =
        serde_json::from_slice(&f.ok(&["session", "show", ID, "--json"]).stdout).unwrap();
    assert_eq!(record["provider"], "codex");
    assert_eq!(model_calls(&f), 0);
    let hits: Value =
        serde_json::from_slice(&f.ok(&["search", "compact widgets", "--json"]).stdout).unwrap();
    assert!(!hits["hits"].as_array().unwrap().is_empty());
    c.typed("off", "next", "$scribe off");
    c.write();
    c.hook(&f, "UserPromptSubmit", "next", "$scribe off");
    let no_pass = f.ok(&["session", "pass", "--json"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&no_pass.stdout).unwrap()["passes"],
        json!([])
    );
    assert_eq!(model_calls(&f), 0);
    // The default Claude session remains independent even in the same store.
    let claude = f
        .home()
        .join(".claude/projects/synthetic/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl");
    f.create_private_dir_all(claude.parent().unwrap());
    let mut cmd = f
        .cmd(&["hook", "user-prompt-submit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    cmd.stdin.take().unwrap().write_all(json!({"session_id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","transcript_path":claude,"cwd":"/synthetic","prompt":"/scribe on"}).to_string().as_bytes()).unwrap();
    assert!(cmd.wait_with_output().unwrap().status.success());
    let state: Value = support::control(&f.root());
    assert_eq!(state["sessions"][ID]["on"], false);
    assert_eq!(
        state["sessions"]["aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"]["on"],
        true
    );
}

#[test]
fn deferred_prompt_requires_host_metadata_and_only_processes_current_turn() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    c.write();
    let out = c.hook(&f, "UserPromptSubmit", "active", "/scribe on");
    assert!(text(&out.stdout).contains("not yet verified"));
    assert!(!support::has_control(&f.root()));
    c.typed("old", "previous", "/scribe on");
    c.typed("unrelated", "active", "ordinary typed input");
    c.write();
    c.hook(&f, "Stop", "active", "");
    assert!(!support::has_control(&f.root()), "never replay old opt-in");
    c.typed("new-on", "active", "/scribe on");
    c.write();
    c.hook(&f, "Stop", "active", "");
    let state: Value = support::control(&f.root());
    assert_eq!(state["sessions"][ID]["on"], true);
    c.typed("agent", "later", "#scribe preferences: agent text");
    c.rows.last_mut().unwrap()["payload"]
        .as_object_mut()
        .unwrap()
        .remove("internal_chat_message_metadata_passthrough");
    c.write();
    c.hook(&f, "Stop", "later", "");
    assert!(entries(&f).is_empty());
}

#[test]
fn subagents_unsafe_paths_and_missing_identity_never_record() {
    for source in [json!({"subagent":{"spawn":{}}}), json!("exec")] {
        let f = Fixture::new();
        let mut c = Codex::new(&f);
        c.rows[0]["payload"]["source"] = source;
        c.typed("input", "t", "/scribe on");
        c.write();
        let out = c.hook(&f, "Stop", "t", "");
        assert!(text(&out.stdout).contains("not recorded"));
        assert!(!support::has_control(&f.root()));
    }
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    c.typed("input", "t", "/scribe on");
    c.write();
    let original = c.path.clone();
    let outside = f.elsewhere("outside.jsonl");
    std::fs::rename(&original, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &original).unwrap();
    let out = c.hook(&f, "Stop", "t", "");
    assert!(text(&out.stdout).contains("not recorded"));
    assert!(!support::has_control(&f.root()));
}

fn widget_answer(_: &Value, _: bool) -> Result<Value, support::pass::ActivityFailure> {
    Ok(json!({
        "title":"Compact widgets", "summary":"Chose compact widgets.", "goals":"Choose widgets",
        "next":[], "topics":["widgets"], "related":[], "terms":[],
        "open":{"heading":"Widget choice","text":"Chose compact widgets.","topics":["widgets"],"related":[]},
        "new":[]
    }))
}

fn cli_pass(f: &Fixture) -> Value {
    support::pass::run(
        |step, input| support::pass::cli_step(f, &[], step, input),
        json!({"sessions": [ID]}),
        widget_answer,
    )
    .unwrap()
}

fn plugin_pass(f: &Fixture) -> Value {
    let config =
        json!({"root": f.root(), "codex_home": f.home().join(".codex"), "transcripts": ""});
    support::pass::run(
        |step, input| support::pass::plugin_step(f, &[], step, input, &config),
        json!({}),
        widget_answer,
    )
    .unwrap()
}

#[test]
fn discovery_is_read_only_and_archive_final_pass_uses_optional_plugin_codex_root() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    c.typed("words", "t", "Choose compact widgets.");
    c.write();
    let out = f.ok(&[
        "--provider",
        "codex",
        "transcript",
        "list",
        "--since",
        "2026-01-01",
        "--min-turns",
        "1",
        "--json",
    ]);
    let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed["total"], 1);
    assert!(!f.root().exists(), "discovery must not opt in or write");
    let preview = f.ok(&["--provider", "codex", "transcript", "show", ID, "--json"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&preview.stdout).unwrap()["user_turns"],
        1
    );
    assert!(!f.root().exists());
    assert_eq!(plugin_pass(&f)["passes"], json!([]));
    assert_eq!(model_calls(&f), 0);
    c.typed("on", "live", "/scribe on");
    c.typed("choice", "live", "We choose compact widgets.");
    c.write();
    c.hook(&f, "Stop", "live", "");
    let archive = f.home().join(".codex/archived_sessions");
    f.create_private_dir_all(&archive);
    let moved = archive.join(c.path.file_name().unwrap());
    std::fs::rename(&c.path, &moved).unwrap();
    c.path = moved;
    c.hook(&f, "SessionEnd", "live", "");
    assert_eq!(plugin_pass(&f)["passes"][0]["result"], "written");
    let record: Value =
        serde_json::from_slice(&f.ok(&["session", "show", ID, "--json"]).stdout).unwrap();
    assert_eq!(record["provider"], "codex");
    assert_eq!(record["status"], "final");
    assert_eq!(model_calls(&f), 0);
}

#[test]
fn controls_links_settlements_and_invalid_instructions_do_not_block_later_off() {
    let f = Fixture::new();
    f.seed();
    let mut c = Codex::new(&f);
    c.typed("old-settle", "old", "#scribe forget decisions-2");
    c.typed(
        "on",
        "active",
        "/scribe on --from 0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01",
    );
    c.write();
    c.hook(&f, "Stop", "active", "");
    let old: Value =
        serde_json::from_slice(&f.ok(&["entry", "show", "decisions-2", "--json"]).stdout).unwrap();
    assert!(old["content"].as_str().unwrap().contains("squash"));
    c.typed("status", "status-turn", "$scribe status");
    c.write();
    let status = c.hook(&f, "UserPromptSubmit", "status-turn", "$scribe status");
    assert!(text(&status.stdout).contains("linked:"));
    c.typed("retire", "settle", "#scribe retire decisions-2");
    c.write();
    c.hook(&f, "Stop", "settle", "");
    let entry: Value =
        serde_json::from_slice(&f.ok(&["entry", "show", "decisions-2", "--json"]).stdout).unwrap();
    assert_eq!(entry["status"], "retired");
    c.typed("forget", "forget-turn", "#scribe forget decisions-2");
    c.write();
    c.hook(&f, "Stop", "forget-turn", "");
    let entry: Value =
        serde_json::from_slice(&f.ok(&["entry", "show", "decisions-2", "--json"]).stdout).unwrap();
    assert_eq!(entry["content"], "");
    c.typed("bad", "last", "#scribe unknown-tag: refused instruction");
    c.typed("off", "last", "/scribe off");
    c.write();
    c.hook(&f, "Stop", "last", "");
    let state: Value = support::control(&f.root());
    assert_eq!(state["sessions"][ID]["on"], false);
}

#[test]
fn codex_windows_seal_resume_and_rebuild() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    c.typed("on", "t", "/scribe on");
    for n in 0..6 {
        c.typed(&format!("u-{n}"), "t", "We choose compact widgets.");
    }
    c.write();
    c.hook(&f, "Stop", "t", "");
    let config = f.root().join("config.toml");
    std::fs::write(&config,"[catalog]\nwindow_messages=2\nsegment_windows=1\n[codex.models]\nsession='explicit-luna-id'\n").unwrap();
    support::make_private(&config);
    let first = cli_pass(&f);
    assert_eq!(first["passes"][0]["result"], "written");
    assert_eq!(first["passes"][0]["windows"], 3);
    let record: Value =
        serde_json::from_slice(&f.ok(&["session", "show", ID, "--log", "--json"]).stdout).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 3);
    assert_eq!(record["log"][0]["state"], "sealed");
    let again = cli_pass(&f);
    assert_eq!(again["passes"][0]["result"], "unchanged");
    assert_eq!(model_calls(&f), 0);
    // The rebuild runs as the scribe-session-rebuild job: its plugin steps
    // read the Codex rollout, and fixed answers stand in for the activity.
    let config =
        json!({"root": f.root(), "codex_home": f.home().join(".codex"), "transcripts": ""});
    let rebuilt = support::pass::rebuild(
        |step, input| {
            support::pass::plugin_tool(&f, &[], &format!("session-{step}"), input, &config)
        },
        &json!({"session": ID, "author": "sol", "confirm": true}),
        widget_answer,
    );
    assert!(
        rebuilt.status.success(),
        "{} {}",
        text(&rebuilt.stdout),
        text(&rebuilt.stderr)
    );
    let report: Value = serde_json::from_slice(&rebuilt.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 3, "{report}");
    let record: Value =
        serde_json::from_slice(&f.ok(&["session", "show", ID, "--json"]).stdout).unwrap();
    assert_eq!(record["provider"], "codex");
    assert_eq!(record["rebuilt_by"]["name"], "sol");
    assert_eq!(model_calls(&f), 0, "no flow spawns codex");
}

#[test]
fn unsupported_transcripts_return_nonblocking_refusal_for_ordinary_prompts() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    c.typed("text", "t", "Help me with ordinary work");
    c.rows[0]["payload"]
        .as_object_mut()
        .unwrap()
        .remove("thread_source");
    c.write();
    let reply = c.hook(&f, "UserPromptSubmit", "t", "Help me with ordinary work");
    assert_eq!(reply.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&reply.stdout).unwrap();
    assert!(value.get("decision").is_none());
    assert!(!support::has_control(&f.root()));
}

#[test]
fn whitespace_preserves_original_prompt_hash_lines_and_message_citation() {
    let f = Fixture::new();
    f.seed();
    let before = entries(&f);
    let mut c = Codex::new(&f);
    // A line-start capture form folds into the annotation marker
    // (2_design.md §14.6): Codex's Stop hands it to the agent like any
    // marker, and writes no entry.
    let capture = "\n#scribe preferences: Preserve my exact citation.\n";
    c.typed("capture", "t", capture);
    c.write();
    let out = c.hook(&f, "Stop", "t", "");
    assert!(
        text(&out.stdout).contains("is the agent's to record"),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(entries(&f), before);
    let marks = marks_json(&f);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0]["code"], "agent_handoff");
    // A settling line keeps the citation: the hash of the prompt as typed,
    // leading newline included, and the line's number within it.
    let prompt = "\n#scribe reject p-1\n";
    c.typed("whitespace", "t2", prompt);
    c.write();
    let out = c.hook(&f, "Stop", "t2", "");
    assert!(
        text(&out.stdout).contains("scribe: rejected p-1"),
        "{}",
        text(&out.stdout)
    );
    let listed: Value =
        serde_json::from_slice(&f.ok(&["proposal", "list", "--json"]).stdout).unwrap();
    let p1 = listed["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "p-1")
        .unwrap();
    assert_eq!(p1["state"], "rejected");
    let cite = &p1["settled_by"];
    assert_eq!(cite["session"], ID);
    assert_eq!(
        cite["prompt_sha256"],
        scribe_core::capture::prompt_sha256(prompt)
    );
    assert_ne!(
        cite["prompt_sha256"],
        scribe_core::capture::prompt_sha256(prompt.trim())
    );
    assert_eq!(cite["line"], 2);
}

#[test]
fn packaged_launcher_finds_user_binary_without_cargo_on_gui_path() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    c.typed("on", "t", "$scribe on");
    c.write();
    let bin = f.home().join(".cargo/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(support::BIN, bin.join("scribe")).unwrap();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin/codex/bin/scribe-hook");
    let mut child = std::process::Command::new("/bin/sh")
        .arg(script)
        .arg("stop")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", f.home())
        .env("SCRIBE_ROOT", f.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(c.payload("Stop", "t", "").to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("scribe: on"));
}

#[test]
fn explicit_codex_catalog_stamps_provider_with_agent_authorship() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    c.typed("choice", "t", "We choose compact widgets.");
    c.write();
    let path = fake_model(&f);
    let out = f
        .cmd(&[
            "--provider",
            "codex",
            "session",
            "catalog",
            ID,
            "--author",
            "sol",
            "--json",
        ])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let state: Value = support::control(&f.root());
    assert_eq!(state["sessions"][ID]["provider"], "codex");
    assert_eq!(state["sessions"][ID]["cataloged_by"]["by"]["kind"], "agent");
}

// --- annotations: `#scribe` inside a sentence, handed over at Stop (§14) ---

const MARKED: &str = "I always want short status updates #scribe preferences";

impl Codex {
    /// [`Codex::hook`] with `PATH` set to `path`, where a fake `codex`
    /// logs any model call.
    fn hook_with(
        &self,
        f: &Fixture,
        event: &str,
        turn: &str,
        prompt: &str,
        path: &std::ffi::OsStr,
    ) -> Output {
        let verb = match event {
            "UserPromptSubmit" => "user-prompt-submit",
            "Stop" => "stop",
            _ => panic!("fixture event"),
        };
        let mut c = f
            .cmd(&["--provider", "codex", "hook", verb])
            .env("GITHUB_TOKEN", "synthetic-fixture-token-value")
            .env("PATH", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin
            .take()
            .unwrap()
            .write_all(self.payload(event, turn, prompt).to_string().as_bytes())
            .unwrap();
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success(), "{}", text(&o.stderr));
        o
    }

    /// An assistant reply in the rollout.
    fn reply(&mut self, id: &str, body: &str) {
        self.rows.push(json!({"type":"response_item","timestamp":"2026-11-12T00:00:01Z","payload":{"type":"message","id":id,"role":"assistant","content":[{"type":"output_text","text":body}]}}));
    }
}

/// A fake `codex` whose every call fails, logging each call.
fn failing_codex(f: &Fixture) -> std::ffi::OsString {
    f.fake_program(
        "codex",
        "#!/bin/sh\necho '{}' >> \"$HOME/model-calls.jsonl\"\ncat >/dev/null\nexit 1\n",
    )
}

fn model_calls(f: &Fixture) -> usize {
    std::fs::read_to_string(f.home().join("model-calls.jsonl"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn marks_json(f: &Fixture) -> Vec<Value> {
    match support::exported(&f.root(), "marks.json") {
        Some(doc) => serde_json::from_str::<Value>(&doc).unwrap()["marks"]
            .as_array()
            .unwrap()
            .clone(),
        None => Vec::new(),
    }
}

fn hook_reply(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

#[test]
fn a_codex_marker_is_echoed_once_verified_and_handed_to_the_agent_at_stop() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    c.write();
    let path = failing_codex(&f);
    // Before the rollout holds the turn: not verified, Stop retries.
    let early = c.hook_with(&f, "UserPromptSubmit", "t-1", MARKED, &path);
    assert!(
        text(&early.stdout).contains("not yet verified"),
        "{}",
        text(&early.stdout)
    );
    c.typed("m-1", "t-1", MARKED);
    c.write();
    let submitted = hook_reply(&c.hook_with(&f, "UserPromptSubmit", "t-1", MARKED, &path));
    assert_eq!(
        submitted["systemMessage"], "scribe: marked — cataloged after the reply",
        "{submitted}"
    );
    let context = submitted["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains(
            "hands this #scribe marker to you with the command that records it; record nothing for it before then"
        ),
        "{context}"
    );
    assert_eq!(model_calls(&f), 0, "no model call at submit");
    assert!(marks_json(&f).is_empty(), "nothing written at submit");

    c.reply("a-1", "Noted.");
    c.write();
    let stopped = hook_reply(&c.hook_with(&f, "Stop", "t-1", "", &path));
    assert_eq!(
        stopped["systemMessage"],
        format!("scribe: #scribe in your message is the agent's to record — \"{MARKED}\""),
        "{stopped}"
    );
    let context = stopped["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains(&format!(
            "scribe entry add --tag preferences --project home --author <name> --related session:{ID}"
        )),
        "{context}"
    );
    assert_eq!(model_calls(&f), 0, "no model call at Stop");
    assert!(entries(&f).is_empty());
    let marks = marks_json(&f);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0]["outcome"], "refused");
    assert_eq!(marks[0]["code"], "agent_handoff");

    // The next Stop neither hands over nor writes.
    let before = support::snapshot(&f.root());
    let again = c.hook_with(&f, "Stop", "t-1", "", &path);
    assert!(again.stdout.is_empty(), "{}", text(&again.stdout));
    assert_eq!(support::snapshot(&f.root()), before);
}

#[test]
fn a_codex_mention_in_quotes_or_code_is_neither_echoed_nor_cataloged() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    let mention = "the `#scribe` marker and \"#scribe\" in quotes";
    c.typed("m-1", "t-1", mention);
    c.write();
    let path = failing_codex(&f);
    let submitted = c.hook_with(&f, "UserPromptSubmit", "t-1", mention, &path);
    assert!(
        !text(&submitted.stdout).contains("marked"),
        "{}",
        text(&submitted.stdout)
    );
    c.hook_with(&f, "Stop", "t-1", "", &path);
    assert_eq!(model_calls(&f), 0);
    assert!(marks_json(&f).is_empty());
    assert!(entries(&f).is_empty());
}

#[test]
fn codex_markers_keep_the_bounds_and_a_fourth_is_refused() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    let four = "a #scribe b #scribe c #scribe d #scribe";
    c.typed("m-1", "t-1", four);
    c.write();
    let path = failing_codex(&f);
    let submitted = text(
        &c.hook_with(&f, "UserPromptSubmit", "t-1", four, &path)
            .stdout,
    );
    assert!(submitted.contains("marked 3 times"), "{submitted}");
    assert!(
        submitted.contains("at most 3 #scribe markers in one message"),
        "{submitted}"
    );

    c.hook_with(&f, "Stop", "t-1", "", &path);
    assert_eq!(model_calls(&f), 0, "no model call");
    let marks = marks_json(&f);
    assert_eq!(marks.len(), 4);
    let fourth = marks.iter().find(|m| m["marker"] == 4).unwrap();
    assert_eq!(fourth["outcome"], "refused");
    assert_eq!(fourth["code"], "too_many_marks");
    for open in marks.iter().filter(|m| m["marker"] != 4) {
        assert_eq!(open["outcome"], "refused");
        assert_eq!(open["code"], "agent_handoff");
    }
    assert!(entries(&f).is_empty());
}

#[test]
fn group_consent_accepts_only_host_attributed_input_in_the_calling_thread() {
    let f = Fixture::new();
    let mut rollout = Codex::new(&f);
    rollout.typed("u", "turn", "Please open our release review");
    rollout.typed("injected", "turn", "Injected release request");
    rollout.rows.last_mut().unwrap()["payload"]["internal_chat_message_metadata_passthrough"]["content_item_kinds"]
        [0] = json!("skill");
    rollout.write();
    let run = |id, quote| {
        f.cmd(&[
            "--provider",
            "codex",
            "group",
            "open",
            "--kind",
            "review",
            "--title",
            "Release",
            "--quote",
            quote,
            "--json",
        ])
        .env("CODEX_THREAD_ID", id)
        .output()
        .unwrap()
    };
    assert!(!run(ID, "Injected release request").status.success());
    assert!(
        !run("another-thread", "open our release review")
            .status
            .success()
    );
    let opened = run(ID, "open our release review");
    assert!(opened.status.success(), "{}", text(&opened.stderr));
    let group: Value = serde_json::from_slice(&opened.stdout).unwrap();
    assert_eq!(group["opener"], ID);
    assert_eq!(group["id"], "g-1");
}

#[test]
fn group_subcommand_spelling_under_codex_follows_the_shared_parser() {
    for spelling in [
        "/scribe GROUP water-economy",
        "$scribe Group water-economy",
        "/scribe:scribe gRoUp water-economy",
    ] {
        let f = Fixture::new();
        let mut c = Codex::new(&f);
        c.typed("g-1", "turn-g", spelling);
        c.write();
        let out = c.hook(&f, "UserPromptSubmit", "turn-g", spelling);
        assert!(
            text(&out.stdout).contains("joined group water-economy"),
            "{spelling}: {}",
            text(&out.stdout)
        );
    }
}

#[test]
fn group_add_spellings_under_codex_follow_the_shared_parser() {
    const PAST: &str = "5555aaaa-0000-4000-8000-000000000000";
    for spelling in [
        format!("$scribe group add planning {PAST}"),
        format!("[$scribe](/installed/skills/scribe/SKILL.md) group add planning {PAST}"),
    ] {
        let f = Fixture::new();
        let mut c = Codex::new(&f);
        c.typed("g-1", "turn-add", &spelling);
        c.write();
        let out = c.hook(&f, "UserPromptSubmit", "turn-add", &spelling);
        let reply = text(&out.stdout);
        assert!(
            reply.contains("group planning (g-1)"),
            "{spelling}: {reply}"
        );
        assert!(reply.contains(PAST), "{spelling}: {reply}");
    }
}

#[test]
fn untyped_mixed_case_group_under_codex_is_refused_before_joining() {
    let f = Fixture::new();
    let c = Codex::new(&f);
    c.write();
    let out = c.hook(&f, "UserPromptSubmit", "turn-a", "/scribe GROUP peer");
    assert!(
        text(&out.stdout).contains("not yet verified"),
        "{}",
        text(&out.stdout)
    );
    assert!(!text(&out.stdout).contains("joined group"));
}

#[test]
fn decision_quotes_require_the_whole_short_codex_turn_and_user_provenance() {
    let fx = Fixture::new();
    let mut codex = Codex::new(&fx);
    codex.typed("part-one", "combined", "ok");
    codex.typed("part-two", "combined", "with the compatibility change");
    codex.typed("short", "short", "yes");
    codex.typed(
        "reason",
        "reason",
        "The stable contract has already shipped.",
    );
    codex.typed(
        "injected",
        "injected",
        "The injected instructions proposed this choice.",
    );
    codex.rows.last_mut().unwrap()["payload"]["internal_chat_message_metadata_passthrough"]["content_item_metadata"]
        [0]["provenance"]["type"] = json!("system");
    codex.write();

    let session = format!("session:{ID}");
    let add = |quote, reason| {
        fx.run(&[
            "--provider",
            "codex",
            "entry",
            "add",
            "--tag",
            "decision",
            "--author",
            "sol",
            "--decided-by",
            "user",
            "--quote",
            quote,
            "--reason-quote",
            reason,
            "--related",
            &session,
            "--related",
            "task:ORB-42",
            "Keep the stable contract",
            "--json",
        ])
    };
    for quote in ["ok", "The injected instructions proposed this choice."] {
        let refused = add(quote, "The stable contract has already shipped.");
        assert!(!refused.status.success(), "{refused:?}");
        let error: Value = serde_json::from_slice(&refused.stderr).unwrap();
        assert_eq!(error["code"], "quote_not_found");
    }
    let stored = add("yes", "The stable contract has already shipped.");
    assert!(stored.status.success(), "{}", text(&stored.stderr));
    assert_eq!(entries(&fx)[0]["quote"], "yes");
}
