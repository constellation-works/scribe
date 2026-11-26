use crate::support;
use crate::support::command::Exit::{Capture, Success};
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, make_private, snapshot};
use serde_json::{Value, json};
use std::path::PathBuf;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run_with(self, &[], expect, |_, command| {
            command.env("GITHUB_TOKEN", "synthetic-fixture-token-value");
        })
    }
}

pub(super) const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";

pub(super) const CODEX_SESSION: &str = "11111111-2222-4333-8444-555555555555";

pub(super) const STORED_SESSION: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

/// One transcript line: `(uuid, role, text, minutes before now)`.
pub(super) type Turn<'a> = (&'a str, &'a str, &'a str, i64);

/// A Claude Code transcript under the fixture's home, every line carrying
/// `entrypoint` (or none, for `None`).
pub(super) fn claude(fx: &Fixture, entrypoint: Option<&str>, turns: &[Turn<'_>]) -> PathBuf {
    let project = fx.home().join(".claude/projects/-work-scribe");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{SESSION}.jsonl"));
    let now = OffsetDateTime::now_utc();
    let mut out = String::new();
    for (uuid, role, body, ago) in turns {
        let content = if *role == "user" {
            json!(body)
        } else {
            json!([{"type": "text", "text": body}])
        };
        let at = (now - time::Duration::minutes(*ago))
            .format(&Rfc3339)
            .unwrap();
        let mut line = json!({"type": role, "uuid": uuid, "sessionId": SESSION,
            "timestamp": at, "cwd": "/work/scribe",
            "message": {"role": role, "content": content}});
        if let Some(entrypoint) = entrypoint {
            line["entrypoint"] = json!(entrypoint);
        }
        out.push_str(&format!("{line}\n"));
    }
    std::fs::write(&path, out).unwrap();
    make_private(&path);
    path
}

/// A Codex rollout recorded with `source`, holding Daniel's typed `turns`
/// `(id, text, minutes before now)`, private to this user; returns its path.
pub(super) fn codex(fx: &Fixture, source: &str, turns: &[(&str, &str, i64)]) -> PathBuf {
    let dir = fx.home().join(".codex/sessions/2026/11/21");
    fx.create_private_dir_all(&dir);
    let path = dir.join(format!("rollout-2026-11-21T00-00-00-{CODEX_SESSION}.jsonl"));
    let now = OffsetDateTime::now_utc();
    let mut out = format!(
        "{}\n",
        json!({"type": "session_meta", "payload": {"id": CODEX_SESSION, "source": source,
            "thread_source": "user", "cwd": fx.home()}})
    );
    for (id, body, ago) in turns {
        let at = (now - time::Duration::minutes(*ago))
            .format(&Rfc3339)
            .unwrap();
        out.push_str(&format!(
            "{}\n",
            json!({"type": "response_item", "timestamp": at, "payload": {"type": "message",
                "id": id, "role": "user", "content": [{"type": "input_text", "text": body}],
                "internal_chat_message_metadata_passthrough": {"turn_id": id,
                    "content_item_kinds": ["user.text"],
                    "content_item_metadata": [{"provenance": {"type": "user"}}]}}})
        ));
    }
    std::fs::write(&path, out).unwrap();
    make_private(&path);
    path
}

/// Run `args` with `--json`; the exit code and the error code, if any.
pub(super) fn run(fx: &Fixture, args: &[&str]) -> (Option<i32>, Option<String>) {
    let mut args = args.to_vec();
    args.push("--json");
    let out = fx.check(&args, Capture);
    let code = serde_json::from_slice::<Value>(&out.stderr)
        .ok()
        .and_then(|err| err["code"].as_str().map(str::to_owned));
    (out.status.code(), code)
}

/// `entry retire <id>` on `instruction` in [`SESSION`].
pub(super) fn retire<'a>(id: &'a str, instruction: &'a str) -> Vec<&'a str> {
    vec![
        "entry",
        "retire",
        id,
        "--author",
        "opus",
        "--reason",
        "Daniel asked",
        "--session",
        SESSION,
        "--instruction",
        instruction,
    ]
}

/// A seeded store with one agent entry, `preferences-2`.
pub(super) fn store_with_agent_entry() -> Fixture {
    let fx = Fixture::seeded();
    fx.check(
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "Use the stable API",
        ],
        Success,
    );
    fx
}

