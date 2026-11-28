//! The archive tables (2_design.md §15.7, §15.9 phase 8): the files a
//! phase-7 move left, imported once by the next write and then removed,
//! and `forget` erasing from them as it erases from the live rows.

use super::{Seed, on_disk, private_tempdir, seed};
use crate::ProposalVerbs;
use crate::apps::store::MigrateTestWrites;
use crate::consistency::{Settle, Settled};
use crate::control::{self, Switch};
use crate::entry::{Cite, Entry};
use crate::session::SessionId;
use crate::store::{Store, codec, db};
use crate::testing::StoreTestInternals;
use scribe_store::fixtures::{T0, session};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const ARCHIVE: &str = "pre-sqlite-2026-10-01";
/// Words only the forgotten session's record holds.
const SESSION_WORDS: &str = "the forgotten session planned the offsite";
/// Words only the other session's record holds.
const KEPT_SESSION_WORDS: &str = "the kept session tuned the build cache";
/// Words only the forgotten entry holds.
const ENTRY_WORDS: &str = "the dashboard password rotates on fridays";
/// Words only the unrelated entry holds.
const KEPT_ENTRY_WORDS: &str = "the release train leaves on tuesdays";
/// A free-text extra a legacy audit line carried.
const AUDIT_EXTRA: &str = "said the offsite is in lisbon";

