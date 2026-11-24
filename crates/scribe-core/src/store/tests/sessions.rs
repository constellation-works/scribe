//! Sessions in `scribe.db` (§15.7): the move, `store migrate`, `export`,
//! `s-<n>` numbers and the pass as one transaction.

use super::{SESSION_BODY, on_disk, private_tempdir, session_text};
use crate::consistency::{Settle, Settled};
use crate::control::{self, Switch};
use crate::entry::{Cite, Speaker};
use crate::error::Error;
use crate::memory::{Checked, Draft, ElementKind, NowKind};
use crate::message::NewMessage;
use crate::session::{SessionFilter, SessionId, SessionRecord};
use crate::store::memory::CRASH_AFTER_COMMIT;
use crate::store::{ExportFormat, PassMemory, PassWrite, Redactor, Store, db, format};
use crate::tests::{T0, session};
use std::fs;
use std::path::Path;
use time::macros::date;

fn code(err: &Error) -> &'static str {
    err.code()
}

/// Write `text` at `rel` under `root`, creating owner-only directories.
fn put(root: &Path, rel: &str, text: &str) {
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
    fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

/// A file store holding the record `s-1` and no switch.
fn file_store() -> (tempfile::TempDir, Store) {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    put(
        &root,
        "sessions/2026/11/s-1.md",
        &session_text("", SESSION_BODY),
    );
    (dir, Store::open(root))
}

fn s1() -> SessionId {
    "s-1".parse().unwrap()
}

#[test]
fn migrate_moves_the_files_aside_and_the_records_read_the_same() {
    let (_dir, store) = file_store();
    let before = store.session(&s1()).unwrap().record;
    let moved = store.migrate_store(T0).unwrap();
    assert_eq!(moved.sessions, 1);
    assert_eq!(moved.moved_to.as_deref(), Some("pre-sqlite-2026-11-01"));
    let root = store.root();
    assert!(!root.join("sessions").exists());
    assert!(
        root.join("pre-sqlite-2026-11-01/sessions/2026/11/s-1.md")
            .is_file()
    );
    assert_eq!(store.session(&s1()).unwrap().record, before);
    // Once moved, a second migrate is refused and changes nothing.
    let err = store.migrate_store(T0).unwrap_err();
    assert_eq!(code(&err), "migrate_blocked", "{err}");
}

#[test]
fn migrate_is_refused_while_a_session_is_on() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    put(
        &root,
        "sessions/2026/11/s-1.md",
        &session_text("", SESSION_BODY),
    );
    put(
        &root,
        "control.json",
        r#"{"format": 1, "sessions": {"s-1": {"on": true, "since": "2026-11-01T09:00:00Z", "due": true, "final": false}}}"#,
    );
    let store = Store::open(&root);
    let err = store.migrate_store(T0).unwrap_err();
    assert_eq!(code(&err), "migrate_blocked", "{err}");
    assert!(root.join("sessions/2026/11/s-1.md").is_file());
    assert_eq!(store.session(&s1()).unwrap().record.title, "a session");
}

#[test]
fn migrate_is_refused_while_a_rebuild_checkpoint_exists() {
    let (_dir, store) = file_store();
    let root = store.root().to_path_buf();
    put(
        &root,
        "sessions/2026/11/.s-1.rebuild.md",
        &session_text("", SESSION_BODY),
    );
    let err = store.migrate_store(T0).unwrap_err();
    assert_eq!(code(&err), "migrate_blocked", "{err}");
    assert!(err.to_string().contains("checkpoint"), "{err}");
    assert!(root.join("sessions/2026/11/s-1.md").is_file());
    assert!(root.join("sessions/2026/11/.s-1.rebuild.md").is_file());
    assert_eq!(store.session(&s1()).unwrap().record.title, "a session");
}

#[test]
fn a_write_moves_the_files_and_keeps_a_checkpoint_as_a_row() {
    let (_dir, store) = file_store();
    let root = store.root().to_path_buf();
    put(
        &root,
        "sessions/2026/11/.s-1.rebuild.md",
        &session_text("", SESSION_BODY),
    );
    // A session write is never refused for the move.
    let other: SessionId = "s-2".parse().unwrap();
    store
        .update_control(|s| control::switch(s, &other, &Switch::On, T0))
        .unwrap();
    assert!(!root.join("sessions").exists());
    let moved = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().starts_with("pre-sqlite-"))
        .expect("the files moved aside");
    assert!(
        moved
            .path()
            .join("sessions/2026/11/.s-1.rebuild.md")
            .is_file()
    );
    let record = store.session(&s1()).unwrap().record;
    assert!(store.checkpoint_of(&record).unwrap().is_some());
}

#[test]
fn a_record_that_does_not_read_refuses_the_move() {
    let (_dir, store) = file_store();
    let root = store.root().to_path_buf();
    put(&root, "sessions/2026/11/s-9.md", "not a session\n");
    let err = store.migrate_store(T0).unwrap_err();
    assert_eq!(code(&err), "store_corrupt", "{err}");
    assert!(root.join("sessions/2026/11/s-1.md").is_file());
}