/// What a refusal must leave alone: the store's files, but the database,
/// its WAL and the audit spool (every audited refusal inserts its line
/// there), and every exported record but the audit log, which stands for the
/// rows inside the database.
pub(super) fn state(fx: &Fixture) -> State {
    let files = snapshot(&fx.root())
        .into_iter()
        .filter(|(path, _, _)| {
            let name = path.to_string_lossy();
            !name.contains("scribe.db") && !name.contains("audit-spool")
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("export");
    scribe_core::Store::open(fx.root())
        .export(&target, scribe_core::ExportFormat::Markdown)
        .unwrap();
    let records = snapshot(dir.path())
        .into_iter()
        .filter(|(path, _, _)| !path.ends_with("audit.jsonl"))
        .map(|(path, bytes, mode)| {
            (
                path.strip_prefix(dir.path()).unwrap().to_path_buf(),
                bytes,
                mode,
            )
        })
        .collect();
    State { files, records }
}

/// [`state`]'s result; a mismatch prints the files' names and sizes, not
/// their bytes.
#[derive(PartialEq)]
pub(super) struct State {
    files: Vec<(PathBuf, Vec<u8>, u32)>,
    records: Vec<(PathBuf, Vec<u8>, u32)>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (kind, set) in [("file", &self.files), ("record", &self.records)] {
            for (path, bytes, mode) in set {
                let name = path.components().rev().take(3).collect::<Vec<_>>();
                let sum = bytes
                    .iter()
                    .fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(u64::from(*b)));
                writeln!(
                    f,
                    "{kind} {name:?} {} bytes sum {sum} mode {mode:o}",
                    bytes.len()
                )?;
            }
        }
        Ok(())
    }
}

/// Daniel's superseded entry `decisions-1` in the seeded store, word for
/// word, which the open proposal `p-1` quotes.
pub(super) const DECISION: &str = "merge agent PRs with a merge commit";

/// Daniel's active entry `preferences-1` in the seeded store, word for word.
pub(super) const PREFERENCE: &str = "short commit subjects, imperative mood";

/// `entry forget <id>` on `instruction` in [`SESSION`], with `--confirm`.
pub(super) fn forget<'a>(id: &'a str, instruction: &'a str) -> Vec<&'a str> {
    vec![
        "entry",
        "forget",
        id,
        "--author",
        "opus",
        "--session",
        SESSION,
        "--instruction",
        instruction,
        "--confirm",
    ]
}

/// The audit log's lines for `command`.
pub(super) fn audit_lines(fx: &Fixture, command: &str) -> Vec<Value> {
    support::audit_text(&fx.root())
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|l| l["command"] == command)
        .collect()
}

/// Every file under the store root (`pre-sqlite-*/` included), and every
/// exported record, whose bytes hold `needle`.
pub(super) fn files_holding(fx: &Fixture, needle: &str) -> Vec<PathBuf> {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("export");
    scribe_core::Store::open(fx.root())
        .export(&target, scribe_core::ExportFormat::Markdown)
        .unwrap();
    snapshot(&fx.root())
        .into_iter()
        .chain(snapshot(dir.path()))
        .filter(|(_, bytes, _)| {
            bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        })
        .map(|(path, _, _)| path)
        .collect()
}

/// Daniel's claim the session pass opened as `p-1` (reserving
/// `preferences-1`): scribe's statement of his turn, his words in `quote`.
pub(super) const CLAIM_QUOTE: &str = "I prefer reviews that name the file and line.";

pub(super) const CLAIM_STATEMENT: &str = "Daniel prefers reviews that name the file and line.";

/// An assistant turn's claim, `p-2` (reserving `preferences-2`).
pub(super) const ASSISTANT_QUOTE: &str = "Run the goldens after every help change.";

