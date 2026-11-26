use crate::support;
use crate::support::command::Exit::{Capture, Success};
use crate::support::command::Input::Cli;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, make_private, seed_records, text};
use rusqlite::{Connection, OpenFlags};
use scribe_core::message::NewMessage;
use scribe_core::testing::StoreTestReads;
use scribe_core::{Redactor, Speaker, Store};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Output;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::datetime;

pub(super) const SOURCE: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

const USER: &str = "50ffac20-d430-4d20-a54d-0f6b2afcf5be";

pub(super) const AT: OffsetDateTime = datetime!(2026-11-02 20:00 UTC);

pub(super) const KEPT: &str = "keep-unrelated-archive-8bc43";

pub(super) const ARCHIVE: &str = "pre-sqlite-2026-10-01";

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

struct Concurrent;

impl Expectation for Concurrent {
    type Output = crate::support::ChildGuard;

    #[track_caller]
    fn observe(self, command: &mut Command) -> Self::Output {
        use std::process::Stdio;
        crate::support::ChildGuard::new(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("start concurrent forget"),
        )
    }
}

pub(super) struct EntrySeed {
    pub(super) n: usize,
    pub(super) id: String,
    old: String,
    pub(super) edited: String,
    pub(super) quote: String,
}

impl EntrySeed {
    pub(super) fn new(n: usize) -> Self {
        Self {
            n,
            id: format!("preferences-{n}"),
            old: format!("erase-original-entry-{n}-51d7e"),
            edited: format!("erase-edited-entry-{n}-b16ee"),
            quote: format!("erase-claim-quote-{n}-cf72a"),
        }
    }

    pub(super) fn markers(&self) -> [&str; 3] {
        [&self.old, &self.edited, &self.quote]
    }
}

/// Include even files that are hidden, have no known suffix, or contain
/// binary data. Report the paths, so a new copy identifies its own surface.
pub(super) fn copies(root: &Path, marker: &str) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for item in fs::read_dir(dir).unwrap() {
            let item = item.unwrap();
            let kind = item.file_type().unwrap();
            let path = item.path();
            if kind.is_dir() {
                dirs.push(path);
            } else {
                assert!(kind.is_file(), "unexpected file type: {}", path.display());
                if contains(&path, marker) {
                    hits.push(path.strip_prefix(root).unwrap().to_path_buf());
                }
            }
        }
    }
    hits.sort();
    hits
}