#[test]
fn records_number_by_creation_and_a_new_session_numbers_after_them() {
    let (_dir, store) = file_store();
    let before = store.session_numbers().unwrap();
    assert_eq!(before[&s1()].get(), 1);
    store.migrate_store(T0).unwrap();
    assert_eq!(store.session_numbers().unwrap(), before);
    let new: SessionId = "0b8ef4a6-0c6a-4d77-9c41-000000000001".parse().unwrap();
    store
        .update_control(|s| control::switch(s, &new, &Switch::On, T0))
        .unwrap();
    // Updates spend no number: the record and switch rewritten, then a
    // third session.
    store
        .update_control(|s| control::switch(s, &new, &Switch::Off, T0))
        .unwrap();
    let mut record = store.session(&s1()).unwrap().record;
    record.title = String::from("renamed");
    store
        .settle_pass(
            &s1(),
            Some(record.clone()),
            false,
            Some(record.updated),
            &Redactor::default(),
        )
        .unwrap();
    let third: SessionId = "0b8ef4a6-0c6a-4d77-9c41-000000000002".parse().unwrap();
    store
        .update_control(|s| control::switch(s, &third, &Switch::On, T0))
        .unwrap();
    let numbers = store.session_numbers().unwrap();
    assert_eq!(numbers[&new].get(), 2);
    assert_eq!(numbers[&third].get(), 3);
    // `s-<n>` names a session wherever an id is taken.
    let by_number: SessionId = "s-3".parse().unwrap();
    assert_eq!(store.resolve_session(&by_number).unwrap(), third);
}

#[test]
fn markdown_export_writes_every_session_in_its_file_shape() {
    let (dir, store) = file_store();
    store.migrate_store(T0).unwrap();
    let record = store.session(&s1()).unwrap().record;
    let target = dir.path().join("export");
    let out = store.export(&target, ExportFormat::Markdown).unwrap();
    assert_eq!(out.sessions, 1);
    assert_eq!(
        fs::read(target.join("sessions/2026/11/s-1.md")).unwrap(),
        format::encode_session(Path::new("record"), &record).unwrap()
    );
    assert!(target.join("control.json").is_file());
    // A directory with anything in it is refused.
    let err = store.export(&target, ExportFormat::Markdown).unwrap_err();
    assert_eq!(code(&err), "export_target_exists", "{err}");
}

#[test]
fn jsonl_export_writes_one_line_per_row_and_needs_moved_sessions() {
    let (dir, store) = file_store();
    let err = store
        .export(&dir.path().join("early"), ExportFormat::Jsonl)
        .unwrap_err();
    assert_eq!(code(&err), "export_needs_migrate", "{err}");
    store.migrate_store(T0).unwrap();
    let target = dir.path().join("export");
    let out = store.export(&target, ExportFormat::Jsonl).unwrap();
    assert_eq!(out.sessions, 1);
    let sessions = fs::read_to_string(target.join("sessions.jsonl")).unwrap();
    assert_eq!(sessions.lines().count(), 1, "{sessions}");
    let row: serde_json::Value = serde_json::from_str(sessions.lines().next().unwrap()).unwrap();
    assert_eq!(row["provider_session"], "s-1");
}

#[test]
fn jsonl_export_reads_phase_six_sessions_when_no_live_files_remain() {
    let (dir, store) = file_store();
    store.migrate_store(T0).unwrap();
    let root = store.root();
    let conn = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
    conn.execute_batch(db::REMOVE_GROUP_CAPTURE_SCHEMA).unwrap();
    conn.execute_batch(
        "DROP TABLE group_members; DROP TABLE group_seats; DROP TABLE groups;
         DROP TABLE entries;
         DROP TABLE claims;
         DROP TABLE agent_tags;
         DROP TABLE marks;
         DROP TABLE pickups;
         DROP TABLE audit;
         DROP TABLE pending_archive_forgets;
         DELETE FROM record_kinds WHERE kind <> 'sessions';
         CREATE TABLE record_kinds_v3 (
             kind TEXT NOT NULL PRIMARY KEY CHECK (kind IN ('sessions')),
             moved_at TEXT NOT NULL,
             moved_to TEXT
         ) STRICT;
         INSERT INTO record_kinds_v3 (kind, moved_at, moved_to)
             SELECT kind, moved_at, moved_to FROM record_kinds;
         DROP TABLE record_kinds;
         ALTER TABLE record_kinds_v3 RENAME TO record_kinds;
         PRAGMA user_version = 4;",
    )
    .unwrap();
    drop(conn);
    for stamp in ["proposals.json", "tags.json", "marks.json", "pickups.json"] {
        let _ = fs::remove_file(root.join(stamp));
    }
    put(
        root,
        "control.json",
        r#"{"format": 26, "sessions": {}, "codex_cursors": {}}"#,
    );

    let target = dir.path().join("export");
    let out = store.export(&target, ExportFormat::Jsonl).unwrap();
    assert_eq!(out.sessions, 1, "{out:?}");
    assert_eq!(out.entries, 0);
    assert_eq!(out.claims, 0);
    assert_eq!(out.audit_lines, 0);
    let sessions = fs::read_to_string(target.join("sessions.jsonl")).unwrap();
    assert_eq!(sessions.lines().count(), 1, "{sessions}");
    let row: serde_json::Value = serde_json::from_str(sessions.lines().next().unwrap()).unwrap();
    assert_eq!(row["provider_session"], "s-1");

    // A live document file beside the phase-six DB still requires migration.
    put(
        root,
        "entries/preferences/preferences-1.md",
        &super::entry_text(""),
    );
    let blocked = dir.path().join("blocked");
    let err = store.export(&blocked, ExportFormat::Jsonl).unwrap_err();
    assert_eq!(code(&err), "export_needs_migrate", "{err}");
    assert!(!blocked.exists());
}

