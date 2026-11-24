//! `scribe.db`: privacy, the schema, upserts, redaction on write, FTS5
//! search, the read-only copy, and WAL with `busy_timeout` under concurrent
//! writer processes (a harness of child processes of this test binary,
//! never load on a real host).

use super::super::db::{self, DB_FILE};
use super::private_tempdir;
use crate::GroupVerbs;
use crate::ProposalVerbs;
use crate::entry::Speaker;
use crate::error::Error;
use crate::group::GroupCloser;
use crate::message::NewMessage;
use crate::store::{AuditEvent, AuditStatus, Redactor, Store};
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

fn rows(conn: &rusqlite::Connection, query: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let mut statement = conn.prepare(query).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get::<_, rusqlite::types::Value>(column))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[derive(Debug, PartialEq)]
struct ProjectGroupRows {
    groups: Vec<Vec<rusqlite::types::Value>>,
    members: Vec<Vec<rusqlite::types::Value>>,
    messages: Vec<Vec<rusqlite::types::Value>>,
    sessions: Vec<Vec<rusqlite::types::Value>>,
}

fn project_group_rows(path: &Path) -> ProjectGroupRows {
    let conn = rusqlite::Connection::open(path).unwrap();
    ProjectGroupRows {
        groups: rows(
            &conn,
            "SELECT id,title,kind,project,opened,closed,opener,related,summary,slug FROM groups ORDER BY id",
        ),
        members: rows(
            &conn,
            "SELECT position,group_id,seat,session,author,joined,turn_cursor FROM group_members ORDER BY position",
        ),
        messages: rows(
            &conn,
            "SELECT id,session,message,role,at,text,sha256,group_only FROM messages ORDER BY id",
        ),
        sessions: rows(
            &conn,
            "SELECT provider_session,has_record FROM sessions ORDER BY provider_session",
        ),
    }
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
    assert_eq!(version, 20);
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
fn a_memory_read_refuses_a_database_from_a_newer_build() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    store
        .write_messages(
            &[message("s-1", "m-1", "merge queue")],
            &Redactor::default(),
        )
        .unwrap();
    db::execute(dir.path(), "PRAGMA user_version = 99").unwrap();
    let err = db::memory_view(dir.path()).unwrap_err();
    assert!(
        matches!(err, Error::NewerFormat { found: 99, .. }),
        "{err:?}"
    );
}

#[test]
fn schema_nine_checkpoint_reports_a_neutral_holder_when_a_writer_is_active() {
    let dir = private_tempdir();
    let store = Store::open(dir.path()).with_lock_wait(Duration::from_millis(50));
    store
        .write_messages(
            &[message("s-1", "m-1", "existing committed WAL frame")],
            &Redactor::default(),
        )
        .unwrap();
    db::execute(
        dir.path(),
        &format!(
            "{} ALTER TABLE entries DROP COLUMN decision; ALTER TABLE entries DROP COLUMN item; PRAGMA user_version = 9",
            db::REMOVE_GROUP_CAPTURE_SCHEMA
        ),
    )
    .unwrap();
    assert_eq!(db::meta(dir.path()).unwrap().unwrap().0, 9);

    let writer = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
    writer.busy_timeout(Duration::ZERO).unwrap();
    writer
        .execute_batch(
            "BEGIN IMMEDIATE;
             INSERT INTO messages (session, message, role, at, text, sha256)
             VALUES ('s-held', 'm-held', 'user', '2026-11-23T00:00:00Z', 'uncommitted',
                     '0000000000000000000000000000000000000000000000000000000000000000');",
        )
        .unwrap();

    let err = store
        .write_messages(
            &[message("s-1", "m-2", "retry after the writer releases")],
            &Redactor::default(),
        )
        .unwrap_err();
    let Error::Locked {
        holder, waited_ms, ..
    } = &err
    else {
        panic!("expected a locked store, got {err:?}");
    };
    assert_eq!(holder, "another scribe.db connection");
    assert!(
        *waited_ms >= 40,
        "busy checkpoint did not honor the 50ms lock wait: {waited_ms}ms"
    );
    assert_eq!(err.code(), "store_locked");
    assert!(
        err.to_string()
            .contains("is locked by another scribe.db connection"),
        "{err}"
    );
    assert_eq!(db::meta(dir.path()).unwrap().unwrap().0, 9);

    writer.execute_batch("ROLLBACK").unwrap();
    drop(writer);
    store
        .write_messages(
            &[message("s-1", "m-2", "retry after the writer releases")],
            &Redactor::default(),
        )
        .unwrap();
    assert_eq!(db::meta(dir.path()).unwrap().unwrap().0, 20);
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
        "INSERT INTO pending_archive_forgets (entry, salt, digests, created) VALUES ('ab', '0000000000000000000000000000000000000000000000000000000000000000', '[]', '2026-11-01T10:00:00Z')",
        "INSERT INTO pending_archive_forgets (entry, salt, digests, created) VALUES ('preferences-1', 'short', '[]', '2026-11-01T10:00:00Z')",
        "INSERT INTO pending_archive_forgets (entry, salt, digests, created) VALUES ('preferences-1', '0000000000000000000000000000000000000000000000000000000000000000', '{}', '2026-11-01T10:00:00Z')",
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
    let mut holder = db::open_new_writer(dir.path(), Duration::from_secs(5)).unwrap();
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
    assert_eq!(db::meta(dir.path()).unwrap().unwrap().0, 20);
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

/// Template defect 3: an audit line that did not encode was reported as an
/// I/O error on `audit.jsonl`. Every value scribe encodes for a row is
/// checked through one function, and its failure is `store_corrupt` naming
/// the database, never `io_error`.
#[test]
fn a_value_that_does_not_encode_is_corrupt_not_io() {
    let dir = private_tempdir();
    let db_path = dir.path().join(DB_FILE);
    // A map with non-string keys has no JSON object form.
    let bad: std::collections::BTreeMap<(u8, u8), u8> = [((1, 2), 3)].into_iter().collect();
    let err = db::docs::serialized(&db_path, "an audit line", &bad).unwrap_err();
    assert_eq!(err.code(), "store_corrupt", "{err:?}");
    let crate::Error::Corrupt { path, source } = err else {
        unreachable!()
    };
    assert_eq!(path, db_path);
    assert!(source.to_string().contains("an audit line"), "{source}");
}

#[test]
fn project_group_slug_joins_and_inherits() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let first = "session-one".parse().unwrap();
    let second = "session-two".parse().unwrap();
    let child = "session-child".parse().unwrap();
    let id = store
        .join_project_group("water-economy", &project, &first, T0, &Redactor::default())
        .unwrap();
    assert_eq!(
        store
            .join_project_group("water-economy", &project, &second, T0, &Redactor::default())
            .unwrap(),
        id
    );
    let group = store.group("water-economy").unwrap();
    assert_eq!(group.id, id);
    assert_eq!(group.kind, "project");
    assert_eq!(group.seats.len(), 2);
    assert!(group.seats.iter().all(|s| s.role == "member"));
    assert!(group.closed.is_none());
    store
        .capture_group_turns(
            &first,
            &[message("session-one", "m-1", "water economy plans")],
            &Redactor::default(),
        )
        .unwrap();
    assert_eq!(
        store
            .search_group_messages("water-economy", &tokens("economy"))
            .unwrap()
            .matches
            .len(),
        1
    );
    assert!(
        store
            .join_project_group("other", &project, &first, T0, &Redactor::default())
            .unwrap_err()
            .to_string()
            .contains("water-economy")
    );
    assert!(store.group("other").is_err());
    assert!(matches!(
        store.inherit_seat(&child, Some(&first)).unwrap(),
        crate::group::SeatStart::Inherited { .. }
    ));
    assert!(
        store
            .group("water-economy")
            .unwrap()
            .members()
            .contains(&child)
    );
}

