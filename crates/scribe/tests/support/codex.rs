use crate::support;
use crate::support::Fixture;
use crate::support::command::Exit::{Capture, Success};
use crate::support::command::Input::Cli;
use crate::support::command::Json;
use crate::support::command::with_env;
use crate::support::command::{Expectation, Input};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Output};

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["--provider", "codex"], expect)
    }
}

pub(super) const ID: &str = "11111111-2222-4333-8444-555555555555";

pub(super) struct Codex {
    pub(super) path: PathBuf,
    pub(super) rows: Vec<Value>,
}

impl Codex {
    pub(super) fn new(f: &Fixture) -> Self {
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
    pub(super) fn typed(&mut self, id: &str, turn: &str, body: &str) {
        self.rows.push(json!({"type":"response_item","timestamp":"2026-11-12T00:00:00Z","payload":{"type":"message","id":id,"role":"user","content":[{"type":"input_text","text":body}],"internal_chat_message_metadata_passthrough":{"turn_id":turn,"content_item_kinds":["user.text"],"content_item_metadata":[{"provenance":{"type":"user"}}]}}}));
    }
    pub(super) fn write(&self) {
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
    pub(super) fn payload(&self, event: &str, turn: &str, prompt: &str) -> Value {
        json!({"hook_event_name":event,"session_id":ID,"transcript_path":self.path,"turn_id":turn,"cwd":"/synthetic/project","prompt":prompt})
    }
    pub(super) fn hook(&self, f: &Fixture, event: &str, turn: &str, prompt: &str) -> Output {
        let verb = match event {
            "UserPromptSubmit" => "user-prompt-submit",
            "Stop" => "stop",
            "SessionEnd" => "session-end",
            _ => panic!("fixture event"),
        };
        f.check(
            with_env(
                &["--provider", "codex", "hook", verb],
                "GITHUB_TOKEN",
                "synthetic-fixture-token-value",
            ),
            Success.stdin(self.payload(event, turn, prompt).to_string().as_bytes()),
        )
    }
}

pub(super) fn entries(f: &Fixture) -> Vec<Value> {
    f.check(Cli(&["entry", "list", "--json"]), Json)["entries"]
        .as_array()
        .unwrap()
        .clone()
}

pub(super) fn fake_model(f: &Fixture) -> std::ffi::OsString {
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

pub(super) fn widget_answer(_: &Value, _: bool) -> Result<Value, support::pass::ActivityFailure> {
    Ok(json!({
        "title":"Compact widgets", "summary":"Chose compact widgets.", "goals":"Choose widgets",
        "next":[], "topics":["widgets"], "related":[], "terms":[],
        "open":{"heading":"Widget choice","text":"Chose compact widgets.","topics":["widgets"],"related":[]},
        "new":[]
    }))
}

pub(super) fn cli_pass(f: &Fixture) -> Value {
    support::pass::run(
        |step, input| support::pass::cli_step(f, &[], step, input),
        json!({"sessions": [ID]}),
        widget_answer,
    )
    .unwrap()
}

pub(super) fn plugin_pass(f: &Fixture) -> Value {
    let config =
        json!({"root": f.root(), "codex_home": f.home().join(".codex"), "transcripts": ""});
    support::pass::run(
        |step, input| support::pass::plugin_step(f, &[], step, input, &config),
        json!({}),
        widget_answer,
    )
    .unwrap()
}

pub(super) const MARKED: &str = "I always want short status updates #scribe preferences";

impl Codex {
    /// [`Codex::hook`] with `PATH` set to `path`, where a fake `codex`
    /// logs any model call.
    pub(super) fn hook_with(
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
        f.check(
            f.cmd(&["--provider", "codex", "hook", verb])
                .env("GITHUB_TOKEN", "synthetic-fixture-token-value")
                .env("PATH", path),
            Success.stdin(self.payload(event, turn, prompt).to_string().as_bytes()),
        )
    }

    /// An assistant reply in the rollout.
    pub(super) fn reply(&mut self, id: &str, body: &str) {
        self.rows.push(json!({"type":"response_item","timestamp":"2026-11-12T00:00:01Z","payload":{"type":"message","id":id,"role":"assistant","content":[{"type":"output_text","text":body}]}}));
    }
}

/// A fake `codex` whose every call fails, logging each call.
pub(super) fn failing_codex(f: &Fixture) -> std::ffi::OsString {
    f.fake_program(
        "codex",
        "#!/bin/sh\necho '{}' >> \"$HOME/model-calls.jsonl\"\ncat >/dev/null\nexit 1\n",
    )
}

pub(super) fn model_calls(f: &Fixture) -> usize {
    std::fs::read_to_string(f.home().join("model-calls.jsonl"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

pub(super) fn marks_json(f: &Fixture) -> Vec<Value> {
    match support::exported(&f.root(), "marks.json") {
        Some(doc) => serde_json::from_str::<Value>(&doc).unwrap()["marks"]
            .as_array()
            .unwrap()
            .clone(),
        None => Vec::new(),
    }
}

pub(super) fn hook_reply(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

pub(super) fn packaged_stop(f: &Fixture, payload: &Value) -> Output {
    let bin = f.home().join(".cargo/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(support::BIN, bin.join("scribe")).unwrap();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin/codex/bin/scribe-hook");
    f.check(
        std::process::Command::new("/bin/sh")
            .arg(script)
            .arg("stop")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", f.home())
            .env("SCRIBE_ROOT", f.root()),
        Capture.stdin(payload.to_string().as_bytes()),
    )
}