fn draft(title: &str) -> Draft {
    const TURN: &str = "My sister's wedding is on december 12 and I'm still on the seating.";
    let day = date!(2026 - 12 - 12);
    Draft {
        checked: Checked {
            kind: ElementKind::Now,
            title: title.to_owned(),
            text: format!("{title}."),
            quote: TURN.to_owned(),
            message: String::from("u-1"),
            date: Some(crate::record::nullable_date::format(day)),
            now_kind: NowKind::Event,
            update: None,
            completes: None,
        },
        date: Some(day),
        turn: TURN.to_owned(),
        at: T0,
    }
}

fn memory(drafts: Vec<Draft>) -> PassMemory {
    PassMemory {
        drafts,
        signal: None,
        final_pass: false,
    }
}

const WEDDING: &str = "memory/now/2026-12-12-sister-s-wedding.md";

#[test]
fn a_pass_window_commits_its_record_and_memory_together_then_applies_the_journal() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let id: SessionId = "0b8ef4a6-0c6a-4d77-9c41-000000000001".parse().unwrap();
    let record = session(id.as_str(), T0);
    CRASH_AFTER_COMMIT.with(|c| c.set(true));
    let err = store
        .write_pass_window(
            record.clone(),
            None,
            PassWrite::Settle(false),
            &[],
            Some(&memory(vec![draft("Sister's wedding")])),
            &[],
            &redactor,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err:?}");
    // The crash came after the commit: the record and the memory rows are
    // in, the journal open, its file not yet written.
    assert_eq!(store.session(&id).unwrap().record, record);
    assert!(!store.root().join(WEDDING).exists());
    assert!(store.memory_view().unwrap().pending);
    // The next write replays the journal first.
    let mut next = record.clone();
    next.updated = T0 + time::Duration::minutes(1);
    store
        .write_pass_window(
            next,
            Some(record.updated),
            PassWrite::Settle(false),
            &[],
            Some(&memory(Vec::new())),
            &[],
            &redactor,
        )
        .unwrap();
    assert!(store.root().join(WEDDING).is_file());
    let view = store.memory_view().unwrap();
    assert!(!view.pending);
    assert_eq!(view.now.len(), 1);
}

#[test]
fn a_pass_window_that_fails_inside_its_transaction_writes_nothing() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let id: SessionId = "0b8ef4a6-0c6a-4d77-9c41-000000000001".parse().unwrap();
    store
        .update_control(|s| control::switch(s, &id, &Switch::On, T0))
        .unwrap();
    // The record's row is refused after the memory rows were staged.
    db::execute(
        store.root(),
        "CREATE TRIGGER staged_failure BEFORE UPDATE ON sessions WHEN NEW.has_record = 1
         BEGIN SELECT RAISE(ABORT, 'staged failure'); END;",
    )
    .unwrap();
    let err = store
        .write_pass_window(
            session(id.as_str(), T0),
            None,
            PassWrite::Settle(false),
            &[],
            Some(&memory(vec![draft("Sister's wedding")])),
            &[],
            &redactor,
        )
        .unwrap_err();
    assert_eq!(code(&err), "store_database", "{err}");
    let listed = store
        .list_sessions(&SessionFilter {
            include_inactive: true,
            ..SessionFilter::default()
        })
        .unwrap();
    assert!(listed.listing.items.is_empty());
    let view = store.memory_view().unwrap();
    assert!(!view.pending, "no journal committed");
    assert!(view.now.is_empty());
    assert!(!store.root().join(WEDDING).exists());
    assert!(
        store.control().unwrap().get(&id).unwrap().due,
        "the switch is unchanged"
    );
}

