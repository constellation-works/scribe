//! Exercise the shipped forget routes and search the persisted bytes.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use rusqlite::{Connection, OpenFlags};
use scribe_core::message::NewMessage;
use scribe_core::{Redactor, Speaker, Store};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};
use support::{ChildGuard, Fixture, fixture_store, make_private, text};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::datetime;

const SOURCE: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
const USER: &str = "50ffac20-d430-4d20-a54d-0f6b2afcf5be";
const AT: OffsetDateTime = datetime!(2026-11-02 20:00 UTC);
const KEPT: &str = "keep-unrelated-archive-8bc43";

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

fn migrate(fx: &Fixture) -> PathBuf {
    let out = json_output(&fx.ok(&["store", "migrate", "--json"]));
    fx.root().join(out["moved_to"].as_str().unwrap())
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
    id: String,
    old: String,
    edited: String,
    quote: String,
}

impl EntrySeed {
    fn new(n: usize) -> Self {
        Self {
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

/// Stage an old file store, then let the real migration and edit produce
/// different archived and live copies. Current audit writers store ids only.
fn seed_entries(fx: &Fixture, entries: &[EntrySeed]) -> PathBuf {
    let before_move = Fixture::new();
    for seed in entries {
        before_move.ok(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            &seed.old,
        ]);
    }
    let markdown = before_move.elsewhere("legacy");
    export(&before_move, &markdown, "markdown");

    let mut claims = Vec::new();
    let mut audit = String::new();
    for (i, seed) in entries.iter().enumerate() {
        let rel = format!("entries/preferences/{}.md", seed.id);
        put(
            fx,
            &fx.root().join(&rel),
            &fs::read_to_string(markdown.join(&rel)).unwrap(),
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
    put(fx, &fx.root().join("audit.jsonl"), &audit);
    put(
        fx,
        &fx.root().join("proposals.json"),
        &json!({"format": 1, "proposals": claims}).to_string(),
    );
    let archive = migrate(fx);
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
        assert!(contains(&archive.join("audit.jsonl"), &seed.old));
        assert!(contains(&archive.join("proposals.json"), &seed.quote));
        assert!(contains(&fx.root().join("scribe.db"), &seed.edited));
    }
    archive
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

fn block_archive_cleanup(fx: &Fixture) -> PathBuf {
    // A directory at the lock path makes acquisition fail immediately,
    // after the erasure transaction has committed.
    let lock = fx.root().join("locks/archive-forget.lock");
    if lock.is_file() {
        fs::remove_file(&lock).unwrap();
    }
    fx.create_private_dir_all(&lock);
    lock
}

fn assert_archive_cleanup_failed(out: &Output) {
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("archive-forget.lock"));
}

fn entry_routes() {
    for via_hook in [false, true] {
        for interrupted in [false, true] {
            let fx = Fixture::new();
            let entry = EntrySeed::new(1);
            let archive = seed_entries(&fx, std::slice::from_ref(&entry));
            let prior = fx.elsewhere("caller-export");
            export(&fx, &prior, "markdown");
            assert!(!copies(&prior, &entry.edited).is_empty());
            let words = format!("forget {} for good", entry.id);
            transcript(&fx, std::slice::from_ref(&words));
            let args = entry_forget_args(&entry.id, &words);
            let prompt = format!("#scribe forget {}", entry.id);
            if interrupted {
                let lock = block_archive_cleanup(&fx);
                let out = if via_hook {
                    run_submit(&fx, &prompt)
                } else {
                    fx.run(&[args.as_slice(), &["--json"]].concat())
                };
                assert_archive_cleanup_failed(&out);
                assert_eq!(pending(&fx), 1);
                let shown = json_output(&fx.ok(&["entry", "show", &entry.id, "--json"]));
                assert_eq!(shown["status"], "forgotten");
                assert!(contains(&archive.join("audit.jsonl"), &entry.old));
                fs::remove_dir(lock).unwrap();
            }
            if via_hook {
                assert!(submit(&fx, &prompt).contains(if interrupted {
                    "already"
                } else {
                    "forgot"
                }));
            } else {
                fx.ok(&args);
            }
            assert_eq!(pending(&fx), 0);
            sweep(&fx, &entry.markers());
            assert!(contains(&archive.join("audit.jsonl"), KEPT));
            assert!(!copies(&prior, &entry.edited).is_empty());
        }
    }
}

fn interleaved_entries() {
    let fx = Fixture::new();
    let entries = [EntrySeed::new(1), EntrySeed::new(2)];
    let archive = seed_entries(&fx, &entries);
    let words = entries
        .each_ref()
        .map(|entry| format!("forget {} for good", entry.id));
    transcript(&fx, &words);
    let path = fx.root().join("locks/archive-forget.lock");
    fx.create_private_dir_all(path.parent().unwrap());
    let lock = File::create(&path).unwrap();
    make_private(&path);
    lock.lock().unwrap();
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
    // Both transactions must have committed while neither can acquire
    // the archive lock. The store unit tests also interleave at each read
    // inside the proposals/audit read-modify-write.
    let deadline = Instant::now() + Duration::from_secs(5);
    while pending(&fx) < 2 {
        assert!(
            Instant::now() < deadline,
            "both forgets must commit before archive cleanup"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    for entry in &entries {
        assert!(contains(&archive.join("audit.jsonl"), &entry.old));
    }
    drop(lock);
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
    assert!(contains(&archive.join("audit.jsonl"), KEPT));
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

fn session_route(interrupted: bool) {
    const RECORD: &str = "erase-session-record-95bde";
    const CHECKPOINT: &str = "erase-rebuild-checkpoint-40c70";
    const TURN: &str = "erase-indexed-session-turn-825d2";
    const CLAIM: &str = "erase-session-claim-a9dbd";
    let fx = Fixture::new();
    let rel = format!("sessions/2026/11/{SOURCE}.md");
    let legacy = fs::read_to_string(fixture_store().join(&rel))
        .unwrap()
        .replace(
            "Discussed how agent PRs are merged and recorded the decision.",
            RECORD,
        );
    put(&fx, &fx.root().join(&rel), &legacy);
    put(
        &fx,
        &fx.root().join("audit.jsonl"),
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
    put(
        &fx,
        &fx.root().join("proposals.json"),
        &proposals.to_string(),
    );
    let archive = migrate(&fx);
    let store = Store::open(fx.root());
    write_turn(&store, TURN, "private-turn");
    let mut checkpoint = store.session(&SOURCE.parse().unwrap()).unwrap().record;
    CHECKPOINT.clone_into(&mut checkpoint.summary);
    store
        .checkpoint_rebuild(checkpoint, &Redactor::default())
        .unwrap();
    let archived_checkpoint = archive
        .join(&rel)
        .with_file_name(format!(".{SOURCE}.rebuild.md"));
    put(
        &fx,
        &archived_checkpoint,
        &legacy.replace(RECORD, CHECKPOINT),
    );
    assert!(contains(&archive.join(&rel), RECORD));
    assert!(contains(&archive.join("audit.jsonl"), RECORD));
    assert!(contains(&archive.join("proposals.json"), CLAIM));
    assert!(contains(&archived_checkpoint, CHECKPOINT));
    for marker in [TURN, CHECKPOINT] {
        assert!(contains(&fx.root().join("scribe.db-wal"), marker));
    }
    let prompt = "#scribe forget s-1";
    if interrupted {
        let lock = block_archive_cleanup(&fx);
        assert_archive_cleanup_failed(&run_submit(&fx, prompt));
        let committed: bool = db(&fx)
            .query_row(
                "SELECT forgotten IS NOT NULL FROM sessions WHERE provider_session = ?1",
                [SOURCE],
                |row| row.get(0),
            )
            .unwrap();
        assert!(committed);
        assert!(contains(&archived_checkpoint, CHECKPOINT));
        fs::remove_dir(lock).unwrap();
    }
    let reply = submit(&fx, prompt);
    assert!(
        reply.contains(if interrupted {
            "already forgotten"
        } else {
            "forgot s-1"
        }),
        "{reply}"
    );
    sweep(&fx, &[RECORD, CHECKPOINT, TURN, CLAIM]);
    assert!(contains(&archive.join("audit.jsonl"), KEPT));
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
    // Reserve issue-1 and force the import to assign the legacy issue a
    // different entry id. The old filename alone cannot identify its entry.
    fx.ok(&["entry", "add", "--tag", "issue", "--author", "opus", KEPT]);
    let legacy = include_str!("fixtures/issue-store/issue-1.md");
    let issue = legacy
        .replacen(
            "## Cause\ncause not found",
            &format!("## Cause\n{CAUSE}"),
            1,
        )
        .replacen("## Fix\nfix not found", &format!("## Fix\n{FIX}"), 1);
    let source = fx.root().join("issues/issue-1.md");
    put(&fx, &source, &issue);
    let kept_issue = legacy
        .replace("id: issue-1", "id: issue-2")
        .replace("2026-11-22T04:09:43Z", "2026-11-22T04:10:00Z")
        .replace("GitHub API secondary rate limit during drains", KEPT);
    put(&fx, &fx.root().join("issues/issue-2.md"), &kept_issue);
    fx.ok(&[
        "entry",
        "update",
        "issue-1",
        "--author",
        "opus",
        "--content",
        KEPT,
    ]);
    let archived = copies(&fx.root(), CAUSE)
        .into_iter()
        .find(|path| {
            path.iter()
                .next()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("pre-issue-")
        })
        .unwrap();
    let archived = fx.root().join(archived);
    let neighbor = archived.with_file_name("issue-2.md");
    assert!(contains(&neighbor, KEPT));
    let issue = json_output(&fx.ok(&["entry", "show", "issue-2", "--json"]));
    assert!(issue["content"].as_str().unwrap().contains(CAUSE));
    fx.ok(&[
        "entry",
        "update",
        "issue-2",
        "--author",
        "opus",
        "--content",
        CORRECTED,
    ]);
    let prompt = "#scribe forget issue-2";
    // A different legacy source can share the creation metadata and
    // quote. The edited body matching it is not proof of ownership.
    let ambiguous = fx.root().join("pre-issue-test-collision/issue-1.md");
    let alternative = legacy
        .replace(
            "GitHub API secondary rate limit during drains",
            "Corrected incident notes",
        )
        .replacen("## Cause\ncause not found", "## Cause\nDifferent cause.", 1)
        .replacen("## Fix\nfix not found", "## Fix\nDifferent fix.", 1);
    put(&fx, &ambiguous, &alternative);
    let refusal = submit(&fx, prompt);
    assert!(refusal.contains("multiple archived issues"), "{refusal}");
    assert_eq!(pending(&fx), 0);
    assert!(contains(&archived, CAUSE));
    assert_eq!(fs::read_to_string(&ambiguous).unwrap(), alternative);
    fs::remove_file(ambiguous).unwrap();

    let lock = block_archive_cleanup(&fx);
    assert_archive_cleanup_failed(&run_submit(&fx, prompt));
    assert_eq!(pending(&fx), 1);
    assert!(contains(&archived, CAUSE));
    // Stage the source where a crash before the import's move-aside
    // would leave it. A retry must not convert it into a new entry.
    fs::rename(archived, source).unwrap();
    fs::remove_dir(lock).unwrap();
    assert!(submit(&fx, prompt).contains("already"));
    assert_eq!(pending(&fx), 0);
    sweep(&fx, &[CAUSE, FIX]);
    assert_eq!(fs::read_to_string(neighbor).unwrap(), kept_issue);
    let unrelated = json_output(&fx.ok(&["entry", "show", "issue-1", "--json"]));
    assert_eq!(unrelated["content"], KEPT);
}

#[test]
fn public_forgets_leave_no_copy_of_erased_text_in_the_store() {
    entry_routes();
    interleaved_entries();
    session_route(false);
    session_route(true);
    memory_route(false);
    memory_route(true);
    imported_issue_route();
}