pub(super) fn contains(path: &Path, marker: &str) -> bool {
    fs::read(path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
        .windows(marker.len())
        .any(|bytes| bytes == marker.as_bytes())
}

pub(super) fn put(fx: &Fixture, path: &Path, body: &str) {
    fx.create_private_dir_all(path.parent().unwrap());
    fs::write(path, body).unwrap();
    make_private(path);
}

pub(super) fn json_output(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

pub(super) fn transcript(fx: &Fixture, instructions: &[String]) -> PathBuf {
    let path = fx
        .home()
        .join(format!(".claude/projects/-work-scribe/{USER}.jsonl"));
    let now = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let lines: String = instructions
        .iter()
        .enumerate()
        .map(|(n, words)| {
            let turn = json!({
                "type": "user", "uuid": format!("u-{n}"), "timestamp": now,
                "cwd": "/work/scribe", "entrypoint": "claude-desktop",
                "message": {"role": "user", "content": words},
            });
            format!("{turn}\n")
        })
        .collect();
    put(fx, &path, &lines);
    path
}

fn run_submit(fx: &Fixture, prompt: &str) -> Output {
    let payload = json!({
        "session_id": USER, "transcript_path": transcript(fx, &[]),
        "cwd": "/work/scribe", "permission_mode": "default",
        "hook_event_name": "UserPromptSubmit", "prompt": prompt,
    });
    fx.check(
        Cli(&["hook", "user-prompt-submit"]),
        Capture.stdin(payload.to_string().as_bytes()),
    )
}

pub(super) fn submit(fx: &Fixture, prompt: &str) -> String {
    let out = run_submit(fx, prompt);
    assert!(out.status.success(), "{}", text(&out.stderr));
    json_output(&out)["systemMessage"]
        .as_str()
        .unwrap()
        .to_owned()
}

pub(super) fn entry_forget_args<'a>(id: &'a str, instruction: &'a str) -> [&'a str; 10] {
    [
        "entry",
        "forget",
        id,
        "--author",
        "opus",
        "--session",
        USER,
        "--instruction",
        instruction,
        "--confirm",
    ]
}

/// Whether any record `scribe.db` archived holds `marker`.
pub(super) fn archived_has(fx: &Fixture, marker: &str) -> bool {
    let n: i64 = db(fx)
        .query_row(
            "SELECT count(*) FROM archive_records WHERE instr(record, ?1) > 0",
            [marker],
            |row| row.get(0),
        )
        .unwrap();
    n > 0
}

pub(super) fn export(fx: &Fixture, dir: &Path, format: &str) {
    fx.check(
        Cli(&["export", dir.to_str().unwrap(), "--as", format]),
        Success,
    );
}

pub(super) fn sweep(fx: &Fixture, markers: &[&str]) {
    // Look on disk before any later command has a chance to repair it.
    let check = || {
        for marker in markers {
            let found = copies(&fx.root(), marker);
            assert!(found.is_empty(), "{marker} remains in {found:?}");
        }
    };
    check();
    // Exports are snapshots owned by their caller. Sweep fresh exports to
    // catch renderers that expose text no longer visible in normal reads.
    for format in ["markdown", "jsonl"] {
        export(fx, &fx.root().join(format!("export-{format}")), format);
    }
    check();
}

/// A format-31 entry an agent wrote, so `entry update` may change it.
pub(super) fn agent_entry(n: usize, content: &str) -> String {
    let text = support::entry_text(n, content);
    let mut out = Vec::new();
    for line in text.lines() {
        out.push(if line.starts_with("author:") {
            "author: {kind: agent, name: opus}".to_owned()
        } else if line.starts_with("said:") {
            "said: ''".to_owned()
        } else if line.starts_with("content_from:") {
            "content_from: {kind: agent}".to_owned()
        } else if line.starts_with("cite:") {
            "cite: null".to_owned()
        } else {
            line.to_owned()
        });
    }
    out.join("\n") + "\n"
}

/// Stage a store with live entries and the archive a phase-7 move left of
/// them (old text), then let the next write import the archive and an edit
/// make the live copy differ. Current audit writers store ids only.
pub(super) fn seed_entries(fx: &Fixture, entries: &[EntrySeed]) {
    let texts: Vec<String> = entries
        .iter()
        .map(|seed| agent_entry(seed.n, &seed.old))
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    seed_records(&fx.root(), &refs);

    let archive = fx.root().join(ARCHIVE);
    let mut claims = Vec::new();
    let mut audit = String::new();
    for (i, (seed, text)) in entries.iter().zip(&texts).enumerate() {
        // The archived body ends in a newline; the audit line's free text
        // holds the same words without it, and the forget still erases it.
        put(
            fx,
            &archive.join(format!("entries/preferences/{}.md", seed.id)),
            text,
        );
        claims.push(json!({
            "id": format!("p-{}", i + 1), "kind": "stale", "older": seed.id,
            "reason": "An obsolete preference",
            "quotes": [{"entry": seed.id, "text": seed.quote}],
            "state": "open", "opened_at": "2026-11-02T20:00:00Z",
            "newer": null, "settled_at": null, "settled_by": null,
        }));
        audit.push_str(&legacy_audit(&seed.id, &seed.old));
    }
    audit.push_str(&legacy_audit("preferences-99", KEPT));
    put(fx, &archive.join("audit.jsonl"), &audit);
    put(
        fx,
        &archive.join("proposals.json"),
        &json!({"format": 1, "proposals": claims}).to_string(),
    );
    for seed in entries {
        fx.check(
            &[
                "update",
                &seed.id,
                "--author",
                "opus",
                "--content",
                &seed.edited,
            ],
            Success,
        );
        assert!(archived_has(fx, &seed.old));
        assert!(archived_has(fx, &seed.quote));
        assert!(contains(&fx.root().join("scribe.db"), &seed.edited));
    }
    assert!(!archive.exists(), "the write imported the archive");
}

pub(super) fn legacy_audit(target: &str, marker: &str) -> String {
    let line = json!({
        "format": 5, "at": "2026-11-02T20:00:00Z", "command": "hook stop",
        "status": "success", "target": target, "detail": marker, "duration_ms": 1,
    });
    format!("{line}\n")
}

pub(super) fn db(fx: &Fixture) -> Connection {
    Connection::open_with_flags(
        fx.root().join("scribe.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}

pub(super) fn pending(fx: &Fixture) -> i64 {
    db(fx)
        .query_row("SELECT count(*) FROM pending_archive_forgets", [], |row| {
            row.get(0)
        })
        .unwrap()
}

pub(super) fn write_turn(store: &Store, marker: &str, message: &str) {
    assert_eq!(
        store
            .write_messages(
                &[NewMessage {
                    session: SOURCE.parse().unwrap(),
                    message: message.to_owned(),
                    role: Speaker::User,
                    at: AT,
                    text: marker.to_owned(),
                }],
                &Redactor::default()
            )
            .unwrap(),
        1,
    );
}

pub(super) fn forget_concurrently(fx: &Fixture, entries: &[EntrySeed; 2], words: &[String; 2]) {
    let children = entries
        .each_ref()
        .into_iter()
        .zip(words)
        .map(|(entry, words)| fx.check(Cli(&entry_forget_args(&entry.id, words)), Concurrent))
        .collect::<Vec<_>>();
    for child in children {
        let out = child.finish();
        assert!(out.status.success(), "{}", text(&out.stderr));
    }
}