#[test]
fn session_forget_retires_single_source_memory_and_keeps_other_occurrences() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let first = session("first", T0);
    let second = session("second", T0 + time::Duration::hours(1));
    let sole = "Private forgotten source appointment";
    for (record, drafts) in [
        (&first, vec![draft(sole), draft("Shared event")]),
        (&second, vec![draft("Shared event")]),
    ] {
        store
            .write_pass_window(
                record.clone(),
                None,
                PassWrite::Settle(false),
                &[],
                Some(&memory(drafts)),
                &[],
                &redactor,
            )
            .unwrap();
    }
    let items = store.memory_view().unwrap().now;
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(
        items
            .iter()
            .find(|i| i.title == "Shared event")
            .unwrap()
            .sources
            .len(),
        2
    );
    let cite = Cite {
        session: second.id.clone(),
        message: None,
        at: T0,
        prompt_sha256: "0".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    };
    // Interruption after the transaction commits must not lose the erasure.
    CRASH_AFTER_COMMIT.with(|crash| crash.set(true));
    assert!(
        store
            .settle(&Settle::ForgetSession(first.id.clone()), &cite, T0)
            .is_err()
    );
    assert_eq!(
        store.refuse_forgotten(&first.id).unwrap_err().code(),
        "session_forgotten"
    );
    assert_eq!(
        store.memory_view().unwrap().now[0].sources,
        vec![second.id.clone()]
    );
    store
        .settle(&Settle::ForgetSession(first.id.clone()), &cite, T0)
        .unwrap();
    let items = store.memory_view().unwrap().now;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].title, "Shared event");
    assert_eq!(items[0].sources, vec![second.id.clone()]);
    let conn = rusqlite::Connection::open(store.root().join(db::DB_FILE)).unwrap();
    let count: u32 = conn
        .query_row(
            "SELECT count(*) FROM memory_messages WHERE session = ?1",
            [first.id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert!(!on_disk(store.root(), sole));
}

/// Plaintext an older `session rebuild` could store on a forgotten session.
const STALE_CHECKPOINT: &str = "forgotten-checkpoint-plaintext-9f3a";
/// Plaintext of a checkpoint that belongs to a session that was not forgotten.
const LIVE_CHECKPOINT: &str = "live-checkpoint-kept-4c1b";

struct ForgottenPair {
    dir: tempfile::TempDir,
    store: Store,
    forgotten: SessionRecord,
}

/// Two moved sessions: `s-1` forgotten, `s-2` with a real rebuild checkpoint.
fn forgotten_pair() -> ForgottenPair {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::new([]);
    let forgotten = session("s-1", T0);
    let live = session("s-2", T0 + time::Duration::hours(1));
    store
        .settle_pass(
            &forgotten.id,
            Some(forgotten.clone()),
            false,
            None,
            &redactor,
        )
        .unwrap();
    store
        .settle_pass(&live.id, Some(live.clone()), false, None, &redactor)
        .unwrap();
    let cite = Cite {
        session: forgotten.id.clone(),
        message: None,
        at: T0,
        prompt_sha256: "0".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    };
    store
        .settle(&Settle::ForgetSession(forgotten.id.clone()), &cite, T0)
        .unwrap();
    let _guard = store.lock_rebuild(&live).unwrap();
    let mut rebuilt = live.clone();
    rebuilt.summary = LIVE_CHECKPOINT.to_owned();
    store.checkpoint_rebuild(rebuilt, &redactor).unwrap();
    ForgottenPair {
        dir,
        store,
        forgotten,
    }
}

/// The pre-batch checkpoint writer: insert by provider id, with no forgotten check.
fn stage_legacy_checkpoint(root: &Path, session: &str, record: &str, user_version: Option<u32>) {
    let conn = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
    let inserted = conn
        .execute(
            "INSERT INTO rebuild_checkpoints (session, record, created)
             SELECT id, ?2, '2026-11-01T00:00:00Z' FROM sessions
             WHERE provider_session = ?1",
            (session, record),
        )
        .unwrap();
    assert_eq!(inserted, 1, "the legacy writer found the session row");
    if let Some(version) = user_version {
        conn.execute_batch(db::REMOVE_GROUP_CAPTURE_SCHEMA).unwrap();
        conn.execute_batch(
            "ALTER TABLE entries DROP COLUMN decision; ALTER TABLE entries DROP COLUMN item;",
        )
        .unwrap();
        if version == 6 {
            // Remove the schema-9 group tables and schema-7 entry column
            // before replaying the upgrades from version 6.
            conn.execute_batch(
                "DROP TABLE group_members; DROP TABLE group_seats; DROP TABLE groups;
                 ALTER TABLE entries DROP COLUMN handoff_mode;",
            )
            .unwrap();
        }
        conn.pragma_update(None, "user_version", version).unwrap();
    }
}

fn checkpoint_record(root: &Path, session: &str) -> Option<String> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join(db::DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    match conn.query_row(
        "SELECT c.record FROM rebuild_checkpoints c
         JOIN sessions s ON s.id = c.session
         WHERE s.provider_session = ?1",
        [session],
        |row| row.get(0),
    ) {
        Ok(text) => Some(text),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(err) => panic!("read checkpoint: {err}"),
    }
}

fn user_version(root: &Path) -> u32 {
    db::meta(root).unwrap().unwrap().0
}

fn jsonl_text(store: &Store, target: &Path) -> String {
    store.export(target, ExportFormat::Jsonl).unwrap();
    let mut out = String::new();
    for entry in fs::read_dir(target).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            out.push_str(&fs::read_to_string(&path).unwrap());
        }
    }
    out
}

