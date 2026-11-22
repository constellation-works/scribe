//! `scribe.db`: privacy, the schema, upserts, redaction on write, FTS5
//! search, the read-only copy, and WAL with `busy_timeout` under concurrent
//! writer processes (a harness of child processes of this test binary,
//! never load on a real host).

use super::super::db::{self, DB_FILE};
use super::private_tempdir;
use crate::entry::Speaker;
use crate::error::Error;
use crate::message::NewMessage;
use crate::store::{Redactor, Store};
use crate::tests::T0;
use std::path::Path;
use std::time::Duration;

const SECRET: &str = "sk-live-0123456789abcdef";

fn message(session: &str, id: &str, text: &str) -> NewMessage {
    NewMessage {
        session: session.parse().unwrap(),
        message: id.to_owned(),
        role: Speaker::User,
        at: T0,
        text: text.to_owned(),
    }
}

fn tokens(text: &str) -> Vec<String> {
    crate::ask::tokens(text)
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn the_first_write_creates_an_owner_only_wal_database_at_the_current_schema() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    assert_eq!(db::meta(dir.path()).unwrap(), None);
    let written = store
        .write_messages(
            &[message("s-1", "m-1", "merge queue first")],
            &Redactor::default(),
        )
        .unwrap();
    assert_eq!(written, 1);
    let (version, store_id) = db::meta(dir.path()).unwrap().unwrap();
    assert_eq!(version, 2);
    assert_eq!(store_id.len(), 32);
    // PERSIST_WAL: both companions outlive the last connection.
    for name in [DB_FILE, "scribe.db-wal", "scribe.db-shm"] {
        let path = dir.path().join(name);
        assert!(path.exists(), "{name} is missing");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600, "{name}");
    }
}

#[test]
fn a_window_written_again_changes_no_row_and_a_new_text_replaces_the_old() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let redactor = Redactor::default();
    let window = [
        message("s-1", "m-1", "squash agent pull requests"),
        message("s-1", "m-2", "keep the merge queue"),
    ];
    assert_eq!(store.write_messages(&window, &redactor).unwrap(), 2);
    assert_eq!(store.write_messages(&window, &redactor).unwrap(), 0);
    let found = store.search_messages(&tokens("merge")).unwrap();
    assert_eq!(found.matches.len(), 1);
    assert!(!found.read_only_copy);

    let edited = [message("s-1", "m-2", "drop the release train")];
    assert_eq!(store.write_messages(&edited, &redactor).unwrap(), 1);
    assert!(
        store
            .search_messages(&tokens("merge"))
            .unwrap()
            .matches
            .is_empty()
    );
    let found = store.search_messages(&tokens("release")).unwrap();
    assert_eq!(found.matches.len(), 1);
    assert_eq!(found.matches[0].message, "m-2");
    // The same message id in another session is another row.
    store
        .write_messages(&[message("s-2", "m-2", "release notes")], &redactor)
        .unwrap();
    assert_eq!(
        store
            .search_messages(&tokens("release"))
            .unwrap()
            .matches
            .len(),
        2
    );
}

#[test]
fn search_ranks_by_bm25_and_matches_a_stem_as_a_word_prefix() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    store
        .write_messages(
            &[
                message("s-1", "m-1", "merging is fine"),
                message("s-1", "m-2", "merge merge merge, the merge queue"),
                message("s-1", "m-3", "nothing here"),
            ],
            &Redactor::default(),
        )
        .unwrap();
    let found = store.search_messages(&tokens("merges")).unwrap();
    let ids: Vec<&str> = found.matches.iter().map(|m| m.message.as_str()).collect();
    assert_eq!(ids, ["m-2", "m-1"]);
    assert!(found.matches[0].score > found.matches[1].score);
    assert!(store.search_messages(&[]).unwrap().matches.is_empty());
}