/// Write `bytes` at `rel` under `root`, creating owner-only directories.
fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    let mut missing = Vec::new();
    let mut dir = path.parent().unwrap().to_path_buf();
    while !dir.exists() {
        missing.push(dir.clone());
        dir = dir.parent().unwrap().to_path_buf();
    }
    for d in missing.iter().rev() {
        fs::create_dir(d).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    fs::write(&path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn id(s: &str) -> SessionId {
    s.parse().unwrap()
}

fn cite(session: &str) -> Cite {
    Cite {
        session: id(session),
        message: Some(String::from("m-1")),
        at: T0,
        prompt_sha256: "0".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    }
}

fn record(s: &str, words: &str) -> crate::SessionRecord {
    let mut r = session(s, T0);
    r.summary = words.to_owned();
    r
}

/// Daniel's entry `n`, typed in `session`.
fn entry(n: u32, session: &str, words: &str) -> Entry {
    let mut e = scribe_store::fixtures::entry(&format!("preferences-{n}"), T0);
    e.content = words.to_owned();
    e.said = format!("#scribe preferences: {words}");
    e.cite = Some(cite(session));
    e
}

/// A store with live sessions `s-1` and `s-2` and entries 1 to 3, and the
/// archive a phase-7 move left of the same records, a `control.json` and
/// an `audit.jsonl` whose first line concerns `s-1`.
fn store_with_archive(root: &Path) -> Store {
    let sessions = vec![
        record("s-1", SESSION_WORDS),
        record("s-2", KEPT_SESSION_WORDS),
    ];
    let entries = vec![
        entry(1, "s-1", "typed in the forgotten session"),
        entry(2, "s-2", KEPT_ENTRY_WORDS),
        entry(3, "s-2", ENTRY_WORDS),
    ];
    let store = seed(
        root,
        &Seed {
            sessions: sessions.clone(),
            entries: entries.clone(),
            ..Seed::default()
        },
    );
    for r in &sessions {
        let rel = format!("{ARCHIVE}/sessions/2026/11/{}.md", r.id);
        put(
            root,
            &rel,
            &codec::encode_session(&root.join(&rel), r).unwrap(),
        );
    }
    for e in &entries {
        let rel = format!("{ARCHIVE}/entries/preferences/{}.md", e.id);
        put(
            root,
            &rel,
            &codec::encode_entry(&root.join(&rel), e).unwrap(),
        );
    }
    put(
        root,
        &format!("{ARCHIVE}/control.json"),
        br#"{"format": 26, "codex_cursors": {}, "sessions": {"s-1": {"on": false, "due": false}, "s-2": {"on": false, "due": false}}}"#,
    );
    let audit = format!(
        "{}\n{}\n",
        serde_json::json!({
            "format": 3,
            "at": "2026-10-01T12:00:00Z",
            "command": "entry retire",
            "status": "success",
            "target": "preferences-1",
            "error_code": null,
            "author": "opus",
            "instruction_session": "s-1",
            "instruction_message": "m-1",
            "duration_ms": 5,
            "note": AUDIT_EXTRA,
        }),
        serde_json::json!({
            "format": 1,
            "at": "2026-09-26T12:00:00Z",
            "command": "hook stop",
            "status": "success",
            "target": null,
            "error_code": null,
            "duration_ms": 3,
            "note": "an unrelated extra",
        }),
    );
    put(root, &format!("{ARCHIVE}/audit.jsonl"), audit.as_bytes());
    store
}

/// Every archived record, by (path, line).
fn archived(root: &Path) -> BTreeMap<(String, u32), (String, Value)> {
    let conn = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
    conn.prepare("SELECT path, line, kind, record FROM archive_records")
        .unwrap()
        .query_map([], |row| {
            let record: String = row.get(3)?;
            Ok((
                (row.get(0)?, row.get(1)?),
                (row.get(2)?, serde_json::from_str(&record).unwrap()),
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn kinds(root: &Path) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for (kind, _) in archived(root).values() {
        *out.entry(kind.clone()).or_default() += 1;
    }
    out
}

fn holds(root: &Path, needle: &str) -> bool {
    archived(root)
        .values()
        .any(|(_, record)| record.to_string().contains(needle))
}

/// The next write imports the archive whole, checks its counts, and
/// removes the files; nothing reads the files again.
#[test]
fn the_next_write_imports_an_archive_once_and_removes_its_files() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = store_with_archive(&root);
    store
        .update_control(|s| control::switch(s, &id("s-9"), &Switch::On, T0))
        .unwrap();
    assert!(!root.join(ARCHIVE).exists(), "the files are removed");
    let expected: BTreeMap<String, usize> =
        [("audit", 2), ("control", 1), ("entry", 3), ("session", 2)]
            .into_iter()
            .map(|(k, n)| (k.to_owned(), n))
            .collect();
    assert_eq!(kinds(&root), expected);
    let session = &archived(&root)[&(String::from("sessions/2026/11/s-1.md"), 0)].1;
    assert_eq!(session["front"]["id"], "s-1");
    assert!(session["body"].as_str().unwrap().contains(SESSION_WORDS));
    let line = &archived(&root)[&(String::from("audit.jsonl"), 1)].1;
    assert_eq!(line["note"], AUDIT_EXTRA);

    // A second write imports nothing more.
    store
        .update_control(|s| control::switch(s, &id("s-9"), &Switch::Off, T0))
        .unwrap();
    assert_eq!(kinds(&root), expected);
    let err = store.migrate_store(T0).unwrap_err();
    assert_eq!(err.code(), "migrate_blocked", "{err:?}");
}

/// Forgetting a session erases its record and its words from the archive
/// and drops a legacy audit line's free-text extras, keeping the line's
/// audit fields; the other session's records are kept whole.
#[test]
fn forgetting_a_session_erases_it_from_the_archive_and_keeps_unrelated_records() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = store_with_archive(&root);
    let settled = store
        .settle(&Settle::ForgetSession(id("s-1")), &cite("s-2"), T0)
        .unwrap();
    assert!(
        matches!(settled, Settled::SessionForgotten(ref f) if f.files == 1),
        "{settled:?}"
    );
    assert!(!root.join(ARCHIVE).exists());

    let records = archived(&root);
    assert!(!records.contains_key(&(String::from("sessions/2026/11/s-1.md"), 0)));
    assert!(!holds(&root, SESSION_WORDS));
    assert!(!on_disk(&root, SESSION_WORDS), "no page keeps the record");
    assert!(
        holds(&root, KEPT_SESSION_WORDS),
        "the other session is kept"
    );
    assert!(holds(&root, KEPT_ENTRY_WORDS));
    let control = &records[&(String::from("control.json"), 0)].1;
    assert!(control["sessions"].get("s-1").is_none());
    assert!(control["sessions"].get("s-2").is_some());

    let line = &records[&(String::from("audit.jsonl"), 1)].1;
    assert_eq!(line["command"], "entry retire");
    assert_eq!(line["at"], "2026-10-01T12:00:00Z");
    assert_eq!(line["instruction_session"], "s-1");
    assert_eq!(line["duration_ms"], 5);
    assert!(line.get("note").is_none(), "{line}");
    assert!(!on_disk(&root, AUDIT_EXTRA));
    let other = &records[&(String::from("audit.jsonl"), 2)].1;
    assert_eq!(
        other["note"], "an unrelated extra",
        "an unrelated line is kept"
    );
}

/// Forgetting an entry erases its words from the archived entry, wherever
/// the archive holds them, and keeps every other archived entry.
#[test]
fn forgetting_an_entry_erases_its_text_from_the_archive_and_keeps_others() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = store_with_archive(&root);
    store
        .settle(
            &Settle::Forget("preferences-3".parse().unwrap()),
            &cite("s-2"),
            T0,
        )
        .unwrap();
    assert!(!root.join(ARCHIVE).exists());
    assert!(!holds(&root, ENTRY_WORDS));
    assert!(!on_disk(&root, ENTRY_WORDS), "no page keeps the words");
    assert!(holds(&root, KEPT_ENTRY_WORDS));
    assert!(holds(&root, KEPT_SESSION_WORDS));
    assert!(holds(&root, SESSION_WORDS));
    assert_eq!(kinds(&root)["audit"], 2);
}

/// A session forgotten while its archive was still files (before phase 8)
/// is erased from the archive as it is imported.
#[test]
fn an_archive_imported_after_a_forget_is_erased_on_import() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = store_with_archive(&root);
    let parked = dir.path().join("parked");
    fs::rename(root.join(ARCHIVE), &parked).unwrap();
    store
        .settle(&Settle::ForgetSession(id("s-1")), &cite("s-2"), T0)
        .unwrap();
    fs::rename(&parked, root.join(ARCHIVE)).unwrap();
    store
        .update_control(|s| control::switch(s, &id("s-9"), &Switch::On, T0))
        .unwrap();
    assert!(!root.join(ARCHIVE).exists());
    assert!(!holds(&root, SESSION_WORDS));
    assert!(!holds(&root, AUDIT_EXTRA));
    assert!(holds(&root, KEPT_SESSION_WORDS));
}