#[test]
fn jsonl_export_omits_a_preexisting_forgotten_checkpoint_until_upgrade_erases_it() {
    let pair = forgotten_pair();
    let root = pair.store.root().to_path_buf();
    // Schema 6 predates handoff_mode and can still hold the row. Version 8 deletes it.
    stage_legacy_checkpoint(&root, "s-1", STALE_CHECKPOINT, Some(6));
    assert_eq!(user_version(&root), 6);
    let stale = checkpoint_record(&root, "s-1").unwrap();
    assert!(stale.contains(STALE_CHECKPOINT));
    let live = checkpoint_record(&root, "s-2").unwrap();
    assert!(live.contains(LIVE_CHECKPOINT));

    let exported = jsonl_text(&pair.store, &pair.dir.path().join("export"));
    assert!(
        !exported.contains(STALE_CHECKPOINT),
        "JSONL export included the forgotten checkpoint before any later write: {exported}"
    );
    assert!(
        exported.contains(LIVE_CHECKPOINT),
        "JSONL export dropped the live session's checkpoint: {exported}"
    );
    // The export is a read: the row and the schema version are unchanged.
    assert_eq!(user_version(&root), 6);
    assert!(
        checkpoint_record(&root, "s-1")
            .unwrap()
            .contains(STALE_CHECKPOINT)
    );
    assert_eq!(
        pair.store.rebuild_checkpoint(&pair.forgotten).unwrap(),
        None,
        "a forgotten session's checkpoint is not readable"
    );

    pair.store
        .write_messages(
            &[NewMessage {
                session: "s-9".parse().unwrap(),
                message: String::from("m-unrelated"),
                role: Speaker::User,
                at: T0,
                text: String::from("an unrelated note"),
            }],
            &Redactor::default(),
        )
        .unwrap();
    assert_eq!(user_version(&root), 19);
    assert_eq!(checkpoint_record(&root, "s-1"), None);
    assert!(
        checkpoint_record(&root, "s-2")
            .unwrap()
            .contains(LIVE_CHECKPOINT)
    );
    assert!(
        !on_disk(&root, STALE_CHECKPOINT),
        "the stale plaintext is still in the store"
    );
    assert!(on_disk(&root, LIVE_CHECKPOINT));
}

#[test]
fn forgetting_again_erases_a_stale_checkpoint_and_keeps_a_live_one() {
    let pair = forgotten_pair();
    let root = pair.store.root().to_path_buf();
    let version = user_version(&root);
    stage_legacy_checkpoint(&root, "s-1", STALE_CHECKPOINT, None);
    assert!(
        checkpoint_record(&root, "s-1")
            .unwrap()
            .contains(STALE_CHECKPOINT)
    );
    let exported = jsonl_text(&pair.store, &pair.dir.path().join("export"));
    assert!(!exported.contains(STALE_CHECKPOINT), "{exported}");
    assert!(exported.contains(LIVE_CHECKPOINT), "{exported}");
    assert!(
        checkpoint_record(&root, "s-1")
            .unwrap()
            .contains(STALE_CHECKPOINT)
    );

    let cite = Cite {
        session: pair.forgotten.id.clone(),
        message: None,
        at: T0,
        prompt_sha256: "0".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    };
    let again = pair
        .store
        .settle(&Settle::ForgetSession(pair.forgotten.id.clone()), &cite, T0)
        .unwrap();
    assert!(
        matches!(again, Settled::SessionForgetRepaired { .. }),
        "a repeated forget still reports already forgotten: {again:?}"
    );
    assert_eq!(user_version(&root), version);
    assert_eq!(checkpoint_record(&root, "s-1"), None);
    assert!(
        checkpoint_record(&root, "s-2")
            .unwrap()
            .contains(LIVE_CHECKPOINT)
    );
    assert!(!on_disk(&root, STALE_CHECKPOINT));
    assert!(on_disk(&root, LIVE_CHECKPOINT));
}