#[test]
fn project_group_backfill_rejects_unverified_turns_atomically() {
    use crate::group::GroupAddition;

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let opener = "group-opener".parse().unwrap();
    let redactor = Redactor::default();
    store
        .add_project_group_sessions(
            "verified-backfill",
            &project,
            &opener,
            &[GroupAddition {
                session: "established-session".parse().unwrap(),
                turns: Some(vec![message(
                    "established-session",
                    "established-turn",
                    "verified existing turn",
                )]),
            }],
            T0,
            &redactor,
        )
        .unwrap();
    let before = project_group_rows(&dir.path().join(DB_FILE));

    let mut assistant_turn = message(
        "assistant-session",
        "assistant-turn",
        "assistant text must not be backfilled",
    );
    assistant_turn.role = Speaker::Assistant;
    let invalid_additions = [
        GroupAddition {
            session: "foreign-named-session".parse().unwrap(),
            turns: Some(vec![message(
                "different-session",
                "foreign-turn",
                "foreign session text",
            )]),
        },
        GroupAddition {
            session: "assistant-session".parse().unwrap(),
            turns: Some(vec![assistant_turn]),
        },
    ];

    for (index, invalid) in invalid_additions.into_iter().enumerate() {
        let accepted_before_failure = GroupAddition {
            session: format!("would-be-added-{index}").parse().unwrap(),
            turns: Some(vec![message(
                &format!("would-be-added-{index}"),
                &format!("valid-turn-{index}"),
                "this earlier batch entry must not be written",
            )]),
        };
        let err = store
            .add_project_group_sessions(
                "verified-backfill",
                &project,
                &opener,
                &[accepted_before_failure, invalid],
                T0,
                &redactor,
            )
            .unwrap_err();
        assert_eq!(err.code(), "group_membership", "{err}");
        assert_eq!(
            project_group_rows(&dir.path().join(DB_FILE)),
            before,
            "rejected batch {index} changed group, membership, message, session, or cursor rows"
        );
    }
}

#[test]
fn project_group_automatic_seat_avoids_every_taken_name() {
    use crate::group::JoinGroup;
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let redactor = Redactor::default();
    let join = |session: &str| {
        let session = session.parse().unwrap();
        store
            .join_project_group("seat-taking", &project, &session, T0, &redactor)
            .unwrap();
        session
    };
    // The short name is held, then an explicit seat takes another session's full id.
    join("abcdefgh-one");
    let two = "abcdefgh-two".parse().unwrap();
    let seat = crate::Label::sanitized("abcdefgh-three", "member");
    let author = "aide".parse().unwrap();
    store
        .join_project_group("seat-taking", &project, &two, T0, &redactor)
        .unwrap();
    store.leave_group(&two).unwrap();
    store
        .join_group(
            JoinGroup {
                group: "seat-taking",
                seat: &seat,
                role: None,
                session: &two,
                author: &author,
                replace: false,
                ended: None,
                predecessor: None,
                earlier_turns: &[],
                snapshot_at: time::OffsetDateTime::now_utc(),
            },
            &redactor,
        )
        .unwrap();
    // Both of this session's candidate names are taken.
    join("abcdefgh-three");
    // A session whose short and full ids are the same name.
    join("abcdefgh");
    let mut names: Vec<_> = store
        .group("seat-taking")
        .unwrap()
        .seats
        .into_iter()
        .map(|s| (s.name, s.transcripts.len()))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            ("abcdefgh".to_owned(), 1),
            ("abcdefgh-2".to_owned(), 1),
            ("abcdefgh-three".to_owned(), 1),
            ("abcdefgh-three-2".to_owned(), 1),
        ]
    );
}

