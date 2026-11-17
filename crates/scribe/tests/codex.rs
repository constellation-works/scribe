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
fn verified_skill_opt_in_capture_luna_pass_citations_off_and_provider_isolation() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    c.typed(
        "old",
        "previous",
        "#scribe decisions: historical input must not be replayed",
    );
    let on = "[$scribe:scribe](/installed/skills/scribe/SKILL.md) on";
    c.typed("on", "current", on);
    c.typed("choice", "current", "We choose compact widgets.");
    c.typed(
        "capture",
        "current",
        "#scribe decisions: We prefer compact widgets.",
    );
    c.typed("quoted","current","> #scribe decisions: quoted text\n```\n#scribe decisions: fenced text\n```\n<pasted_content>\n#scribe decisions: pasted text\n</pasted_content>");
    let mut injected = c.rows.last().unwrap().clone();
    injected["payload"]["id"] = json!("skill");
    injected["payload"]["content"] =
        json!([{"type":"input_text","text":"#scribe decisions: injected skill text"}]);
    injected["payload"]["internal_chat_message_metadata_passthrough"]["content_item_kinds"] =
        json!(["skills.selected_skill_instructions"]);
    injected["payload"]["internal_chat_message_metadata_passthrough"]["content_item_metadata"] =
        json!([{"provenance":{"type":"skills"}}]);
    c.rows.push(injected);
    c.write();
    let reply = c.hook(&f, "Stop", "current", "");
    assert!(text(&reply.stdout).contains("scribe: on"));
    let e = entries(&f);
    assert_eq!(e.len(), 1);
    assert_eq!(e[0]["provider"], "codex");
    assert_eq!(e[0]["cite"]["message"], "capture");
    let control: Value =
        serde_json::from_slice(&std::fs::read(f.root().join("control.json")).unwrap()).unwrap();
    assert_eq!(control["sessions"][ID]["provider"], "codex");
    let before = support::snapshot(&f.root());
    c.hook(&f, "Stop", "current", "");
    assert_eq!(
        support::snapshot(&f.root()),
        before,
        "duplicate delivery is a no-op"
    );
    let path = fake_model(&f);
    let out = f
        .cmd(&["session", "pass", "--session", ID, "--json"])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let record: Value =
        serde_json::from_slice(&f.ok(&["session", "show", ID, "--json"]).stdout).unwrap();
    assert_eq!(record["provider"], "codex");
    let calls = std::fs::read_to_string(f.home().join("model-calls.jsonl")).unwrap();
    let call: Value = serde_json::from_str(calls.lines().next().unwrap()).unwrap();
    let args = call["args"].as_array().unwrap();
    assert!(
        args.windows(2)
            .any(|a| a == [json!("--model"), json!("gpt-6-luna")])
    );
    assert!(
        args.windows(2)
            .any(|a| a == [json!("--disable"), json!("hooks")])
    );
    assert!(args.contains(&json!("--ephemeral")));
    assert!(call["env"].get("SCRIBE_ROOT").is_none());
    assert!(
        !PathBuf::from(call["cwd"].as_str().unwrap()).exists(),
        "isolated runtime directory removed"
    );
    let hits: Value = serde_json::from_slice(
        &f.ok(&["record", "search", "compact widgets", "--json"])
            .stdout,
    )
    .unwrap();
    assert!(!hits["hits"].as_array().unwrap().is_empty());
    c.typed("off", "next", "$scribe off");
    c.write();
    c.hook(&f, "UserPromptSubmit", "next", "$scribe off");
    let no_pass = f
        .cmd(&["session", "pass", "--json"])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(no_pass.status.success());
    assert_eq!(
        std::fs::read_to_string(f.home().join("model-calls.jsonl")).unwrap(),
        calls
    );
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
    let state: Value =
        serde_json::from_slice(&std::fs::read(f.root().join("control.json")).unwrap()).unwrap();
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
    assert!(!f.root().join("control.json").exists());
    c.typed("old", "previous", "/scribe on");
    c.typed("unrelated", "active", "ordinary typed input");
    c.write();
    c.hook(&f, "Stop", "active", "");
    assert!(
        !f.root().join("control.json").exists(),
        "never replay old opt-in"
    );
    c.typed("new-on", "active", "/scribe on");
    c.write();
    c.hook(&f, "Stop", "active", "");
    let state: Value =
        serde_json::from_slice(&std::fs::read(f.root().join("control.json")).unwrap()).unwrap();
    assert_eq!(state["sessions"][ID]["on"], true);
    c.typed("agent", "later", "#scribe decisions: agent text");
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
        assert!(!f.root().join("control.json").exists());
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
    assert!(!f.root().join("control.json").exists());
}