#[test]
fn schema_eight_cleanup_retries_after_a_reader_releases_the_wal() {
    let pair = forgotten_pair();
    let root = pair.store.root().to_path_buf();
    stage_legacy_checkpoint(&root, "s-1", STALE_CHECKPOINT, Some(6));
    let reader = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _: i64 = reader
        .query_row("SELECT count(*) FROM rebuild_checkpoints", [], |r| r.get(0))
        .unwrap();
    let store = Store::open(&root).with_lock_wait(std::time::Duration::from_millis(50));
    let write = || {
        store.write_messages(
            &[NewMessage {
                session: "s-9".parse().unwrap(),
                message: String::from("retry"),
                role: Speaker::User,
                at: T0,
                text: String::from("unrelated"),
            }],
            &Redactor::default(),
        )
    };
    let exported = jsonl_text(&store, &pair.dir.path().join("before-retry"));
    assert!(!exported.contains(STALE_CHECKPOINT));
    assert!(exported.contains(LIVE_CHECKPOINT));
    assert!(write().is_err(), "a reader must prevent completing cleanup");
    let exported = jsonl_text(&store, &pair.dir.path().join("during-retry"));
    assert!(!exported.contains(STALE_CHECKPOINT));
    assert!(exported.contains(LIVE_CHECKPOINT));
    reader.execute_batch("ROLLBACK").unwrap();
    drop(reader);
    write().unwrap();
    assert_eq!(checkpoint_record(&root, "s-1"), None);
    assert!(
        checkpoint_record(&root, "s-2")
            .unwrap()
            .contains(LIVE_CHECKPOINT)
    );
    assert!(!on_disk(&root, STALE_CHECKPOINT));
    let exported = jsonl_text(&store, &pair.dir.path().join("after-retry"));
    assert!(!exported.contains(STALE_CHECKPOINT));
    assert!(exported.contains(LIVE_CHECKPOINT));
}

#[test]
fn schema_nine_retries_cleanup_left_by_an_older_writer() {
    let pair = forgotten_pair();
    let root = pair.store.root().to_path_buf();
    stage_legacy_checkpoint(&root, "s-1", STALE_CHECKPOINT, Some(9));
    let reader = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _: i64 = reader
        .query_row("SELECT count(*) FROM rebuild_checkpoints", [], |r| r.get(0))
        .unwrap();
    let old_writer = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
    old_writer.busy_timeout(std::time::Duration::ZERO).unwrap();
    old_writer.execute_batch("PRAGMA secure_delete = ON; DELETE FROM rebuild_checkpoints WHERE session IN (SELECT id FROM sessions WHERE forgotten IS NOT NULL);").unwrap();
    let busy: i64 = old_writer
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
        .unwrap();
    assert_ne!(busy, 0);
    drop(old_writer);
    assert!(on_disk(&root, STALE_CHECKPOINT));
    let store = Store::open(&root).with_lock_wait(std::time::Duration::from_millis(50));
    let write = || {
        store.write_messages(
            &[NewMessage {
                session: "s-9".parse().unwrap(),
                message: String::from("retry"),
                role: Speaker::User,
                at: T0,
                text: String::from("unrelated"),
            }],
            &Redactor::default(),
        )
    };
    assert!(write().is_err());
    assert_eq!(user_version(&root), 9);
    let exported = jsonl_text(&store, &pair.dir.path().join("old-before"));
    assert!(!exported.contains(STALE_CHECKPOINT));
    assert!(exported.contains(LIVE_CHECKPOINT));
    reader.execute_batch("ROLLBACK").unwrap();
    drop(reader);
    write().unwrap();
    assert_eq!(user_version(&root), 19);
    assert!(!on_disk(&root, STALE_CHECKPOINT));
    assert!(
        checkpoint_record(&root, "s-2")
            .unwrap()
            .contains(LIVE_CHECKPOINT)
    );
}

fn unrelated_write(store: &Store) -> crate::error::Result<()> {
    let other = session("s-3", T0 + time::Duration::hours(2));
    store
        .settle_pass(
            &other.id,
            Some(other.clone()),
            false,
            None,
            &Redactor::default(),
        )
        .map(|_| ())
}