#[test]
fn claude_capture_rejects_a_stale_snapshot_before_advancing_progress() {
    use crate::ClaudeCapturePosition;
    use crate::instruction::ClaudeGroupTurns;

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let session = "session-one".parse().unwrap();
    let redactor = Redactor::default();
    store
        .join_project_group(
            "replay",
            &crate::Project::sanitized("checkout"),
            &session,
            T0,
            &redactor,
        )
        .unwrap();
    let batch = |id: &str, text: &str| {
        let row = serde_json::json!({
            "sessionId": session, "uuid": id, "type": "user", "entrypoint": "cli",
            "timestamp": "2090-11-20T01:00:00Z", "message": {"content": text}
        });
        ClaudeGroupTurns::parse(row.to_string().as_bytes(), &session)
    };
    let position = |offset| ClaudeCapturePosition {
        file_id: "1:2".to_owned(),
        offset,
        modified: offset.to_string(),
        boundary: "sample".to_owned(),
    };
    let initial = store.claude_group_capture(&session).unwrap().unwrap();
    assert_eq!(
        store
            .capture_claude_group_turns(
                &initial,
                false,
                &position(100),
                batch("first", "orchid first"),
                &redactor
            )
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        store
            .capture_claude_group_turns(
                &initial,
                false,
                &position(90),
                batch("stale", "orchid stale"),
                &redactor
            )
            .unwrap(),
        None
    );
    let next = store.claude_group_capture(&session).unwrap().unwrap();
    assert_eq!(next.progress().unwrap().offset, 100);
    assert_eq!(
        store
            .capture_claude_group_turns(
                &next,
                true,
                &position(150),
                batch("next", "orchid next"),
                &redactor
            )
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        store
            .search_group_messages("replay", &tokens("orchid"))
            .unwrap()
            .matches
            .len(),
        2
    );

    store.leave_group(&session).unwrap();
    let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
    for table in ["group_claude_progress", "group_claude_seen"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

/// A turn appended after a backfill snapshot is read but dated before the
/// membership commit follows the snapshot's cursor, so capture stores it once
/// on either supported path; a turn before an ordinary join stays cut off.
#[test]
fn backfill_keeps_a_turn_appended_after_the_snapshot_but_dated_before_the_join() {
    use crate::ClaudeCapturePosition;
    use crate::instruction::{ClaudeGroupTurns, claude_group_turns};

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let opener = "group-opener".parse().unwrap();
    let session: crate::SessionId = "backfilled-session".parse().unwrap();
    let redactor = Redactor::default();
    // `T0` is 2026-11-01 10:00 UTC; the late turn is an hour earlier.
    let row = |id: &str, at: &str, text: &str| {
        serde_json::json!({
            "sessionId": session, "uuid": id, "type": "user", "entrypoint": "cli",
            "timestamp": at, "message": {"content": text}
        })
        .to_string()
    };
    let snapshot = row("first", "2026-10-31T09:00:00Z", "orchid snapshot turn");
    let late = row("late", "2026-11-01T09:00:00Z", "orchid late turn");
    store
        .add_project_group_sessions(
            "late-turn",
            &project,
            &opener,
            &[crate::group::GroupAddition {
                session: session.clone(),
                turns: Some(claude_group_turns(snapshot.as_bytes(), &session)),
            }],
            T0,
            &redactor,
        )
        .unwrap();
    let replay = format!("{snapshot}\n{late}\n");

    // Claude path: a full replay after the saved cursor, no byte progress yet.
    let position = ClaudeCapturePosition {
        file_id: "1:2".to_owned(),
        offset: 10,
        modified: "10".to_owned(),
        boundary: "sample".to_owned(),
    };
    for expected in [1, 0] {
        let snap = store.claude_group_capture(&session).unwrap().unwrap();
        let stored = store
            .capture_claude_group_turns(
                &snap,
                false,
                &position,
                ClaudeGroupTurns::parse(replay.as_bytes(), &session),
                &redactor,
            )
            .unwrap();
        assert_eq!(stored, Some(expected));
    }
    let count = |term: &str| {
        store
            .search_group_messages("late-turn", &tokens(term))
            .unwrap()
            .matches
            .len()
    };
    assert_eq!(count("late"), 1);
    assert_eq!(count("snapshot"), 1);

    // Generic (Codex) path.
    let other: crate::SessionId = "codex-backfilled".parse().unwrap();
    let mut first = message("codex-backfilled", "c-1", "orchid codex first");
    first.at = T0 - Duration::from_secs(86_400);
    let mut later = message("codex-backfilled", "c-2", "orchid codex later");
    later.at = T0 - Duration::from_secs(3_600);
    store
        .add_project_group_sessions(
            "late-turn",
            &project,
            &opener,
            &[crate::group::GroupAddition {
                session: other.clone(),
                turns: Some(vec![first.clone()]),
            }],
            T0,
            &redactor,
        )
        .unwrap();
    for _ in 0..2 {
        store
            .capture_group_turns(&other, &[first.clone(), later.clone()], &redactor)
            .unwrap();
    }
    assert_eq!(count("codex"), 2, "first once, later once");
    assert_eq!(count("later"), 1);

    // An ordinary join keeps its cutoff: a turn dated before it is not stored.
    let joined: crate::SessionId = "ordinary-join".parse().unwrap();
    store
        .join_project_group("late-turn", &project, &joined, T0, &redactor)
        .unwrap();
    let mut old = message("ordinary-join", "o-1", "orchid ordinary old");
    old.at = T0 - Duration::from_secs(3_600);
    let mut new = message("ordinary-join", "o-2", "orchid ordinary new");
    new.at = T0;
    store
        .capture_group_turns(&joined, &[old, new], &redactor)
        .unwrap();
    assert_eq!(count("ordinary"), 1);
    assert_eq!(count("new"), 1);
}

/// A displaced holder that typed while another session held its seat keeps
/// those turns out of group search when it regains the seat. Its backfill
/// cursor, set before the displacement, is cleared on restoration, which
/// restarts its consent window; the new join time then cuts off the turns typed
/// during the displacement, even though no hook ran to move the cursor past them.
#[test]
fn a_restored_seat_with_a_cursor_excludes_turns_typed_during_displacement() {
    use crate::ClaudeCapturePosition;
    use crate::group::JoinGroup;
    use crate::instruction::{ClaudeGroupTurns, claude_group_turns};
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let opener = "group-opener".parse().unwrap();
    let author = "aide".parse().unwrap();
    let redactor = Redactor::default();
    let now = OffsetDateTime::now_utc();
    let hour = Duration::from_secs(3_600);
    let claude: crate::SessionId = "restored-claude".parse().unwrap();
    let codex: crate::SessionId = "restored-codex".parse().unwrap();
    let row = |id: &str, at: OffsetDateTime, text: &str| {
        serde_json::json!({
            "sessionId": claude, "uuid": id, "type": "user", "entrypoint": "cli",
            "timestamp": at.format(&Rfc3339).unwrap(), "message": {"content": text}
        })
        .to_string()
    };
    let first = row("first", T0, "orchid claude first");
    let during = row("during", now - hour / 2, "orchid claude during");
    let after = row("after", now + hour, "orchid claude restored");
    let codex_first = {
        let mut m = message("restored-codex", "c-1", "orchid codex first");
        m.at = T0;
        m
    };
    let codex_during = {
        let mut m = message("restored-codex", "c-2", "orchid codex during");
        m.at = now - hour / 2;
        m
    };
    let codex_after = {
        let mut m = message("restored-codex", "c-3", "orchid codex restored");
        m.at = now + hour;
        m
    };
    // Backfilled members keep a cursor; each holds its own seat.
    store
        .add_project_group_sessions(
            "restored",
            &project,
            &opener,
            &[
                crate::group::GroupAddition {
                    session: claude.clone(),
                    turns: Some(claude_group_turns(first.as_bytes(), &claude)),
                },
                crate::group::GroupAddition {
                    session: codex.clone(),
                    turns: Some(vec![codex_first.clone()]),
                },
            ],
            T0,
            &redactor,
        )
        .unwrap();
    let seat_of = |session: &crate::SessionId| -> crate::Label {
        let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
        let seat: String = conn
            .query_row(
                "SELECT seat FROM group_members WHERE session = ?1",
                [session.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        crate::Label::sanitized(&seat, "member")
    };
    // Replacements take both seats; the displaced holders type meanwhile and
    // no hook runs for them.
    let mut replacements = Vec::new();
    for (index, displaced) in [&claude, &codex].into_iter().enumerate() {
        let seat = seat_of(displaced);
        let replacement: crate::SessionId = format!("replacement-{index}").parse().unwrap();
        store
            .join_group(
                JoinGroup {
                    group: "restored",
                    seat: &seat,
                    role: None,
                    session: &replacement,
                    author: &author,
                    replace: true,
                    ended: None,
                    predecessor: None,
                    earlier_turns: &[],
                    snapshot_at: time::OffsetDateTime::now_utc(),
                },
                &redactor,
            )
            .unwrap();
        replacements.push(replacement);
    }
    for replacement in &replacements {
        store.leave_group(replacement).unwrap();
    }

    let position = ClaudeCapturePosition {
        file_id: "1:2".to_owned(),
        offset: 10,
        modified: "10".to_owned(),
        boundary: "sample".to_owned(),
    };
    let capture_claude = |replay: String| {
        let snap = store.claude_group_capture(&claude).unwrap().unwrap();
        store
            .capture_claude_group_turns(
                &snap,
                false,
                &position,
                ClaudeGroupTurns::parse(replay.as_bytes(), &claude),
                &redactor,
            )
            .unwrap()
    };
    let count = |term: &str| {
        store
            .search_group_messages("restored", &tokens(term))
            .unwrap()
            .matches
            .len()
    };
    // The first capture after restoration sees the displacement interval.
    assert_eq!(capture_claude(format!("{first}\n{during}\n")), Some(0));
    store
        .capture_group_turns(
            &codex,
            &[codex_first.clone(), codex_during.clone()],
            &redactor,
        )
        .unwrap();
    assert_eq!(count("during"), 0);
    // Later authorized turns are stored once.
    for _ in 0..2 {
        capture_claude(format!("{first}\n{during}\n{after}\n"));
        store
            .capture_group_turns(
                &codex,
                &[
                    codex_first.clone(),
                    codex_during.clone(),
                    codex_after.clone(),
                ],
                &redactor,
            )
            .unwrap();
    }
    assert_eq!(count("during"), 0);
    assert_eq!(count("restored"), 2, "one per path, once each");
    assert_eq!(count("first"), 2, "the backfilled turns stay");
}

#[test]
fn forgetting_the_seat_holder_restarts_the_restored_holders_capture_window() {
    use crate::ClaudeCapturePosition;
    use crate::consistency::Settle;
    use crate::entry::Cite;
    use crate::group::JoinGroup;
    use crate::instruction::{ClaudeGroupTurns, claude_group_turns};
    use time::format_description::well_known::Rfc3339;

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let opener: crate::SessionId = "group-opener".parse().unwrap();
    let author = "aide".parse().unwrap();
    let redactor = Redactor::default();
    let hour = Duration::from_secs(3_600);
    let displaced_at = T0 + hour;
    let forget_at = T0 + 2 * hour;
    let restored_at = T0 + 3 * hour;

    let claude_backfill: crate::SessionId = "forget-claude-backfill".parse().unwrap();
    let claude_resumed: crate::SessionId = "forget-claude-resumed".parse().unwrap();
    let codex: crate::SessionId = "forget-codex".parse().unwrap();

    let claude_row =
        |session: &crate::SessionId, id: &str, at: time::OffsetDateTime, text: &str| {
            serde_json::json!({
                "sessionId": session, "uuid": id, "type": "user", "entrypoint": "cli",
                "timestamp": at.format(&Rfc3339).unwrap(), "message": {"content": text}
            })
            .to_string()
        };

    let b_first = claude_row(&claude_backfill, "b-1", T0, "orchid backfill first");
    let b_during = claude_row(
        &claude_backfill,
        "b-2",
        displaced_at,
        "orchid backfill during",
    );
    let b_after = claude_row(
        &claude_backfill,
        "b-3",
        restored_at,
        "orchid backfill restored",
    );

    let r_first = claude_row(&claude_resumed, "r-1", T0, "orchid resumed first");
    let r_during = claude_row(
        &claude_resumed,
        "r-2",
        displaced_at,
        "orchid resumed during",
    );
    let r_after = claude_row(
        &claude_resumed,
        "r-3",
        restored_at,
        "orchid resumed restored",
    );

    let mut codex_first = message("forget-codex", "c-1", "orchid codex first");
    codex_first.at = T0;
    let mut codex_during = message("forget-codex", "c-2", "orchid codex during");
    codex_during.at = displaced_at;
    let mut codex_after = message("forget-codex", "c-3", "orchid codex restored");
    codex_after.at = restored_at;

    store
        .add_project_group_sessions(
            "forget-restored",
            &project,
            &opener,
            &[
                crate::group::GroupAddition {
                    session: claude_backfill.clone(),
                    turns: Some(claude_group_turns(b_first.as_bytes(), &claude_backfill)),
                },
                crate::group::GroupAddition {
                    session: codex.clone(),
                    turns: Some(vec![codex_first.clone()]),
                },
            ],
            T0,
            &redactor,
        )
        .unwrap();
    store
        .join_project_group("forget-restored", &project, &claude_resumed, T0, &redactor)
        .unwrap();

    let position = |offset| ClaudeCapturePosition {
        file_id: "1:2".to_owned(),
        offset,
        modified: offset.to_string(),
        boundary: "sample".to_owned(),
    };
    let initial = store
        .claude_group_capture(&claude_resumed)
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .capture_claude_group_turns(
                &initial,
                false,
                &position(100),
                ClaudeGroupTurns::parse(r_first.as_bytes(), &claude_resumed),
                &redactor,
            )
            .unwrap(),
        Some(1)
    );

    let seat_of = |session: &crate::SessionId| -> crate::Label {
        let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
        let seat: String = conn
            .query_row(
                "SELECT seat FROM group_members WHERE session = ?1",
                [session.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        crate::Label::sanitized(&seat, "member")
    };

    let mut replacements = Vec::new();
    for (index, displaced) in [&claude_backfill, &claude_resumed, &codex]
        .into_iter()
        .enumerate()
    {
        let seat = seat_of(displaced);
        let replacement: crate::SessionId = format!("forget-replacement-{index}").parse().unwrap();
        store
            .join_group(
                JoinGroup {
                    group: "forget-restored",
                    seat: &seat,
                    role: None,
                    session: &replacement,
                    author: &author,
                    replace: true,
                    ended: None,
                    predecessor: None,
                    earlier_turns: &[],
                    snapshot_at: time::OffsetDateTime::now_utc(),
                },
                &redactor,
            )
            .unwrap();
        replacements.push(replacement);
    }

    // Forget each seat holder while the displaced members had no hook run.
    for replacement in &replacements {
        let cite = Cite {
            session: replacement.clone(),
            message: None,
            at: forget_at,
            prompt_sha256: "0".repeat(64),
            line: 1,
            cwd: String::from("/work"),
        };
        store
            .settle(
                &Settle::ForgetSession(replacement.clone()),
                &cite,
                forget_at,
            )
            .unwrap();
    }

    let count = |term: &str| {
        store
            .search_group_messages("forget-restored", &tokens(term))
            .unwrap()
            .matches
            .len()
    };

    // First captures see only the displaced turns: none may be stored.
    let backfill_snap = store
        .claude_group_capture(&claude_backfill)
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .capture_claude_group_turns(
                &backfill_snap,
                false,
                &position(10),
                ClaudeGroupTurns::parse(
                    format!("{b_first}\n{b_during}\n").as_bytes(),
                    &claude_backfill
                ),
                &redactor,
            )
            .unwrap(),
        Some(0)
    );
    let resumed_snap = store
        .claude_group_capture(&claude_resumed)
        .unwrap()
        .unwrap();
    assert_eq!(resumed_snap.progress().unwrap().offset, 100);
    assert_eq!(
        store
            .capture_claude_group_turns(
                &resumed_snap,
                true,
                &position(150),
                ClaudeGroupTurns::parse(r_during.as_bytes(), &claude_resumed),
                &redactor,
            )
            .unwrap(),
        Some(0)
    );
    store
        .capture_group_turns(
            &codex,
            &[codex_first.clone(), codex_during.clone()],
            &redactor,
        )
        .unwrap();
    assert_eq!(count("during"), 0);

    // Subsequent authorized turns are stored once each, and prior rows stay.
    for round in 0..2 {
        let expected = usize::from(round == 0);
        let b_snap = store
            .claude_group_capture(&claude_backfill)
            .unwrap()
            .unwrap();
        assert_eq!(
            store
                .capture_claude_group_turns(
                    &b_snap,
                    false,
                    &position(20),
                    ClaudeGroupTurns::parse(
                        format!("{b_first}\n{b_during}\n{b_after}\n").as_bytes(),
                        &claude_backfill,
                    ),
                    &redactor,
                )
                .unwrap(),
            Some(expected)
        );

        let r_snap = store
            .claude_group_capture(&claude_resumed)
            .unwrap()
            .unwrap();
        let (resumed_flag, offset, r_payload) = if round == 0 {
            (true, 200, r_after.clone())
        } else {
            (false, 200, format!("{r_first}\n{r_during}\n{r_after}\n"))
        };
        assert_eq!(
            store
                .capture_claude_group_turns(
                    &r_snap,
                    resumed_flag,
                    &position(offset),
                    ClaudeGroupTurns::parse(r_payload.as_bytes(), &claude_resumed),
                    &redactor,
                )
                .unwrap(),
            Some(expected)
        );

        let codex_stored = store
            .capture_group_turns(
                &codex,
                &[
                    codex_first.clone(),
                    codex_during.clone(),
                    codex_after.clone(),
                ],
                &redactor,
            )
            .unwrap();
        assert_eq!(codex_stored, expected);
    }

    assert_eq!(count("during"), 0);
    assert_eq!(count("first"), 3, "previously authorized turns stay");
    assert_eq!(
        count("restored"),
        3,
        "each path captures its authorized turn once"
    );
}

#[test]
fn forgetting_a_non_holder_preserves_the_active_holders_capture_state() {
    use crate::ClaudeCapturePosition;
    use crate::consistency::Settle;
    use crate::entry::Cite;
    use crate::group::JoinGroup;
    use crate::instruction::ClaudeGroupTurns;
    use time::format_description::well_known::Rfc3339;

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let author = "aide".parse().unwrap();
    let redactor = Redactor::default();
    let hour = Duration::from_secs(3_600);
    let now = time::OffsetDateTime::now_utc();
    let active_joined_at = now + hour / 2;
    let late_turn_at = now + hour;
    let forget_at = now + 2 * hour;

    let prior_claude: crate::SessionId = "prior-claude".parse().unwrap();
    let active_claude: crate::SessionId = "active-claude".parse().unwrap();
    let prior_codex: crate::SessionId = "prior-codex".parse().unwrap();
    let active_codex: crate::SessionId = "active-codex".parse().unwrap();

    store
        .join_project_group("keep-holder", &project, &prior_claude, T0, &redactor)
        .unwrap();
    store
        .join_project_group("keep-holder", &project, &prior_codex, T0, &redactor)
        .unwrap();

    let seat_of = |session: &crate::SessionId| -> crate::Label {
        let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
        let seat: String = conn
            .query_row(
                "SELECT seat FROM group_members WHERE session = ?1",
                [session.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        crate::Label::sanitized(&seat, "member")
    };

    for (prior, active) in [
        (&prior_claude, &active_claude),
        (&prior_codex, &active_codex),
    ] {
        let seat = seat_of(prior);
        store
            .join_group(
                JoinGroup {
                    group: "keep-holder",
                    seat: &seat,
                    role: None,
                    session: active,
                    author: &author,
                    replace: true,
                    ended: None,
                    predecessor: None,
                    earlier_turns: &[],
                    snapshot_at: time::OffsetDateTime::now_utc(),
                },
                &redactor,
            )
            .unwrap();
    }

    let claude_row = |id: &str, at: time::OffsetDateTime, text: &str| {
        serde_json::json!({
            "sessionId": active_claude, "uuid": id, "type": "user", "entrypoint": "cli",
            "timestamp": at.format(&Rfc3339).unwrap(), "message": {"content": text}
        })
        .to_string()
    };
    let pos = |offset| ClaudeCapturePosition {
        file_id: "1:2".to_owned(),
        offset,
        modified: offset.to_string(),
        boundary: "sample".to_owned(),
    };
    let a_first = claude_row("a-1", active_joined_at, "orchid active claude first");
    let a_late = claude_row("a-2", late_turn_at, "orchid active claude late");
    let c_first = {
        let mut m = message("active-codex", "ac-1", "orchid active codex first");
        m.at = active_joined_at;
        m
    };
    let c_late = {
        let mut m = message("active-codex", "ac-2", "orchid active codex late");
        m.at = late_turn_at;
        m
    };

    // Establish cursors and progress on both active holders before forgetting
    // the displaced non-holders. Use `late_turn_at` < `forget_at` so a reset
    // would wrongly drop `a_late` / `c_late`.
    let initial = store.claude_group_capture(&active_claude).unwrap().unwrap();
    assert_eq!(
        store
            .capture_claude_group_turns(
                &initial,
                false,
                &pos(100),
                ClaudeGroupTurns::parse(a_first.as_bytes(), &active_claude),
                &redactor,
            )
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        store
            .capture_group_turns(&active_codex, std::slice::from_ref(&c_first), &redactor)
            .unwrap(),
        1
    );

    let capture_state_rows = || {
        let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
        rows(
            &conn,
            "SELECT session, joined, turn_cursor FROM group_members ORDER BY position",
        )
    };
    let before = capture_state_rows();

    for non_holder in [&prior_claude, &prior_codex] {
        let cite = Cite {
            session: non_holder.clone(),
            message: None,
            at: forget_at,
            prompt_sha256: "0".repeat(64),
            line: 1,
            cwd: String::from("/work"),
        };
        store
            .settle(&Settle::ForgetSession(non_holder.clone()), &cite, forget_at)
            .unwrap();
    }

    let after = capture_state_rows();
    assert_eq!(after.len(), 2);
    // Both active holders kept their exact `joined` and `turn_cursor`.
    assert_eq!(after, before[2..]);

    let snap = store.claude_group_capture(&active_claude).unwrap().unwrap();
    assert_eq!(snap.progress().unwrap().offset, 100);
    assert_eq!(
        store
            .capture_claude_group_turns(
                &snap,
                true,
                &pos(150),
                ClaudeGroupTurns::parse(a_late.as_bytes(), &active_claude),
                &redactor,
            )
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        store
            .capture_group_turns(&active_codex, &[c_first, c_late], &redactor)
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .search_group_messages("keep-holder", &tokens("late"))
            .unwrap()
            .matches
            .len(),
        2
    );
}

#[test]
fn project_group_refuses_sensitive_slug_and_masks_legacy_membership_in_errors() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let first = "session-one".parse().unwrap();
    let second = "session-two".parse().unwrap();
    let secret = "orchid-secret-token";
    let redactor = Redactor::new([("TOKEN".into(), secret.into())]);
    for slug in [secret, "prefix-orchid-secret-token-suffix"] {
        let err = store
            .join_project_group(slug, &project, &first, T0, &redactor)
            .unwrap_err();
        assert_eq!(err.code(), "invalid_text");
        assert!(!err.to_string().contains(secret));
        assert!(store.group(slug).is_err());
    }
    // A name created before the secret was configured must not leak in a refusal either.
    store
        .join_project_group(secret, &project, &first, T0, &Redactor::default())
        .unwrap();
    let err = store
        .join_project_group("ordinary", &project, &first, T0, &redactor)
        .unwrap_err();
    assert!(!err.to_string().contains(secret));
    assert!(
        store
            .join_project_group(secret, &project, &second, T0, &redactor)
            .is_err()
    );
}

#[test]
fn close_group_commits_audit_with_closing_record_without_holding_write_lock_across_model() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let opener: crate::SessionId = "session-opener".parse().unwrap();
    let second_opener: crate::SessionId = "session-second".parse().unwrap();
    let id = store
        .join_project_group("close-audit", &project, &opener, T0, &Redactor::default())
        .unwrap();
    store
        .capture_group_turns(
            &opener,
            &[message("session-opener", "m-1", "secret phrase in turn")],
            &Redactor::default(),
        )
        .unwrap();

    let audit_for = |group_id: &str, session: &crate::SessionId| AuditEvent {
        at: T0,
        command: "group close".to_owned(),
        status: AuditStatus::Success,
        target: Some(group_id.to_owned()),
        error_code: None,
        author: None,
        instruction_session: Some(session.as_str().to_owned()),
        instruction_message: None,
        issue: None,
        duration_ms: 1,
    };

    let mut model_calls = 0;
    let (closed_id, changed) = store
        .close_group_with_outcome(
            &id,
            GroupCloser::Session(&opener),
            T0,
            &Redactor::default(),
            |_| {
                model_calls += 1;
                // No SQLite write transaction may be held during the model call.
                let probe = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
                probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK;").unwrap();
                Ok::<_, Error>("Summary of secret phrase work.".to_owned())
            },
            |group_id| audit_for(group_id, &opener),
        )
        .unwrap()
        .into_result()
        .unwrap();
    assert_eq!(closed_id, id);
    assert!(changed);
    assert_eq!(model_calls, 1);

    let group = store.group(&id).unwrap();
    assert!(group.closed.is_some());
    assert_eq!(
        group.summary.as_deref(),
        Some("Summary of secret phrase work.")
    );
    let (events, torn) = store.audit_events().unwrap();
    assert_eq!(torn, 0);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].command, "group close");
    assert_eq!(events[0].status, AuditStatus::Success);
    assert_eq!(events[0].target.as_deref(), Some(id.as_str()));
    assert_eq!(events[0].error_code, None);
    assert_eq!(events[0].author, None);
    assert_eq!(
        events[0].instruction_session.as_deref(),
        Some(opener.as_str())
    );
    assert_eq!(events[0].instruction_message, None);
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains("Summary of secret phrase work."));
    assert!(!serialized.contains("secret phrase in turn"));

    // Retry returns changed=false without calling the model or adding an audit row.
    let (retry_id, retry_changed) = store
        .close_group_with_outcome(
            &id,
            GroupCloser::Session(&opener),
            T0,
            &Redactor::default(),
            |_| -> Result<String, Error> { panic!("model must not run on retry") },
            |_| panic!("audit builder must not run on retry"),
        )
        .unwrap()
        .into_result()
        .unwrap();
    assert_eq!(retry_id, id);
    assert!(!retry_changed);
    assert_eq!(store.audit_events().unwrap().0.len(), 1);

    // If writing the audit row fails inside the final write, closed and summary roll back too.
    let second_id = store
        .join_project_group(
            "close-rollback",
            &project,
            &second_opener,
            T0,
            &Redactor::default(),
        )
        .unwrap();
    let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER abort_group_close_audit BEFORE INSERT ON audit
         WHEN NEW.command = 'group close'
         BEGIN
             SELECT RAISE(ABORT, 'injected audit failure');
         END;",
    )
    .unwrap();
    drop(conn);

    let err = store
        .close_group_with_outcome(
            &second_id,
            GroupCloser::Session(&second_opener),
            T0,
            &Redactor::default(),
            |_| Ok::<_, Error>("Rolled back summary.".to_owned()),
            |group_id| audit_for(group_id, &second_opener),
        )
        .unwrap_err();
    let source = std::error::Error::source(&err).unwrap().to_string();
    assert!(source.contains("injected audit failure"), "{source}");

    let still_open = store.group(&second_id).unwrap();
    assert!(still_open.closed.is_none());
    assert!(still_open.summary.is_none());
    assert_eq!(store.audit_events().unwrap().0.len(), 1);
}

