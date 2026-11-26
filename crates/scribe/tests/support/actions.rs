use crate::support;
use crate::support::command::Exit::{Capture, Success};
use crate::support::command::Input::Cli;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, make_private, text};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Output;

pub(super) const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";

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

/// A Claude Code layout under the fixture's home.
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

    /// Write the transcript: `(uuid, role, text)` turns, a minute apart,
    /// the last a minute ago.
    pub(super) fn write(&self, turns: &[(&str, &str, &str)]) {
        let mut out = format!(
            "{}\n",
            json!({"type": "session-owner", "sessionId": SESSION})
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
            let line = json!({"type": role, "uuid": uuid, "sessionId": SESSION,
                "timestamp": at,
                "message": {"role": role, "content": content}});
            out.push_str(&format!("{line}\n"));
        }
        std::fs::write(&self.transcript, out).unwrap();
        make_private(&self.transcript);
    }

    /// Run the `UserPromptSubmit` hook for `prompt`; the reply's
    /// `systemMessage`.
    pub(super) fn prompt(&self, fx: &Fixture, prompt: &str) -> String {
        let payload = json!({"session_id": SESSION,
            "transcript_path": self.transcript, "cwd": self.cwd,
            "permission_mode": "default", "hook_event_name": "UserPromptSubmit",
            "prompt": prompt});
        let out = stdin_run(fx, &["hook", "user-prompt-submit"], &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        reply["systemMessage"].as_str().unwrap().to_owned()
    }

    /// Run the `Stop` hook; the reply's `systemMessage` and
    /// `additionalContext`, empty when it printed nothing.
    pub(super) fn stop(&self, fx: &Fixture) -> (String, String) {
        let payload = json!({"session_id": SESSION,
            "transcript_path": self.transcript, "cwd": self.cwd,
            "permission_mode": "default", "hook_event_name": "Stop",
            "stop_hook_active": false});
        let out = stdin_run(fx, &["hook", "stop"], &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        if out.stdout.is_empty() {
            return (String::new(), String::new());
        }
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        let field = |v: &Value| v.as_str().unwrap_or_default().to_owned();
        (
            field(&reply["systemMessage"]),
            field(&reply["hookSpecificOutput"]["additionalContext"]),
        )
    }

    /// The session-start hook's stdout.
    pub(super) fn session_start(&self, fx: &Fixture) -> String {
        let payload = json!({"session_id": SESSION, "transcript_path": self.transcript,
            "cwd": self.cwd, "hook_event_name": "SessionStart", "source": "startup"});
        let out = stdin_run(fx, &["hook", "session-start"], &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        text(&out.stdout)
    }
}

fn stdin_run(fx: &Fixture, args: &[&str], payload: &Value) -> Output {
    fx.check(Cli(args), Capture.stdin(payload.to_string().as_bytes()))
}

pub(super) fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Run `args` with `--json`, require failure, and return the error code.
pub(super) fn refused(fx: &Fixture, args: &[&str]) -> String {
    let mut args = args.to_vec();
    args.push("--json");
    let out = fx.check(Cli(&args), Capture);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{args:?}: {}",
        text(&out.stdout)
    );
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    err["code"].as_str().unwrap().to_owned()
}

pub(super) fn entry(fx: &Fixture, id: &str, extra: &[&str]) -> Value {
    let mut args = vec!["entry", "show", id, "--json"];
    args.extend_from_slice(extra);
    json_of(&fx.check(Cli(&args), Success))
}

/// Move entry `id`'s creation back to a fixed past instant, as if it had
/// been written long ago.
pub(super) fn backdate(fx: &Fixture, _tag: &str, id: &str) {
    support::sql(
        &fx.root(),
        &format!("UPDATE entries SET created = '2026-01-01T00:00:00Z' WHERE id = '{id}'"),
    );
}

/// Seed entry `id` under `tag` as the user's, saying `content`, with
/// `fields` (frontmatter key, YAML value) replacing their `null`s.
pub(super) fn seed_entry(
    fx: &Fixture,
    id: &str,
    tag: &str,
    content: &str,
    fields: &[(&str, &str)],
) {
    let mut record = support::entry_text(1, content)
        .replace("id: preferences-1\n", &format!("id: {id}\n"))
        .replace("tag: preferences\n", &format!("tag: {tag}\n"))
        .replace("#scribe preferences:", &format!("#scribe {tag}:"));
    for (key, value) in fields {
        record = record.replace(&format!("\n{key}: null\n"), &format!("\n{key}: {value}\n"));
    }
    support::seed_records(&fx.root(), &[&record]);
}

/// Whether the store holds any entry of `tag`, in any status.
pub(super) fn entries_of_tag(fx: &Fixture, tag: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        fx.root().join("scribe.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    );
    let Ok(conn) = conn else { return false };
    conn.query_row("SELECT count(*) FROM entries WHERE tag = ?1", [tag], |r| {
        r.get::<_, i64>(0)
    })
    .unwrap_or(0)
        > 0
}

/// `marks.json`'s records.
pub(super) fn marks_json(fx: &Fixture) -> Vec<Value> {
    let text = support::exported(&fx.root(), "marks.json").expect("exported marks");
    serde_json::from_str::<Value>(&text).unwrap()["marks"]
        .as_array()
        .unwrap()
        .clone()
}

pub(super) fn tag_row(fx: &Fixture, name: &str) -> Value {
    let tags = json_of(&fx.check(Cli(&["tag", "list", "--json"]), Success));
    tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .cloned()
        .unwrap()
}