#[test]
fn messages_are_redacted_before_they_are_written() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let redactor = Redactor::new([(String::from("API_KEY"), String::from(SECRET))]);
    store
        .write_messages(
            &[message(
                "s-1",
                "m-1",
                &format!("deploy with {SECRET} tonight"),
            )],
            &redactor,
        )
        .unwrap();
    let found = store.search_messages(&tokens("deploy")).unwrap();
    assert_eq!(
        found.matches[0].text,
        "deploy with [redacted:API_KEY] tonight"
    );
    // Nothing on disk holds the value, the WAL included.
    for name in [DB_FILE, "scribe.db-wal"] {
        let bytes = std::fs::read(dir.path().join(name)).unwrap();
        assert!(
            !bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
            "{name} holds the secret"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_database_open_to_others_or_reached_through_a_link_is_refused_on_load() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    store
        .write_messages(
            &[message("s-1", "m-1", "merge queue")],
            &Redactor::default(),
        )
        .unwrap();
    for name in [DB_FILE, "scribe.db-wal", "scribe.db-shm"] {
        let path = dir.path().join(name);
        chmod(&path, 0o640);
        let read = store.search_messages(&tokens("merge")).unwrap_err();
        assert!(
            matches!(read, Error::Permissions { .. }),
            "{name}: {read:?}"
        );
        let write = store
            .write_messages(&[message("s-1", "m-2", "more")], &Redactor::default())
            .unwrap_err();
        assert!(
            matches!(write, Error::Permissions { .. }),
            "{name}: {write:?}"
        );
        chmod(&path, 0o600);
    }

    let other = private_tempdir();
    std::os::unix::fs::symlink(dir.path().join(DB_FILE), other.path().join(DB_FILE)).unwrap();
    let linked = Store::open(other.path());
    let err = linked.search_messages(&tokens("merge")).unwrap_err();
    assert!(matches!(err, Error::Symlink { .. }), "{err:?}");
}

#[test]
fn a_database_from_a_newer_build_is_refused() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    store
        .write_messages(
            &[message("s-1", "m-1", "merge queue")],
            &Redactor::default(),
        )
        .unwrap();
    db::execute(dir.path(), "PRAGMA user_version = 99").unwrap();
    let err = store.search_messages(&tokens("merge")).unwrap_err();
    assert!(
        matches!(err, Error::NewerFormat { found: 99, .. }),
        "{err:?}"
    );
    let err = store
        .write_messages(&[message("s-1", "m-2", "more")], &Redactor::default())
        .unwrap_err();
    assert!(
        matches!(err, Error::NewerFormat { found: 99, .. }),
        "{err:?}"
    );
}