fn plugin_pass(f: &Fixture, path: &std::ffi::OsStr) -> Value {
    let input = json!({"schema_version":1,"tool":"scribe.session-pass","input":{},"context":{"config":{"root":f.root(),"codex_home":f.home().join(".codex"),"transcripts":""}}});
    let mut child = f
        .cmd(&["plugin", "call"])
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
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
        serde_json::from_slice::<Value>(&preview.stdout).unwrap()["daniel_turns"],
        1
    );
    assert!(!f.root().exists());
    let model = fake_model(&f);
    let never = plugin_pass(&f, &model);
    assert_eq!(never["ok"], true);
    assert!(
        !f.home().join("model-calls.jsonl").exists(),
        "never-on transcript not cataloged"
    );
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
    let passed = plugin_pass(&f, &model);
    assert_eq!(passed["ok"], true, "{passed}");
    let record: Value =
        serde_json::from_slice(&f.ok(&["session", "show", ID, "--json"]).stdout).unwrap();
    assert_eq!(record["provider"], "codex");
    assert_eq!(record["status"], "final");
    let calls = std::fs::read_to_string(f.home().join("model-calls.jsonl")).unwrap();
    let call: Value = serde_json::from_str(calls.lines().next().unwrap()).unwrap();
    assert_eq!(
        call["env"]["CODEX_HOME"],
        f.home().join(".codex").to_string_lossy().as_ref()
    );
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
    let state: Value =
        serde_json::from_slice(&std::fs::read(f.root().join("control.json")).unwrap()).unwrap();
    assert_eq!(state["sessions"][ID]["on"], false);
}

#[test]
fn codex_windows_seal_resume_and_rebuild_and_explicit_model_override() {
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
    let path = fake_model(&f);
    let run = || {
        f.cmd(&["session", "pass", "--session", ID, "--json"])
            .env("PATH", &path)
            .output()
            .unwrap()
    };
    let first = run();
    assert!(
        first.status.success(),
        "{} {}",
        text(&first.stdout),
        text(&first.stderr)
    );
    let record: Value =
        serde_json::from_slice(&f.ok(&["session", "show", ID, "--log", "--json"]).stdout).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 3);
    assert_eq!(record["log"][0]["state"], "sealed");
    let calls = std::fs::read_to_string(f.home().join("model-calls.jsonl")).unwrap();
    assert_eq!(calls.lines().count(), 3);
    let call: Value = serde_json::from_str(calls.lines().next().unwrap()).unwrap();
    assert!(
        call["args"]
            .as_array()
            .unwrap()
            .contains(&json!("explicit-luna-id"))
    );
    let again = run();
    assert!(again.status.success());
    assert_eq!(
        std::fs::read_to_string(f.home().join("model-calls.jsonl")).unwrap(),
        calls
    );
    let rebuilt = f
        .cmd(&[
            "session",
            "rebuild",
            ID,
            "--author",
            "sol",
            "--confirm",
            "--json",
        ])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        rebuilt.status.success(),
        "{} {}",
        text(&rebuilt.stdout),
        text(&rebuilt.stderr)
    );
    let record: Value =
        serde_json::from_slice(&f.ok(&["session", "show", ID, "--json"]).stdout).unwrap();
    assert_eq!(record["provider"], "codex");
    assert_eq!(record["rebuilt_by"]["name"], "sol");
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
    assert!(!f.root().join("control.json").exists());
}

#[test]
fn whitespace_preserves_original_prompt_hash_lines_and_message_citation() {
    let f = Fixture::new();
    let mut c = Codex::new(&f);
    let prompt = "\n#scribe decisions: Preserve my exact citation.\n";
    c.typed("whitespace", "t", prompt);
    c.write();
    c.hook(&f, "Stop", "t", "");
    let all = entries(&f);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["cite"]["message"], "whitespace");
    assert_eq!(all[0]["cite"]["line"], 2);
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
    let state: Value =
        serde_json::from_slice(&std::fs::read(f.root().join("control.json")).unwrap()).unwrap();
    assert_eq!(state["sessions"][ID]["provider"], "codex");
    assert_eq!(state["sessions"][ID]["cataloged_by"]["by"]["kind"], "agent");
}