#[test]
fn project_group_join_returns_error_promptly_when_seat_lookup_fails() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let first: crate::SessionId = "session-one".parse().unwrap();
    let second: crate::SessionId = "session-two".parse().unwrap();
    store
        .join_project_group("existing", &project, &first, T0, &Redactor::default())
        .unwrap();

    // Replace `group_seats` with a view whose evaluation fails inside `query_row`
    // (after `prepare` succeeds), simulating a persistent SQLite seat-query failure.
    let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE group_seats;
         CREATE VIEW group_seats AS
         SELECT id AS group_id, json('not-json') AS name, 'member' AS role FROM groups;",
    )
    .unwrap();
    drop(conn);

    for slug in ["existing", "new-group"] {
        let err = store
            .join_project_group(slug, &project, &second, T0, &Redactor::default())
            .unwrap_err();
        let source = std::error::Error::source(&err).unwrap().to_string();
        assert!(source.contains("malformed JSON"), "{slug}: {source}");
    }

    let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
    let member_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM group_members", [], |r| r.get(0))
        .unwrap();
    assert_eq!(member_count, 1);
    let second_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM group_members WHERE session = ?1",
            [second.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(second_rows, 0);
    let new_groups: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM groups WHERE slug = 'new-group'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_groups, 0);
}