#[test]
fn unrelated_write_without_forgets_does_not_read_session_payloads() {
    let (_dir, store) = file_store();
    store.migrate_store(T0).unwrap();
    let conn = rusqlite::Connection::open(store.root().join(db::DB_FILE)).unwrap();
    // Invalid UTF-8 makes fetching segment text fail, while SQLite can still
    // store it as TEXT. This detects a payload read without timing assumptions.
    let changed = conn
        .execute("UPDATE segments SET text = CAST(x'80' AS TEXT)", [])
        .unwrap();
    assert!(changed > 0, "fixture must contain a segment");
    drop(conn);
    assert!(
        store.load_sessions().is_err(),
        "the poisoned text detects a full load"
    );
    store
        .with_conn(|conn, path| {
            conn.execute("UPDATE meta SET created = created", [])
                .map_err(|e| db::db_error("update", path, e))?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn an_unrelated_write_ignores_the_archive_lock_when_no_archive_file_is_left() {
    use crate::store::{LOCKS_DIR, fsio};
    use std::time::Duration;

    let pair = forgotten_pair();
    let root = pair.store.root().to_path_buf();
    let store = Store::open(&root).with_lock_wait(Duration::from_millis(50));
    let lock_path = store.lock_path("archive-forget.lock").unwrap();
    assert!(lock_path.starts_with(root.join(LOCKS_DIR)));
    let _held =
        fsio::LockGuard::acquire(&lock_path, "other forget", Duration::from_secs(1)).unwrap();
    unrelated_write(&store).unwrap();
    let ids = store.load_sessions().unwrap().numbers;
    assert!(
        ids.contains_key(&"s-3".parse().unwrap()),
        "the write committed"
    );
}

#[test]
fn archive_erasure_stays_serialized_and_retries_after_the_lock_is_released() {
    use crate::store::fsio;
    use std::time::Duration;

    let pair = forgotten_pair();
    let second: SessionId = "s-2".parse().unwrap();
    let cite = Cite {
        session: second.clone(),
        message: None,
        at: T0,
        prompt_sha256: "0".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    };
    pair.store
        .settle(&Settle::ForgetSession(second), &cite, T0)
        .unwrap();
    let root = pair.store.root().to_path_buf();
    let archived = "pre-sqlite-2026-11-01/sessions/2026/11/s-1.md";
    put(&root, archived, "forgotten plaintext");
    let checkpoint = "pre-sqlite-2026-11-02/sessions.2/2025/12/.s-1.rebuild.md";
    let second = "pre-sqlite-2026-11-02/sessions.2/2025/12/s-2.md";
    let live = "pre-sqlite-2026-11-02/sessions.2/2025/12/s-4.md";
    put(&root, checkpoint, "forgotten checkpoint");
    put(&root, second, "another forgotten session");
    put(&root, live, "live session");
    let store = Store::open(&root).with_lock_wait(Duration::from_millis(50));
    let lock_path = store.lock_path("archive-forget.lock").unwrap();
    let held =
        fsio::LockGuard::acquire(&lock_path, "other forget", Duration::from_secs(1)).unwrap();
    let err = unrelated_write(&store).unwrap_err();
    assert_eq!(code(&err), "store_locked");
    assert!(root.join(archived).exists(), "no erasure under a held lock");
    drop(held);
    store.clean_forgotten_session_files().unwrap();
    assert!(!root.join(archived).exists());
    assert!(!root.join(checkpoint).exists());
    assert!(!root.join(second).exists());
    assert!(root.join(live).exists());
}

#[test]
fn a_first_write_forget_counts_the_legacy_files_it_deletes() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let record = session_text("", SESSION_BODY);
    let other = record.replace("id: s-1", "id: s-2");
    put(&root, "sessions/2026/11/s-1.md", &record);
    put(&root, "sessions/2026/11/.s-1.rebuild.md", &record);
    put(&root, "sessions/2026/11/s-2.md", &other);
    let store = Store::open(&root);
    let cite = Cite {
        session: s1(),
        message: None,
        at: T0,
        prompt_sha256: "0".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    };

    let settled = store
        .settle(&Settle::ForgetSession(s1()), &cite, T0)
        .unwrap();

    let Settled::SessionForgotten(forgot) = settled else {
        panic!("expected a forgotten session, got {settled:?}");
    };
    assert_eq!(forgot.id, s1());
    assert_eq!(
        forgot.files, 2,
        "the record and the checkpoint the cleanup deleted"
    );
    let archive = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("pre-sqlite-"))
        })
        .expect("the first write moved the files aside");
    let moved = archive.join("sessions/2026/11");
    assert!(!moved.join("s-1.md").exists());
    assert!(!moved.join(".s-1.rebuild.md").exists());
    assert!(moved.join("s-2.md").is_file(), "the other session is kept");
    assert!(store.session(&"s-2".parse().unwrap()).is_ok());
}

#[test]
fn resolve_with_extra_ids_expands_unique_prefixes_and_refuses_ambiguity() {
    let (_dir, store) = file_store();
    let stored: SessionId = "0b8ef4a6-0c6a-4d77-9c41-000000000002".parse().unwrap();
    store
        .update_control(|s| control::switch(s, &stored, &Switch::On, T0))
        .unwrap();
    let extra_only = String::from("9999aaaa-1111-4222-8333-444455556666");
    let colliding_extra = String::from("0b8ef4a6-ffff-4000-8000-000000000099");
    let extra = vec![
        extra_only.clone(),
        colliding_extra.clone(),
        stored.to_string(),
    ];

    // Extra-only id resolves from a 4+ char prefix.
    let prefix: SessionId = "9999aaaa".parse().unwrap();
    assert_eq!(
        store
            .resolve_session_or_typed_with(&prefix, &extra)
            .unwrap(),
        extra_only.parse::<SessionId>().unwrap()
    );
    let rel: crate::Related = "session:9999aaaa".parse().unwrap();
    assert_eq!(
        store.resolve_relateds_with(&[rel], &extra).unwrap(),
        vec![format!("session:{extra_only}").parse().unwrap()]
    );

    // Same id in both the store and `extra_ids` deduplicates without ambiguity.
    let stored_prefix: SessionId = "0b8ef4a6-0c6a".parse().unwrap();
    assert_eq!(
        store
            .resolve_session_or_typed_with(&stored_prefix, &extra)
            .unwrap(),
        stored
    );

    // Shared prefix across a store session (`stored`) and `extra_ids` (`colliding_extra`) is ambiguous.
    let ambig: SessionId = "0b8ef4a6".parse().unwrap();
    let err = store
        .resolve_session_or_typed_with(&ambig, &extra)
        .unwrap_err();
    assert_eq!(code(&err), "id_ambiguous");
    let rel_ambig: crate::Related = "session:0b8ef4a6".parse().unwrap();
    let err = store
        .resolve_relateds_with(&[rel_ambig], &extra)
        .unwrap_err();
    assert_eq!(code(&err), "id_ambiguous");

    // Unknown 4+ char prefix stays as typed.
    let unknown: SessionId = "deadbeef".parse().unwrap();
    assert_eq!(
        store
            .resolve_session_or_typed_with(&unknown, &extra)
            .unwrap(),
        unknown
    );
}