#[test]
fn the_memory_tables_hold_their_constraints() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    store
        .write_messages(&[message("s-1", "m-1", "hackathon")], &Redactor::default())
        .unwrap();
    for n in 1..=5 {
        db::execute(
            dir.path(),
            &format!(
                "INSERT INTO now_pending (session, message, subject, text, created)
                 VALUES ('s-1', 'm-{n}', 'subject {n}', 'joining', '2026-11-01T10:00:00Z')"
            ),
        )
        .unwrap();
    }
    let sixth = db::execute(
        dir.path(),
        "INSERT INTO now_pending (session, message, subject, text, created)
         VALUES ('s-1', 'm-6', 'subject 6', 'joining', '2026-11-01T10:00:00Z')",
    );
    assert!(sixth.is_err(), "a sixth pending candidate was kept");
    db::execute(
        dir.path(),
        "INSERT INTO now_pending (session, message, subject, text, created)
         VALUES ('s-2', 'm-1', 'subject 1', 'joining', '2026-11-01T10:00:00Z')",
    )
    .unwrap();
    for bad in [
        "INSERT INTO memory_history (item, kind, text, sources, removed_at) VALUES ('m-1', 'now', 't', '{}', 'x')",
        "INSERT INTO profile_revisions (created, content, file_sha256) VALUES ('x', 'c', 'short')",
        "INSERT INTO profile_cursor (id, through_at, through_session) VALUES (2, 'x', 's-1')",
        "INSERT INTO messages (session, message, role, at, text, sha256) VALUES ('s-1', 'm-9', 'system', 'x', 't', 'x')",
        "INSERT INTO meta (id, store_id, created) VALUES (2, 'x', 'x')",
    ] {
        assert!(db::execute(dir.path(), bad).is_err(), "accepted: {bad}");
    }
    db::execute(
        dir.path(),
        "INSERT INTO memory_history (item, kind, text, sources, removed_at)
         VALUES ('m-1', 'working', 'run the requeue', '[\"s-1\"]', '2026-11-01T10:00:00Z')",
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn a_read_only_root_reads_through_its_wal_files_or_else_as_a_read_only_copy() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    store
        .write_messages(
            &[message("s-1", "m-1", "merge queue")],
            &Redactor::default(),
        )
        .unwrap();

    // With the persisted -wal and -shm files, a read-only root reads normally.
    chmod(dir.path(), 0o500);
    let found = store.search_messages(&tokens("merge"));
    chmod(dir.path(), 0o700);
    let found = found.unwrap();
    assert_eq!(found.matches.len(), 1);
    assert!(!found.read_only_copy);

    // A copy without them cannot open the WAL, so it reads immutable.
    let copy = private_tempdir();
    std::fs::copy(dir.path().join(DB_FILE), copy.path().join(DB_FILE)).unwrap();
    chmod(&copy.path().join(DB_FILE), 0o600);
    chmod(copy.path(), 0o500);
    let found = Store::open(copy.path()).search_messages(&tokens("merge"));
    let left: Vec<_> = std::fs::read_dir(copy.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    chmod(copy.path(), 0o700);
    let found = found.unwrap();
    assert_eq!(found.matches.len(), 1);
    assert!(found.read_only_copy);
    assert_eq!(
        left,
        [std::ffi::OsString::from(DB_FILE)],
        "the reader wrote"
    );
}

#[test]
fn a_writer_held_past_the_lock_wait_is_locked() {
    let dir = private_tempdir();
    let store = Store::open(dir.path()).with_lock_wait(Duration::from_millis(150));
    store
        .write_messages(&[message("s-1", "m-1", "merge")], &Redactor::default())
        .unwrap();
    let mut holder = db::open_writer(dir.path(), Duration::from_secs(5)).unwrap();
    let tx = holder
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let err = store
        .write_messages(&[message("s-1", "m-2", "more")], &Redactor::default())
        .unwrap_err();
    assert!(matches!(err, Error::Locked { .. }), "{err:?}");
    assert_eq!(err.code(), "store_locked");
    // A reader is never blocked by a writer (WAL).
    assert_eq!(
        store
            .search_messages(&tokens("merge"))
            .unwrap()
            .matches
            .len(),
        1
    );
    drop(tx);
    store
        .write_messages(&[message("s-1", "m-2", "more")], &Redactor::default())
        .unwrap();
}

const CHILD_ROOT: &str = "SCRIBE_TEST_DB_CHILD_ROOT";
const CHILD_ID: &str = "SCRIBE_TEST_DB_CHILD_ID";
const WRITERS: usize = 6;
const WINDOWS: usize = 10;

/// One writer of the harness below: a no-op unless it was started as a
/// child with the root in its environment. Each window is its own
/// connection and transaction, as each hook process's pass is.
#[test]
fn child_writes_windows() {
    let (Ok(root), Ok(id)) = (std::env::var(CHILD_ROOT), std::env::var(CHILD_ID)) else {
        return;
    };
    let store = Store::open(Path::new(&root)).with_lock_wait(Duration::from_secs(20));
    for w in 0..WINDOWS {
        let window: Vec<NewMessage> = (0..3)
            .map(|n| {
                message(
                    &format!("s-{id}"),
                    &format!("m-{w}-{n}"),
                    "merge queue turn",
                )
            })
            .collect();
        store.write_messages(&window, &Redactor::default()).unwrap();
    }
}

#[test]
fn concurrent_writer_processes_all_commit_under_wal_and_busy_timeout() {
    let dir = private_tempdir();
    let exe = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..WRITERS)
        .map(|id| {
            std::process::Command::new(&exe)
                .args([
                    "--exact",
                    "store::tests::db::child_writes_windows",
                    "--nocapture",
                    "--test-threads",
                    "1",
                ])
                .env(CHILD_ROOT, dir.path())
                .env(CHILD_ID, id.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "a writer failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let found = Store::open(dir.path())
        .search_messages(&tokens("merge"))
        .unwrap();
    assert_eq!(found.matches.len(), WRITERS * WINDOWS * 3);
    assert_eq!(db::meta(dir.path()).unwrap().unwrap().0, 2);
}

#[test]
fn a_forget_deletes_its_messages_and_leaves_no_trace_in_the_database_or_its_wal() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let gone = "zanzibar holiday plans";
    store
        .write_messages(
            &[
                message("s-1", "m-1", gone),
                message("s-1", "m-2", "merge queue"),
            ],
            &Redactor::default(),
        )
        .unwrap();
    let keys = [("s-1".parse().unwrap(), String::from("m-1"))];
    assert_eq!(store.forget_messages(&keys).unwrap(), 1);
    assert_eq!(store.forget_messages(&keys).unwrap(), 0);
    assert!(
        store
            .search_messages(&tokens("zanzibar"))
            .unwrap()
            .matches
            .is_empty()
    );
    assert_eq!(
        store
            .search_messages(&tokens("merge"))
            .unwrap()
            .matches
            .len(),
        1
    );
    for name in [DB_FILE, "scribe.db-wal"] {
        let bytes = std::fs::read(dir.path().join(name)).unwrap();
        assert!(
            !bytes.windows(8).any(|w| w == b"zanzibar"),
            "{name} still holds the forgotten text"
        );
    }
    // A forget on a store with no database creates none.
    let empty = private_tempdir();
    assert_eq!(Store::open(empty.path()).forget_messages(&keys).unwrap(), 0);
    assert!(!empty.path().join(DB_FILE).exists());
}