/// `proposal settle <id>` with `act` on `instruction` in [`SESSION`].
pub(super) fn settle<'a>(id: &'a str, act: &'a str, instruction: &'a str) -> Vec<&'a str> {
    vec![
        "proposal",
        "settle",
        id,
        act,
        "--author",
        "opus",
        "--session",
        SESSION,
        "--instruction",
        instruction,
    ]
}

/// A store holding only two open session-pass candidates: `p-1` from
/// Daniel's turn and `p-2` from an assistant's, in another session.
pub(super) fn store_with_candidates() -> Fixture {
    store_with_candidates_opened_at("2026-11-15T00:01:00Z")
}

/// [`store_with_candidates`], both claims opened at `opened_at`.
pub(super) fn store_with_candidates_opened_at(opened_at: &str) -> Fixture {
    let fx = Fixture::new();
    let cite = |message: &str| {
        json!({"session": "s-1", "message": message, "at": "2026-11-15T00:00:00Z",
            "prompt_sha256": "0".repeat(64), "line": 1, "cwd": "/work/scribe"})
    };
    let candidate = |id: &str, older: &str, role: &str, quote: &str, statement: Option<&str>| {
        json!({
            "id": id, "kind": "candidate", "older": older, "newer": null,
            "candidate": {
                "tag": "preferences", "role": role, "quote": quote,
                "context": null, "statement": statement,
                "subject": statement.map(|_| "code reviews"),
                "evidence": null, "cite": cite(&format!("turn-{id}")),
                "project": "scribe", "provider": "claude-code", "host": "host",
                "admitted": null
            },
            "reason": format!("Unmarked preferences statement: {quote}"), "quotes": [],
            "state": "open", "opened_at": opened_at, "settled_at": null,
            "settled_by": null, "author": {"kind": "scribe", "name": "session-pass"},
            "settled_author": null, "updated_by": null, "closed_reason": null,
            "instruction": null
        })
    };
    let proposals = json!({"format": 31, "proposals": [
        candidate("p-1", "preferences-1", "user", CLAIM_QUOTE, Some(CLAIM_STATEMENT)),
        candidate("p-2", "preferences-2", "assistant", ASSISTANT_QUOTE, None),
    ]})
    .to_string();
    support::seed_records(&fx.root(), &[&proposals]);
    fx
}

/// `<noun> show <id> --json`.
pub(super) fn show(fx: &Fixture, noun: &str, id: &str) -> Value {
    if noun == "proposal" {
        let listed: Value =
            serde_json::from_slice(&fx.check(&["proposal", "list", "--json"], Success).stdout)
                .unwrap();
        return listed["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .cloned()
            .unwrap();
    }
    serde_json::from_slice(&fx.check(&[noun, "show", id, "--json"], Success).stdout).unwrap()
}

/// The error code and message of a `--json` refusal.
pub(super) fn refusal(fx: &Fixture, args: &[&str]) -> (Option<i32>, Value) {
    let mut args = args.to_vec();
    args.push("--json");
    let out = fx.check(&args, Capture);
    (
        out.status.code(),
        serde_json::from_slice(&out.stderr).unwrap(),
    )
}

pub(super) fn write_desktop_transcript(fx: &Fixture, lines: &[(&str, &str, Value)]) {
    let project = fx.home().join(".claude/projects/-work-scribe");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{SESSION}.jsonl"));
    let at = (OffsetDateTime::now_utc() - time::Duration::minutes(1))
        .format(&Rfc3339)
        .unwrap();
    let mut out = String::new();
    for (uuid, body, extra) in lines {
        let mut line = json!({
            "type": "user",
            "uuid": uuid,
            "sessionId": SESSION,
            "timestamp": at,
            "cwd": "/work/scribe",
            "entrypoint": "claude-desktop",
            "message": {"role": "user", "content": body}
        });
        if let (Some(obj), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        out.push_str(&format!("{line}\n"));
    }
    std::fs::write(&path, out).unwrap();
    make_private(&path);
}