#[test]
fn typed_metadata_update_changes_only_the_named_record_fields() {
    let (_dir, store) = file_store();
    let id = s1();
    let before = store.session(&id).unwrap().record;
    store
        .update_session_metadata(&id, "project", "water-economy", &Redactor::default())
        .unwrap();
    store
        .update_session_metadata(&id, "title", "Water economy study", &Redactor::default())
        .unwrap();
    let after = store.session(&id).unwrap().record;
    assert_eq!(after.project.as_str(), "water-economy");
    assert_eq!(after.title, "Water economy study");
    assert_eq!(after.log, before.log);
    assert!(
        store
            .update_session_metadata(&id, "project", "bad project", &Redactor::default())
            .is_err()
    );
    assert_eq!(
        store.session(&id).unwrap().record.project.as_str(),
        "water-economy"
    );
}

/// An active `open-item` entry whose answer was typed in `session`.
fn answered_item(id: &str, session: &str, words: &str) -> crate::Entry {
    let mut item = crate::tests::entry(id, T0);
    item.content = String::from("Restart the dashboard");
    item.author = crate::Author::agent("0-chief".parse().unwrap());
    item.content_from = crate::ContentFrom::Agent;
    item.cite = None;
    item.said.clear();
    item.item = Some(crate::OpenItem {
        asked_by: "0-chief".parse().unwrap(),
        options: Vec::new(),
        recommended: None,
        why: None,
        blocked: String::from("the restart"),
        answer_in: crate::AnswerIn::Any,
        answer: Some(crate::ItemAnswer {
            words: words.to_owned(),
            session: session.parse().unwrap(),
            at: T0,
            recorded_by: "0-aide".parse().unwrap(),
        }),
        closed: None,
    });
    item
}

#[test]
fn forgetting_a_session_erases_its_open_item_answers_live_and_archived() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let record = session_text("", SESSION_BODY);
    put(&root, "sessions/2026/11/s-1.md", &record);
    put(
        &root,
        "sessions/2026/11/s-2.md",
        &record.replace("id: s-1", "id: s-2"),
    );
    let gone = "restart it after the freeze lifts";
    let kept = "leave the dashboard alone for now";
    for (id, session, words) in [("open-item-1", "s-1", gone), ("open-item-2", "s-2", kept)] {
        let path = root.join(format!("entries/open-item/{id}.md"));
        let bytes = format::encode_entry(&path, &answered_item(id, session, words)).unwrap();
        put(
            &root,
            &format!("entries/open-item/{id}.md"),
            std::str::from_utf8(&bytes).unwrap(),
        );
    }
    let store = Store::open(&root);
    let cite = Cite {
        session: s1(),
        message: None,
        at: T0,
        prompt_sha256: "0".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    };

    store
        .settle(&Settle::ForgetSession(s1()), &cite, T0)
        .unwrap();

    let forgotten = store.resolve_entry("open-item-1").unwrap();
    assert_eq!(forgotten.item.as_ref().unwrap().answer, None);
    assert_eq!(
        crate::ItemState::of(&forgotten),
        Some(crate::ItemState::Open)
    );
    let unrelated = store.resolve_entry("open-item-2").unwrap();
    assert_eq!(unrelated.item.unwrap().answer.unwrap().words, kept);
    assert!(!on_disk(&root, gone), "the forgotten words remain on disk");
    assert!(on_disk(&root, kept));
    let answer = crate::ItemAnswer {
        words: gone.to_owned(),
        session: s1(),
        at: T0,
        recorded_by: "0-aide".parse().unwrap(),
    };
    let refused = store
        .answer_open_item("open-item-1", answer, T0, &Redactor::new([]))
        .unwrap_err();
    assert_eq!(code(&refused), "session_forgotten");
    assert!(!on_disk(&root, gone));
}