/// A join whose snapshot held no eligible turn has no cursor, so its join time
/// is the cutoff. That time is the snapshot's: a first typed turn appended
/// after the snapshot but before the membership committed is stored once,
/// while a turn dated before the snapshot stays out.
#[test]
fn an_empty_join_snapshot_keeps_the_first_turn_appended_before_the_join_commits() {
    use crate::group::JoinGroup;

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let redactor = Redactor::default();
    let opener = "empty-snapshot-opener".parse().unwrap();
    store
        .join_project_group("empty-snapshot", &project, &opener, T0, &redactor)
        .unwrap();
    let session: crate::SessionId = "empty-snapshot-joiner".parse().unwrap();
    let seat = crate::Label::sanitized("editor", "member");
    let author = "aide".parse().unwrap();
    // The snapshot at `T0` found no eligible turn; the join commits at the
    // wall clock, long after the turn below was appended.
    assert!(
        store
            .join_group(
                JoinGroup {
                    group: "empty-snapshot",
                    seat: &seat,
                    role: None,
                    session: &session,
                    author: &author,
                    replace: false,
                    ended: None,
                    predecessor: None,
                    earlier_turns: &[],
                    snapshot_at: T0,
                },
                &redactor,
            )
            .unwrap()
    );
    let mut before = message("empty-snapshot-joiner", "j-0", "orchid before the snapshot");
    before.at = T0 - Duration::from_secs(1);
    let mut first = message("empty-snapshot-joiner", "j-1", "orchid first appended turn");
    first.at = T0 + Duration::from_secs(1);
    let mut second = message("empty-snapshot-joiner", "j-2", "orchid second turn");
    second.at = T0 + Duration::from_secs(2);
    let turns = [before, first, second];
    assert_eq!(
        store
            .capture_group_turns(&session, &turns, &redactor)
            .unwrap(),
        2
    );
    // A repeated capture stores nothing twice.
    assert_eq!(
        store
            .capture_group_turns(&session, &turns, &redactor)
            .unwrap(),
        0
    );
    let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
    let stored: Vec<String> = conn
        .prepare("SELECT message FROM messages WHERE session = ?1 ORDER BY message")
        .unwrap()
        .query_map([session.as_str()], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(stored, ["j-1", "j-2"]);
}

/// A backfill carries only the joining session's own user turns; anything else
/// is refused before the membership is written.
#[test]
fn a_join_backfill_takes_only_the_joining_sessions_own_user_turns() {
    use crate::group::JoinGroup;

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let redactor = Redactor::default();
    let opener = "backfill-opener".parse().unwrap();
    store
        .join_project_group("backfill", &project, &opener, T0, &redactor)
        .unwrap();
    let session: crate::SessionId = "backfill-joiner".parse().unwrap();
    let seat = crate::Label::sanitized("editor", "member");
    let author = "aide".parse().unwrap();
    let mut assistant = message("backfill-joiner", "a-1", "assistant words");
    assistant.role = Speaker::Assistant;
    let foreign = message("backfill-other", "f-1", "another session's words");
    for turns in [vec![assistant], vec![foreign]] {
        let err = store
            .join_group(
                JoinGroup {
                    group: "backfill",
                    seat: &seat,
                    role: None,
                    session: &session,
                    author: &author,
                    replace: false,
                    ended: None,
                    predecessor: None,
                    earlier_turns: &turns,
                    snapshot_at: time::OffsetDateTime::now_utc(),
                },
                &redactor,
            )
            .unwrap_err();
        assert!(err.to_string().contains("own user turns"), "{err}");
    }
    let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM group_members WHERE session = ?1)
                  + (SELECT COUNT(*) FROM messages)",
            [session.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}

fn quoter_transcript(session: &str, typed: &str) -> Vec<u8> {
    serde_json::json!({"type": "user", "uuid": "u-1", "sessionId": session,
        "timestamp": "2026-11-23T10:00:00Z", "message": {"role": "user", "content": typed}})
    .to_string()
    .into_bytes()
}

#[test]
fn quoted_close_consent_authorizes_only_its_own_group_and_operation() {
    use crate::Provider;
    use crate::group::GroupConsent;

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let opener: crate::SessionId = "session-opener".parse().unwrap();
    let quoter: crate::SessionId = "session-quoter".parse().unwrap();
    let redactor = Redactor::default();
    let audit = |group_id: &str| AuditEvent {
        at: T0,
        command: "group close".to_owned(),
        status: AuditStatus::Success,
        target: Some(group_id.to_owned()),
        error_code: None,
        author: None,
        instruction_session: Some(quoter.as_str().to_owned()),
        instruction_message: None,
        issue: None,
        duration_ms: 1,
    };
    let try_close = |id: &str, consent: &GroupConsent| {
        store.close_group_with_outcome(
            id,
            GroupCloser::Quoted(consent),
            T0,
            &redactor,
            |_| -> Result<String, Error> { panic!("a refused close must not call the model") },
            audit,
        )
    };

    let first = store
        .join_project_group("alpha", &project, &opener, T0, &redactor)
        .unwrap();
    let other: crate::SessionId = "session-other".parse().unwrap();
    let second = store
        .join_project_group("beta", &project, &other, T0, &redactor)
        .unwrap();

    // Consent for closing one group does not close another, by id or alias.
    let bytes = quoter_transcript(quoter.as_str(), &format!("please close {second} now"));
    let for_second = GroupConsent::verify_close(
        quoter.clone(),
        Provider::ClaudeCode,
        &bytes,
        &format!("close {second}"),
        &second,
    )
    .unwrap();
    for target in [first.as_str(), "alpha"] {
        let err = try_close(target, &for_second).unwrap_err();
        assert_eq!(err.code(), "group_membership", "{target}");
        assert!(store.group(&first).unwrap().closed.is_none());
    }

    // Consent to open a group is another operation and closes nothing.
    let for_open = GroupConsent::verify(
        quoter.clone(),
        Provider::ClaudeCode,
        &bytes,
        &format!("close {second}"),
    )
    .unwrap();
    assert!(try_close(&second, &for_open).is_err());
    assert!(store.group(&second).unwrap().closed.is_none());

    // And consent to close cannot open one.
    let err = store
        .open_group(
            crate::group::NewGroup {
                title: "title".to_owned(),
                kind: "work".to_owned(),
                project: project.clone(),
                related: Vec::new(),
            },
            &for_second,
            &[],
            T0,
            &redactor,
        )
        .err()
        .expect("close consent must not open a group");
    assert_eq!(err.code(), "group_membership");
}

#[test]
fn quoted_close_consent_for_a_closed_group_does_not_reach_its_reused_alias() {
    use crate::Provider;
    use crate::group::GroupConsent;

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let project = crate::Project::sanitized("checkout");
    let opener: crate::SessionId = "session-opener".parse().unwrap();
    let replacer: crate::SessionId = "session-replacer".parse().unwrap();
    let quoter: crate::SessionId = "session-quoter".parse().unwrap();
    let redactor = Redactor::default();

    let old = store
        .join_project_group("reused", &project, &opener, T0, &redactor)
        .unwrap();
    let bytes = quoter_transcript(quoter.as_str(), &format!("close {old}"));
    let consent = GroupConsent::verify_close(
        quoter.clone(),
        Provider::ClaudeCode,
        &bytes,
        &format!("close {old}"),
        &old,
    )
    .unwrap();

    // The old group closes and its slug is reused by a replacement.
    store
        .close_group_with_outcome(
            &old,
            GroupCloser::Session(&opener),
            T0,
            &redactor,
            |_| Ok::<_, Error>("Done.".to_owned()),
            |id| AuditEvent {
                at: T0,
                command: "group close".to_owned(),
                status: AuditStatus::Success,
                target: Some(id.to_owned()),
                error_code: None,
                author: None,
                instruction_session: None,
                instruction_message: None,
                issue: None,
                duration_ms: 1,
            },
        )
        .unwrap();
    let replacement = store
        .join_project_group("reused", &project, &replacer, T0, &redactor)
        .unwrap();
    assert_ne!(replacement, old);
    assert_eq!(store.group("reused").unwrap().id, replacement);

    // The old request, replayed through the alias, cannot close the replacement.
    let err = store
        .close_group_with_outcome(
            "reused",
            GroupCloser::Quoted(&consent),
            T0,
            &redactor,
            |_| -> Result<String, Error> { panic!("the replacement must not be summarized") },
            |_| panic!("the replacement must not be audited"),
        )
        .unwrap_err();
    assert_eq!(err.code(), "group_membership");
    assert!(store.group(&replacement).unwrap().closed.is_none());
    assert!(store.group(&old).unwrap().closed.is_some());
}

#[test]
fn schema_seventeen_replaces_trial_registrations_and_preserves_trial_rows() {
    let dir = private_tempdir();
    let root = dir.path();
    let store = Store::open(root);
    let author = crate::Author::agent("opus".parse().unwrap());
    let mut entries = [
        crate::tests::entry("decision-14", T0),
        crate::tests::entry("open-item-4", T0),
    ];
    for entry in &mut entries {
        entry.author = author.clone();
        entry.content_from = crate::ContentFrom::Agent;
        entry.cite = None;
        entry.said.clear();
        entry.content = format!("Trial entry {} with prose fields", entry.id);
    }
    entries[1].status = crate::EntryStatus::Retired;
    entries[1].inactive_since = Some(T0);
    entries[1].retirement_reason = Some("closed: recorded in prose".to_owned());
    let tags = [
        ("decision", vec!["decisions"]),
        ("open-item", vec!["open-items", "ask"]),
        ("notes", vec!["note"]),
    ]
    .map(|(name, aliases)| crate::StoreTag {
        name: name.parse().unwrap(),
        aliases: aliases
            .into_iter()
            .map(|alias| alias.parse().unwrap())
            .collect(),
        description: format!("Trial {name}"),
        stale_after_days: None,
        author: author.clone(),
        updated_by: None,
        created: T0,
        updated: T0,
    });
    store
        .write(|_| {
            for entry in &entries {
                store.write_entry(entry, true)?;
            }
            store.write_tags(&tags)
        })
        .unwrap();
    db::execute(
        root,
        "ALTER TABLE group_members DROP COLUMN restarted; ALTER TABLE entries DROP COLUMN decision; ALTER TABLE entries DROP COLUMN item;
         PRAGMA user_version = 16;",
    )
    .unwrap();

    let read_trial = || {
        for entry in &entries {
            assert_eq!(store.entry(&entry.id).unwrap(), *entry);
            assert_eq!(
                store.tag_definition(entry.tag.as_str()).unwrap().1,
                "config"
            );
        }
        assert_eq!(store.store_tags().unwrap(), [tags[2].clone()]);
        assert_eq!(
            store.tag_definition("ask").unwrap_err().code(),
            "tag_not_found"
        );
        assert_eq!(store.load_entries().unwrap().0, entries);
    };
    read_trial();
    assert_eq!(
        db::meta(root).unwrap().unwrap().0,
        16,
        "reads never migrate"
    );

    store
        .write_messages(
            &[message("s-1", "m-1", "write after trial")],
            &Redactor::default(),
        )
        .unwrap();
    assert_eq!(db::meta(root).unwrap().unwrap().0, 20);
    read_trial();
    let conn = rusqlite::Connection::open(root.join(DB_FILE)).unwrap();
    let names: String = conn
        .query_row("SELECT group_concat(name) FROM agent_tags", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(names, "notes");
}

#[test]
fn schema_nineteen_marks_possible_restorations_and_preserves_existing_marks() {
    for trailing_gap in [false, true] {
        let dir = private_tempdir();
        let store = Store::open(dir.path());
        store
            .write_messages(&[message("setup", "m", "setup")], &Redactor::default())
            .unwrap();
        let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
        conn.execute_batch(
            "INSERT INTO groups (id, title, kind, project, opened, opener, related)
             VALUES (1, 'group', 'work', 'project', 'now', 'opener', '[]');
             INSERT INTO group_seats (group_id, name, role) VALUES
                 (1, 'shared', 'member'), (1, 'other', 'member'),
                 (1, 'later', 'member'), (1, 'marked', 'member');
             INSERT INTO group_members (position, group_id, seat, session, author, restarted) VALUES
                 (1, 1, 'shared', 's1', 'agent', 0),
                 (2, 1, 'other', 's2', 'agent', 0),
                 (3, 1, 'other', 's3', 'agent', 0),
                 (4, 1, 'shared', 's4', 'agent', 0),
                 (5, 1, 'later', 's5', 'agent', 0),
                 (6, 1, 'marked', 's6', 'agent', 1);
             DELETE FROM group_members WHERE position = 3;
             PRAGMA user_version = 18;",
        )
        .unwrap();
        if trailing_gap {
            conn.execute_batch("INSERT INTO group_members (position, group_id, seat, session, author) VALUES (7, 1, 'later', 'deleted', 'agent'); DELETE FROM group_members WHERE position = 7;").unwrap();
        }
        drop(conn);
        store
            .write_messages(&[message("upgrade", "m", "upgrade")], &Redactor::default())
            .unwrap();
        assert_eq!(db::meta(dir.path()).unwrap().unwrap().0, 20);
        let conn = rusqlite::Connection::open(dir.path().join(DB_FILE)).unwrap();
        let marks: Vec<(i64, i64)> = conn
            .prepare("SELECT position, restarted FROM group_members ORDER BY position")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            marks,
            [
                (1, 0),
                (2, 1),
                (4, i64::from(trailing_gap)),
                (5, i64::from(trailing_gap)),
                (6, 1)
            ]
        );
    }
}
