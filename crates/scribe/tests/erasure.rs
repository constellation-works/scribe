//! Exercise the shipped forget routes and search the persisted bytes.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use rusqlite::{Connection, OpenFlags};
use scribe_core::RebuildVerbs;
use scribe_core::message::NewMessage;
use scribe_core::{Redactor, Speaker, Store};
use serde_json::{Value, json};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use support::{ChildGuard, Fixture, make_private, seed_records, text};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::datetime;

const SOURCE: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
const USER: &str = "50ffac20-d430-4d20-a54d-0f6b2afcf5be";
const AT: OffsetDateTime = datetime!(2026-11-02 20:00 UTC);
const KEPT: &str = "keep-unrelated-archive-8bc43";
const ARCHIVE: &str = "pre-sqlite-2026-10-01";

/// Include even files that are hidden, have no known suffix, or contain
/// binary data. Report the paths, so a new copy identifies its own surface.
fn copies(root: &Path, marker: &str) -> Vec<PathBuf> {
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

fn contains(path: &Path, marker: &str) -> bool {
    fs::read(path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
        .windows(marker.len())
        .any(|bytes| bytes == marker.as_bytes())
}

fn put(fx: &Fixture, path: &Path, body: &str) {
    fx.create_private_dir_all(path.parent().unwrap());
    fs::write(path, body).unwrap();
    make_private(path);
}

fn json_output(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn transcript(fx: &Fixture, instructions: &[String]) -> PathBuf {
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
    let mut child = ChildGuard::new(
        fx.cmd(&["hook", "user-prompt-submit"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    child
        .child()
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    child.finish()
}

fn submit(fx: &Fixture, prompt: &str) -> String {
    let out = run_submit(fx, prompt);
    assert!(out.status.success(), "{}", text(&out.stderr));
    json_output(&out)["systemMessage"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn entry_forget_args<'a>(id: &'a str, instruction: &'a str) -> [&'a str; 10] {
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
fn archived_has(fx: &Fixture, marker: &str) -> bool {
    let n: i64 = db(fx)
        .query_row(
            "SELECT count(*) FROM archive_records WHERE instr(record, ?1) > 0",
            [marker],
            |row| row.get(0),
        )
        .unwrap();
    n > 0
}

fn export(fx: &Fixture, dir: &Path, format: &str) {
    fx.ok(&["export", dir.to_str().unwrap(), "--as", format]);
}

fn sweep(fx: &Fixture, markers: &[&str]) {
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

struct EntrySeed {
    n: usize,
    id: String,
    old: String,
    edited: String,
    quote: String,
}

impl EntrySeed {
    fn new(n: usize) -> Self {
        Self {
            n,
            id: format!("preferences-{n}"),
            old: format!("erase-original-entry-{n}-51d7e"),
            edited: format!("erase-edited-entry-{n}-b16ee"),
            quote: format!("erase-claim-quote-{n}-cf72a"),
        }
    }

    fn markers(&self) -> [&str; 3] {
        [&self.old, &self.edited, &self.quote]
    }
}

/// A format-31 entry an agent wrote, so `entry update` may change it.
fn agent_entry(n: usize, content: &str) -> String {
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
fn seed_entries(fx: &Fixture, entries: &[EntrySeed]) {
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
        fx.ok(&[
            "entry",
            "update",
            &seed.id,
            "--author",
            "opus",
            "--content",
            &seed.edited,
        ]);
        assert!(archived_has(fx, &seed.old));
        assert!(archived_has(fx, &seed.quote));
        assert!(contains(&fx.root().join("scribe.db"), &seed.edited));
    }
    assert!(!archive.exists(), "the write imported the archive");
}

fn legacy_audit(target: &str, marker: &str) -> String {
    let line = json!({
        "format": 5, "at": "2026-11-02T20:00:00Z", "command": "hook stop",
        "status": "success", "target": target, "detail": marker, "duration_ms": 1,
    });
    format!("{line}\n")
}

fn db(fx: &Fixture) -> Connection {
    Connection::open_with_flags(
        fx.root().join("scribe.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}

fn pending(fx: &Fixture) -> i64 {
    db(fx)
        .query_row("SELECT count(*) FROM pending_archive_forgets", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn entry_routes() {
    for via_hook in [false, true] {
        let fx = Fixture::new();
        let entry = EntrySeed::new(1);
        seed_entries(&fx, std::slice::from_ref(&entry));
        let prior = fx.elsewhere("caller-export");
        export(&fx, &prior, "markdown");
        assert!(!copies(&prior, &entry.edited).is_empty());
        let words = format!("forget {} for good", entry.id);
        transcript(&fx, std::slice::from_ref(&words));
        let args = entry_forget_args(&entry.id, &words);
        if via_hook {
            let prompt = format!("#scribe forget {}", entry.id);
            assert!(submit(&fx, &prompt).contains("forgot"));
        } else {
            fx.ok(&args);
        }
        assert_eq!(pending(&fx), 0);
        sweep(&fx, &entry.markers());
        assert!(archived_has(&fx, KEPT), "the unrelated archived line stays");
        assert!(!copies(&prior, &entry.edited).is_empty());
    }
}

/// Two forgets at once each erase their own entry from the archive.
fn concurrent_entries() {
    let fx = Fixture::new();
    let entries = [EntrySeed::new(1), EntrySeed::new(2)];
    seed_entries(&fx, &entries);
    let words = entries
        .each_ref()
        .map(|entry| format!("forget {} for good", entry.id));
    transcript(&fx, &words);
    let children = entries
        .each_ref()
        .into_iter()
        .zip(&words)
        .map(|(entry, words)| {
            ChildGuard::new(
                fx.cmd(&entry_forget_args(&entry.id, words))
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    for child in children {
        let out = child.finish();
        assert!(out.status.success(), "{}", text(&out.stderr));
    }
    assert_eq!(pending(&fx), 0);
    sweep(
        &fx,
        &entries
            .iter()
            .flat_map(EntrySeed::markers)
            .collect::<Vec<_>>(),
    );
    assert!(archived_has(&fx, KEPT));
}

fn write_turn(store: &Store, marker: &str, message: &str) {
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

fn session_route() {
    const RECORD: &str = "erase-session-record-95bde";
    const CHECKPOINT: &str = "erase-rebuild-checkpoint-40c70";
    const TURN: &str = "erase-indexed-session-turn-825d2";
    const CLAIM: &str = "erase-session-claim-a9dbd";
    let fx = Fixture::new();
    let rel = format!("sessions/2026/11/{SOURCE}.md");
    let source = Fixture::new();
    source.seed();
    let legacy = support::record_text(&source.root(), SOURCE)
        .unwrap()
        .replace(
            "Discussed how agent PRs are merged and recorded the decision.",
            RECORD,
        );
    assert!(legacy.contains(RECORD));
    seed_records(&fx.root(), &[&legacy]);
    let archive = fx.root().join(ARCHIVE);
    put(&fx, &archive.join(&rel), &legacy);
    put(
        &fx,
        &archive.join("audit.jsonl"),
        &(legacy_audit(&format!("session:{SOURCE}"), RECORD)
            + &legacy_audit("preferences-99", KEPT)),
    );
    let proposals = json!({"format": 11, "proposals": [{
        "id": "p-1", "kind": "candidate", "older": "preferences-1", "newer": null,
        "reason": "The user stated a preference", "quotes": [], "state": "open",
        "opened_at": AT.format(&Rfc3339).unwrap(), "settled_at": null, "settled_by": null,
        "author": {"kind": "scribe", "name": "session-pass"}, "updated_by": null,
        "settled_author": null, "closed_reason": null,
        "candidate": {
            "tag": "preferences", "role": "user", "quote": CLAIM, "project": "scribe",
            "provider": "claude-code", "host": "host", "admitted": null, "evidence": null,
            "cite": {"session": SOURCE, "message": "candidate-turn", "at": AT.format(&Rfc3339).unwrap(),
                "prompt_sha256": "0".repeat(64), "line": 1, "cwd": "/work/scribe"},
        },
    }]});
    put(&fx, &archive.join("proposals.json"), &proposals.to_string());
    put(
        &fx,
        &archive.join(format!("sessions/2026/11/.{SOURCE}.rebuild.md")),
        &legacy.replace(RECORD, CHECKPOINT),
    );
    let imported = json_output(&fx.ok(&["store", "migrate", "--json"]));
    assert_eq!(imported["archives"], 1, "{imported}");
    let store = Store::open(fx.root());
    write_turn(&store, TURN, "private-turn");
    assert!(!archive.exists(), "the write imported the archive");
    for marker in [RECORD, CHECKPOINT, CLAIM] {
        assert!(archived_has(&fx, marker), "{marker} archived");
    }
    let mut checkpoint = store.session(&SOURCE.parse().unwrap()).unwrap().record;
    CHECKPOINT.clone_into(&mut checkpoint.summary);
    store
        .checkpoint_rebuild(checkpoint, &Redactor::default())
        .unwrap();
    for marker in [TURN, CHECKPOINT] {
        assert!(contains(&fx.root().join("scribe.db-wal"), marker));
    }
    let prompt = "#scribe forget s-1";
    let reply = submit(&fx, prompt);
    assert!(reply.contains("forgot s-1"), "{reply}");
    sweep(&fx, &[RECORD, CHECKPOINT, TURN, CLAIM]);
    assert!(archived_has(&fx, KEPT), "the unrelated audit line stays");
    // The session's own audit line keeps its audit fields.
    let kept: i64 = db(&fx)
        .query_row(
            "SELECT count(*) FROM archive_records WHERE kind = 'audit' AND instr(record, ?1) > 0",
            [format!("session:{SOURCE}")],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);
}

fn memory_route(with_linked_turn: bool) {
    const STEM: &str = "2026-12-01-release-ships";
    const BODY: &str = "erase-memory-body-94d87";
    const HISTORY: &str = "erase-memory-history-91a7c";
    const TURN: &str = "erase-indexed-memory-turn-ba701";
    let fx = Fixture::new();
    let file = fx.root().join(format!("memory/now/{STEM}.md"));
    put(
        &fx,
        &file,
        &format!(
            "---\ndate: 2026-12-01\nkind: deadline\nsources: [{SOURCE}]\nupdated: 2026-11-02T20:00:00Z\n---\n# Release ships\n\n{BODY}\n",
        ),
    );
    fx.ok(&[
        "memory",
        "ack",
        STEM,
        "--session",
        SOURCE,
        "--author",
        "opus",
    ]);
    assert!(contains(&fx.root().join("scribe.db-wal"), BODY));
    // A live item can have past text in history and a linked source turn.
    let writer = Connection::open(fx.root().join("scribe.db")).unwrap();
    writer.execute(
        "INSERT INTO memory_history (item, kind, text, sources, removed_at) VALUES (?1, 'now', ?2, ?3, ?4)",
        (STEM, HISTORY, json!([SOURCE]).to_string(), "2026-11-02T20:00:00Z"),
    ).unwrap();
    if with_linked_turn {
        write_turn(&Store::open(fx.root()), TURN, "memory-turn");
        writer
            .execute(
                "INSERT INTO memory_messages (item, session, message) VALUES (?1, ?2, 'memory-turn')",
                (STEM, SOURCE),
            )
            .unwrap();
    }
    drop(writer);
    assert!(contains(&file, BODY));
    assert!(contains(&fx.root().join("scribe.db"), BODY));
    let mut markers = vec![BODY, HISTORY];
    if with_linked_turn {
        markers.push(TURN);
    }
    for marker in &markers {
        assert!(!copies(&fx.root(), marker).is_empty());
    }
    fx.ok(&["memory", "forget", STEM, "--author", "opus", "--confirm"]);
    sweep(&fx, &markers);
}

fn imported_issue_route() {
    const CAUSE: &str = "erase-imported-issue-cause-2fa40";
    const FIX: &str = "erase-imported-issue-fix-514df";
    const CORRECTED: &str =
        "Corrected incident notes\nCause: Different cause.\nFix: Different fix.";
    let fx = Fixture::new();
    fx.config("[tags.issue]\ndescription = \"Stored incidents\"\n");
    // Two live issue entries; issue-2 quotes the cause the archived issue-1
    // file holds, so the file is its source whatever its name says.
    let issue = |n: usize, content: &str, quote: &str| {
        agent_entry(1, content)
            .replace("preferences-1", &format!("issue-{n}"))
            .replace("tag: preferences", "tag: issue")
            .replace("quote: null", &format!("quote: \"{quote}\""))
    };
    let kept = issue(1, "kept live issue", "kept live quote");
    let forgotten = issue(2, &format!("Cause: {CAUSE}. Fix: {FIX}"), CAUSE);
    seed_records(&fx.root(), &[&kept, &forgotten]);

    let legacy = include_str!("fixtures/issue-store/issue-1.md");
    let source = legacy
        .replacen(
            "## Cause\ncause not found",
            &format!("## Cause\n{CAUSE}"),
            1,
        )
        .replacen("## Fix\nfix not found", &format!("## Fix\n{FIX}"), 1);
    let neighbor = legacy
        .replace("id: issue-1", "id: issue-2")
        .replace("2026-11-22T04:09:43Z", "2026-11-22T04:10:00Z")
        .replace("GitHub API secondary rate limit during drains", KEPT);
    let archive = fx.root().join("pre-issue-2026-11-22");
    put(&fx, &archive.join("issue-1.md"), &source);
    put(&fx, &archive.join("issue-2.md"), &neighbor);
    // The next write imports the archive like any other.
    fx.ok(&[
        "entry",
        "update",
        "issue-2",
        "--author",
        "opus",
        "--content",
        CORRECTED,
    ]);
    assert!(!archive.exists(), "the write imported the archive");
    assert!(archived_has(&fx, CAUSE));
    assert!(archived_has(&fx, KEPT));

    let prompt = "#scribe forget issue-2";
    assert!(submit(&fx, prompt).contains("forgot"));
    assert_eq!(pending(&fx), 0);
    sweep(&fx, &[CAUSE, FIX]);
    assert!(
        archived_has(&fx, KEPT),
        "the unrelated archived issue stays"
    );
    let unrelated = json_output(&fx.ok(&["entry", "show", "issue-1", "--json"]));
    assert_eq!(unrelated["content"], "kept live issue");
}

#[test]
fn public_forgets_leave_no_copy_of_erased_text_in_the_store() {
    entry_routes();
    concurrent_entries();
    session_route();
    memory_route(false);
    memory_route(true);
    imported_issue_route();
}
