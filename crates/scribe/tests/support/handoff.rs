use crate::support::command::Exit::{Capture, Success};
use crate::support::command::Input::Cli;
use crate::support::command::with_env;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, make_private, text};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output};

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["entry"], expect)
    }
}

pub(super) const OFF_SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";

pub(super) const ON_SESSION: &str = "6e2d1c8f-3a5b-4d40-9f72-1b0c9d8e7f60";

pub(super) fn valid_handoff(state: &str, next: &str) -> String {
    format!(
        "\
## State
{state}

## In flight
Uncommitted edits in `crates/scribe-core/src/handoff.rs`.

## Next
{next}

## Waiting on you
Confirm whether we cut the release tag today.

## Do not touch
Leave `crates/scribe/tests/goldens/` untouched until the contract review passes.

## Pointers
- `crates/scribe-core/src/handoff.rs`
- `docs/design/scribe/2_design.md`
"
    )
}

/// A Claude Code transcript layout under the fixture's home.
pub(super) struct Transcript {
    pub(super) session: String,
    pub(super) path: PathBuf,
    pub(super) cwd: PathBuf,
}

impl Transcript {
    pub(super) fn new(fx: &Fixture, session: &str) -> Self {
        let project = fx.home().join(".claude/projects/-work-scribe");
        std::fs::create_dir_all(&project).unwrap();
        let cwd = fx.elsewhere("work/scribe");
        std::fs::create_dir_all(&cwd).unwrap();
        Self {
            session: session.to_owned(),
            path: project.join(format!("{session}.jsonl")),
            cwd,
        }
    }

    pub(super) fn write(&self, turns: &[(&str, &str, &str)]) {
        let mut out = format!(
            "{}\n",
            json!({"type": "session-owner", "sessionId": self.session})
        );
        let now = time::OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .unwrap();
        for (n, (uuid, role, body)) in turns.iter().enumerate() {
            let content = if *role == "user" {
                json!(body)
            } else {
                json!([{"type": "text", "text": body}])
            };
            let ago = i64::try_from(turns.len() - n).unwrap();
            let at = (now - time::Duration::minutes(ago))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
            let line = json!({
                "type": role,
                "uuid": uuid,
                "sessionId": self.session,
                "timestamp": at,
                "message": {"role": role, "content": content}
            });
            out.push_str(&format!("{line}\n"));
        }
        std::fs::write(&self.path, out).unwrap();
        make_private(&self.path);
    }

    pub(super) fn prompt(&self, fx: &Fixture, prompt: &str) -> Value {
        let payload = json!({
            "session_id": self.session,
            "transcript_path": self.path,
            "cwd": self.cwd,
            "permission_mode": "default",
            "hook_event_name": "UserPromptSubmit",
            "prompt": prompt
        });
        let out = piped(fx, fx.cmd(&["hook", "user-prompt-submit"]), &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    pub(super) fn session_start(&self, fx: &Fixture) -> Value {
        self.start_from(fx, "startup", "claude-desktop")
    }

    pub(super) fn start_from(&self, fx: &Fixture, source: &str, entrypoint: &str) -> Value {
        let payload = json!({
            "session_id": self.session,
            "transcript_path": self.path,
            "cwd": self.cwd,
            "hook_event_name": "SessionStart",
            "source": source
        });
        let mut cmd = fx.cmd(&["hook", "session-start"]);
        cmd.env("CLAUDE_CODE_ENTRYPOINT", entrypoint);
        let out = piped(fx, cmd, &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        if out.stdout.is_empty() {
            Value::Null
        } else {
            json_of(&out)
        }
    }
}

pub(super) fn piped(fx: &Fixture, mut cmd: Command, payload: &Value) -> Output {
    fx.check(&mut cmd, Capture.stdin(payload.to_string().as_bytes()))
}

pub(super) fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

pub(super) fn refused(fx: &Fixture, args: &[&str]) -> (String, String) {
    let mut args = args.to_vec();
    args.push("--json");
    let out = fx.check(Cli(&args), Capture);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected failure for {args:?}, got stdout={} stderr={}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    (
        err["code"].as_str().unwrap().to_owned(),
        err["error"].as_str().unwrap().to_owned(),
    )
}

pub(super) const CHILD: &str = "7f3e2d90-4b6c-4e51-8a83-2c1d0e9f8a71";

pub(super) fn refresh_handoff(fx: &Fixture, parent: &Transcript, mode: &str, body: &str) -> String {
    parent.write(&[("u-1", "user", "Continue working on the widget.")]);
    let out = fx.check(
        with_env(
            &[
                "entry",
                "add",
                "--tag",
                "handoff",
                "--author",
                "opus",
                "--related",
                &format!("session:{}", parent.session),
                "--mode",
                mode,
                "--json",
                body,
            ],
            "CLAUDE_CODE_SESSION_ID",
            &parent.session,
        ),
        Success,
    );
    json_of(&out)["id"].as_str().unwrap().to_owned()
}

pub(super) fn after_clear(fx: &Fixture, parents: &[&str]) -> Transcript {
    let child = Transcript::new(fx, CHILD);
    child.write(&[]);
    let mut sessions = vec![json!({"via":"launch", "id":ON_SESSION})];
    sessions.extend(parents.iter().map(|id| json!({"via":"clear","id":id})));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&child.path)
        .unwrap();
    writeln!(
        file,
        "{}",
        json!({"type":"derived-from","sessions":sessions})
    )
    .unwrap();
    child
}

// Write the opening ancestry before the peer-origin user turn, as Claude does.
pub(super) fn clear_transcript(fx: &Fixture, id: &str, parents: &[&str]) -> Transcript {
    let edges: Vec<_> = parents.iter().map(|parent| ("clear", *parent)).collect();
    ancestry_transcript(fx, id, &edges)
}

// Write the opening ancestry of `id`, one `(via, parent)` edge each, before the
// peer-origin user turn, as Claude does.
pub(super) fn ancestry_transcript(fx: &Fixture, id: &str, edges: &[(&str, &str)]) -> Transcript {
    let tr = Transcript::new(fx, id);
    let sessions: Vec<_> = edges
        .iter()
        .map(|(via, parent)| json!({"via": via, "id": parent}))
        .collect();
    let rows = [
        json!({"type":"session-owner", "sessionId":id}),
        json!({"type":"derived-from", "sessions":sessions}),
        json!({"type":"user", "uuid":"peer-1", "sessionId":id,
            "timestamp":"2026-11-22T12:00:00Z", "promptSource":"system",
            "turnOrigin":"peer", "message":{"role":"user", "content":"<cross-session>continue from handoff-1</cross-session>"}}),
    ];
    std::fs::write(
        &tr.path,
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    make_private(&tr.path);
    tr
}

pub(super) fn try_refresh(fx: &Fixture, id: &str) -> Output {
    fx.check(
        with_env(
            &[
                "entry",
                "add",
                "--tag",
                "handoff",
                "--author",
                "opus",
                "--related",
                &format!("session:{id}"),
                "--mode",
                "continue",
                "--json",
                &valid_handoff(&format!("Continued {id}."), "Keep working."),
            ],
            "CLAUDE_CODE_SESSION_ID",
            id,
        ),
        Capture,
    )
}

// A session id no transcript on this host has.
pub(super) const UNREADABLE_SESSION: &str = "9a0b1c2d-3e4f-4a5b-8c6d-7e8f9a0b1c2d";
