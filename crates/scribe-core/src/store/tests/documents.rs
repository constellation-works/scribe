//! Entries, claims, tags, marks, pickups and the audit log in `scribe.db`
//! (2_design.md §15.7, §15.11): `export`, a write and its audit line as one
//! transaction, and `store migrate`'s archive import (phase 8).

use super::{Seed, private_tempdir, seed};
use crate::annotation::{MarkRecord, MarkState};
use crate::consistency::{Draft, ReapplyReport, Settle, Settled};
use crate::control::{self, Switch};
use crate::entry::{Cite, ContentFrom, Entry, EntryStatus};
use crate::error::Error;
use crate::proposal::{Proposal, ProposalKind, ProposalState, Quote};
use crate::store::audit::{self, AuditEvent, AuditStatus};
use crate::store::{ExportFormat, Store, codec, db};
use crate::{Author, StoreTag};
use std::fs;
use std::path::Path;
use time::OffsetDateTime;
use time::macros::datetime;

const T0: OffsetDateTime = datetime!(2026-10-01 09:00 UTC);
const NOW: OffsetDateTime = datetime!(2026-11-02 06:00 UTC);

/// One audit line of each format, 1 to 5, the issue fields of formats 4
/// and 5 included: the lines a phase-7 move took into the `audit` table.
const AUDIT_FIXTURE: &str = concat!(
    r#"{"format":1,"at":"2026-09-26T12:00:00Z","command":"hook stop","status":"success","target":null,"error_code":null,"duration_ms":3}"#,
    "\n",
    r#"{"format":2,"at":"2026-09-27T12:00:00Z","command":"proposal settle","status":"success","target":"p-1","error_code":null,"author":"opus","duration_ms":4}"#,
    "\n",
    r#"{"format":3,"at":"2026-10-01T12:00:00Z","command":"entry retire","status":"failure","target":"preferences-1","error_code":"entry_not_found","author":"opus","instruction_session":"s-1","instruction_message":"m-1","duration_ms":5}"#,
    "\n",
    r#"{"format":4,"at":"2026-10-20T12:00:00Z","command":"hook user-prompt-submit","status":"success","target":"issue-1","error_code":null,"author":null,"instruction_session":null,"instruction_message":null,"issue":{"verb":"add","id":"issue-1","occurrence":1,"session":"s-1","request_message":"m-1"},"duration_ms":6}"#,
    "\n",
    r#"{"format":5,"at":"2026-11-21T09:00:00Z","command":"hook user-prompt-submit","status":"success","target":"session:s-1","error_code":null,"author":null,"issue":{"verb":"fire","session":"s-1","trigger":"root cause","request_sha256":["ab"]},"duration_ms":2}"#,
    "\n",
    // A torn last line, as a crash mid-append leaves: skipped and counted.
    r#"{"format":5,"at":"2026-11-21T09:"#,
);

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

fn entry(id: &str, days: i64, content: &str) -> Entry {
    let mut e = crate::tests::entry(id, T0 + time::Duration::days(days));
    e.content = content.to_owned();
    e.said = format!("#scribe {}: {content}", e.tag);
    e
}

fn supersedes() -> Proposal {
    Draft {
        kind: ProposalKind::Supersedes,
        older: "preferences-1".parse().unwrap(),
        newer: Some("preferences-2".parse().unwrap()),
        reason: String::from("the newer entry replaces it"),
        quotes: vec![Quote {
            entry: "preferences-1".parse().unwrap(),
            text: String::from("the release train leaves on Tuesdays"),
        }],
    }
    .into_proposal("p-1".parse().unwrap(), T0)
}

fn cite() -> Cite {
    Cite {
        session: "s-9".parse().unwrap(),
        message: Some(String::from("m-9")),
        at: NOW,
        prompt_sha256: "1".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    }
}

/// A store holding one record of every kind in `scribe.db`.
fn seeded(root: &Path) -> Store {
    let older = entry("preferences-1", 0, "the release train leaves on Tuesdays");
    let newer = entry("preferences-2", 1, "the release train leaves on Thursdays");
    let tag = StoreTag {
        name: "toolchain".parse().unwrap(),
        aliases: Vec::new(),
        description: String::from("the build toolchain"),
        stale_after_days: None,
        author: Author::agent("opus".parse().unwrap()),
        updated_by: None,
        created: T0,
        updated: T0,
    };
    let mark = MarkRecord {
        session: "s-1".parse().unwrap(),
        message: String::from("m-1"),
        prompt_sha256: "a".repeat(64),
        marker: 1,
        outcome: MarkState::Entry,
        entry: Some("preferences-2".parse().unwrap()),
        code: None,
        attempts: 1,
        at: T0,
    };
    let pickup = crate::action::Pickup {
        entry: "preferences-2".parse().unwrap(),
        author: Author::agent("opus".parse().unwrap()),
        at: T0,
        result: None,
        rejected: false,
    };
    seed(
        root,
        &Seed {
            sessions: vec![crate::tests::session("s-1", T0)],
            entries: vec![older, newer],
            claims: vec![supersedes()],
            tags: vec![tag],
            marks: vec![mark],
            pickups: vec![pickup],
            audit: fixture_lines(),
            ..Seed::default()
        },
    )
}

/// An archive directory `name` a phase-7 move left: one entry file and an
/// `audit.jsonl` of `lines` lines.
fn archive(root: &Path, name: &str, lines: usize) {
    let e = entry("preferences-1", 0, "the release train leaves on Tuesdays");
    let rel = format!("{name}/entries/preferences/preferences-1.md");
    put(
        root,
        &rel,
        &codec::encode_entry(&root.join(&rel), &e).unwrap(),
    );
    let audit: String = AUDIT_FIXTURE
        .lines()
        .take(lines)
        .map(|l| format!("{l}\n"))
        .collect();
    put(root, &format!("{name}/audit.jsonl"), audit.as_bytes());
}

/// Every record kind as the store reads it.
#[derive(Debug, PartialEq)]
struct Kinds {
    sessions: Vec<crate::SessionRecord>,
    entries: Vec<Entry>,
    claims: Vec<Proposal>,
    tags: Vec<StoreTag>,
    marks: Vec<MarkRecord>,
    pickups: Vec<crate::action::Pickup>,
    audit: Vec<AuditEvent>,
}

fn kinds(store: &Store) -> Kinds {
    let mut sessions = store.load_sessions().unwrap().records;
    sessions.sort_by(|a, b| a.id.cmp(&b.id));
    let (mut entries, skipped) = store.load_entries().unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    Kinds {
        sessions,
        entries,
        claims: store.proposals().unwrap(),
        tags: store.store_tags().unwrap(),
        marks: store.all_marks().unwrap(),
        pickups: store.pickups().unwrap(),
        audit: store.audit_events().unwrap().0,
    }
}

/// The audit fixture's whole lines, decoded.
fn fixture_lines() -> Vec<(u32, AuditEvent)> {
    let (lines, torn) =
        audit::decode_formatted(Path::new("audit.jsonl"), AUDIT_FIXTURE.as_bytes()).unwrap();
    assert_eq!(torn, 1);
    assert_eq!(lines.len(), 5);
    lines
}

fn exported_audit_lines(root: &Path) -> Vec<(u32, AuditEvent)> {
    let path = root.join("audit.jsonl");
    let (lines, torn) = audit::decode_formatted(&path, &fs::read(&path).unwrap()).unwrap();
    assert_eq!(torn, 0);
    lines
}

fn audit_table_formats(root: &Path) -> Vec<u32> {
    let conn = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
    conn.prepare("SELECT format FROM audit ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Every audit line of formats 1 to 5, their issue fields included, reads
/// back from the `audit` table at its own format.
#[test]
fn every_audit_line_reads_back_from_the_table_at_its_own_format() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = seeded(&root);

    let expected = fixture_lines();
    let events: Vec<AuditEvent> = expected.iter().map(|(_, e)| e.clone()).collect();
    assert_eq!(store.audit_events().unwrap(), (events, 0));
    let issue = store.audit_events().unwrap().0[3].issue.clone().unwrap();
    assert_eq!(issue.verb, "add");
    assert_eq!(issue.occurrence, Some(1));
    let issue = store.audit_events().unwrap().0[4].issue.clone().unwrap();
    assert_eq!(issue.trigger.as_deref(), Some("root cause"));
    assert_eq!(issue.request_sha256, ["ab"]);

    assert_eq!(audit_table_formats(&root), [1, 2, 3, 4, 5]);
}

/// `export` writes every kind out in both shapes. A Markdown export is not
/// a store this build reads: opened as one, it is refused, not read empty.
#[test]
fn export_writes_every_kind_and_is_not_read_back_as_a_store() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = seeded(&root);
    let before = kinds(&store);
    assert_eq!(before.sessions.len(), 1);
    assert_eq!(before.entries.len(), 2);
    assert_eq!(before.claims.len(), 1);
    assert_eq!(before.tags.len(), 1);
    assert_eq!(before.marks.len(), 1);
    assert_eq!(before.pickups.len(), 1);
    assert_eq!(before.audit.len(), 5);

    let out = dir.path().join("export");
    let exported = store.export(&out, ExportFormat::Markdown).unwrap();
    assert_eq!(
        (
            exported.sessions,
            exported.entries,
            exported.claims,
            exported.audit_lines
        ),
        (1, 2, 1, 5)
    );
    for rel in [
        "entries/preferences/preferences-1.md",
        "proposals.json",
        "tags.json",
        "marks.json",
        "pickups.json",
        "audit.jsonl",
    ] {
        assert!(exported.files.iter().any(|f| f == rel), "{rel} exported");
    }
    assert_eq!(exported_audit_lines(&out), fixture_lines());
    assert_eq!(kinds(&store), before, "an export changes no record");
    let err = Store::open(&out).load_entries().unwrap_err();
    assert_eq!(err.code(), "store_corrupt", "{err:?}");
    assert!(
        format!("{err:?}").contains("earlier scribe build"),
        "{err:?}"
    );

    let jsonl = dir.path().join("jsonl");
    let rows = store.export(&jsonl, ExportFormat::Jsonl).unwrap();
    assert_eq!(
        (rows.sessions, rows.entries, rows.claims, rows.audit_lines),
        (1, 2, 1, 5)
    );
    for table in [
        "entries",
        "claims",
        "agent_tags",
        "marks",
        "pickups",
        "audit",
    ] {
        let text = fs::read_to_string(jsonl.join(format!("{table}.jsonl"))).unwrap();
        assert!(!text.is_empty(), "{table}.jsonl has its rows");
    }
}

/// An approval and its audit line commit together or not at all, so
/// `reapply_approved`, which repaired a crash between the two files an
/// approval wrote, finds nothing to repair.
#[test]
fn an_approval_and_its_audit_line_commit_together() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = seeded(&root);
    let line = AuditEvent {
        at: NOW,
        command: String::from("proposal settle"),
        status: AuditStatus::Success,
        target: Some(String::from("p-1")),
        error_code: None,
        author: None,
        instruction_session: None,
        instruction_message: None,
        issue: None,
        duration_ms: 1,
    };
    let lines = |store: &Store| {
        store
            .audit_events()
            .unwrap()
            .0
            .iter()
            .filter(|e| e.at == NOW)
            .count()
    };

    // A failure after the approval, before the commit: nothing is kept.
    let failed: Result<(), Error> = store.transaction(|| {
        store.settle(&Settle::Approve("p-1".parse().unwrap()), &cite(), NOW)?;
        store.record_audit(&line)?;
        Err(Error::corrupt(&root, "a failure after the approval"))
    });
    assert!(failed.is_err());
    assert_eq!(store.proposals().unwrap()[0].state, ProposalState::Open);
    let older = store.entry(&"preferences-1".parse().unwrap()).unwrap();
    assert_eq!(older.status, EntryStatus::Active);
    assert_eq!(lines(&store), 0);

    let settled = store
        .transaction(|| {
            let settled = store.settle(&Settle::Approve("p-1".parse().unwrap()), &cite(), NOW)?;
            store.record_audit(&line)?;
            Ok::<_, Error>(settled)
        })
        .unwrap();
    assert!(matches!(settled, Settled::Approved { .. }), "{settled:?}");
    assert_eq!(store.proposals().unwrap()[0].state, ProposalState::Approved);
    let older = store.entry(&"preferences-1".parse().unwrap()).unwrap();
    assert_eq!(older.status, EntryStatus::Superseded);
    assert_eq!(lines(&store), 1);
    assert_eq!(
        store.reapply_approved(NOW).unwrap(),
        ReapplyReport::default(),
        "nothing to repair"
    );
}

/// `store migrate` imports an archive in the write's transaction: a
/// failure after it rolls back the import and its audit line and keeps
/// the directory; success commits both and removes the directory.
#[test]
fn migrate_and_its_audit_line_commit_or_roll_back_together() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = seeded(&root);
    archive(&root, "pre-sqlite-2026-10-01", 5);
    let line = AuditEvent {
        at: NOW,
        command: String::from("store migrate"),
        status: AuditStatus::Success,
        target: Some(String::from("store:records")),
        error_code: None,
        author: None,
        instruction_session: None,
        instruction_message: None,
        issue: None,
        duration_ms: 1,
    };
    let migrate_lines = |store: &Store| {
        store
            .audit_events()
            .unwrap()
            .0
            .into_iter()
            .filter(|e| e.at == NOW && e.command == "store migrate")
            .count()
    };
    let archived = |root: &Path| -> i64 {
        rusqlite::Connection::open(root.join(db::DB_FILE))
            .unwrap()
            .query_row("SELECT count(*) FROM archive_records", [], |row| row.get(0))
            .unwrap()
    };

    let failed: Result<(), Error> = store.transaction(|| {
        store.migrate_store(NOW)?;
        store.record_audit(&line)?;
        Err(Error::corrupt(&root, "interrupted after migrate"))
    });
    assert!(failed.is_err());
    assert!(root.join("pre-sqlite-2026-10-01/audit.jsonl").is_file());
    assert_eq!(archived(&root), 0);
    assert_eq!(migrate_lines(&store), 0);

    // Calling migrate_store after another write already began inside a
    // transaction is refused and rolls back that write.
    let nested = store.transaction(|| {
        store.settle(&Settle::Approve("p-1".parse().unwrap()), &cite(), NOW)?;
        store.migrate_store(NOW)?;
        Ok::<_, Error>(())
    });
    assert!(nested.is_err());
    assert_eq!(store.proposals().unwrap()[0].state, ProposalState::Open);
    assert!(root.join("pre-sqlite-2026-10-01").is_dir());

    let moved = store
        .transaction(|| {
            let moved = store.migrate_store(NOW)?;
            store.record_audit(&line)?;
            Ok::<_, Error>(moved)
        })
        .unwrap();
    assert_eq!((moved.archives, moved.archived), (1, 6));
    assert_eq!(moved.moved_to, None);
    assert!(!root.join("pre-sqlite-2026-10-01").exists());
    assert_eq!(archived(&root), 6);
    assert_eq!(migrate_lines(&store), 1);
    assert_eq!(kinds(&store).entries.len(), 2, "an archive is not a record");

    let again = store.migrate_store(NOW).unwrap_err();
    assert_eq!(again.code(), "migrate_blocked", "{again:?}");
}

/// A pending archive erasure a phase-7 forget left is replayed when the
/// archive is imported; one that does not read fails the write and is kept.
#[test]
fn corrupt_pending_archive_forget_row_fails_closed_without_dropping() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = seeded(&root);
    archive(&root, "pre-sqlite-2026-10-01", 1);

    db::execute(
        &root,
        "INSERT INTO pending_archive_forgets (entry, salt, digests, created)
         VALUES ('preferences-1', '0000000000000000000000000000000000000000000000000000000000000000',
                 '[{\"len\":4,\"sha256\":\"not-hex\"}]', '2026-11-02T06:00:00Z')",
    )
    .unwrap();

    let err = store
        .update_control(|s| control::switch(s, &"s-9".parse().unwrap(), &Switch::On, NOW))
        .unwrap_err();
    assert_eq!(err.code(), "store_corrupt", "{err:?}");

    let conn = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
    let remaining: i64 = conn
        .query_row("SELECT count(*) FROM pending_archive_forgets", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(remaining, 1, "corrupt obligation must not be dropped");
    assert!(root.join("pre-sqlite-2026-10-01").is_dir());
}

#[test]
fn export_includes_pending_spooled_audit_once_and_skips_drained_spool() {
    use crate::store::audit::IssueAudit;
    use std::time::Duration;

    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = seeded(&root);

    // Hold a write lock so record_audit spools a pending event instead of
    // writing to scribe.db.
    let holder = db::open_new_writer(&root, Duration::from_secs(1)).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let pending = AuditEvent {
        at: NOW,
        command: String::from("entry retire"),
        status: AuditStatus::Failure,
        target: Some(String::from("preferences-1")),
        error_code: Some(String::from("not_interactive")),
        author: Some(String::from("opus")),
        instruction_session: Some(String::from("s-9")),
        instruction_message: Some(String::from("m-10")),
        issue: Some(IssueAudit {
            verb: String::from("add"),
            id: Some(String::from("issue-1")),
            occurrence: Some(1),
            session: Some(String::from("s-9")),
            request_message: Some(String::from("m-10")),
            moved_to: None,
            trigger: None,
            request_sha256: vec![String::from("cd")],
        }),
        duration_ms: 9,
    };
    store.record_audit(&pending).unwrap();
    holder.execute_batch("ROLLBACK").unwrap();
    drop(holder);

    let spool_dir = root.join(crate::store::write::AUDIT_SPOOL_DIR);
    let spool_files: Vec<String> = fs::read_dir(&spool_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(spool_files.len(), 1, "{spool_files:?}");
    let pending_name = spool_files[0].clone();
    let pending_bytes = fs::read(spool_dir.join(&pending_name)).unwrap();

    // Before the next write, JSONL export merges the pending spooled event
    // into audit.jsonl and counts it in audit_lines.
    let jsonl_pending = dir.path().join("jsonl-pending");
    let exported = store.export(&jsonl_pending, ExportFormat::Jsonl).unwrap();
    assert_eq!(exported.audit_lines, 6);
    let rows: Vec<serde_json::Value> = fs::read_to_string(jsonl_pending.join("audit.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 6);
    let spooled_row = &rows[5];
    assert_eq!(spooled_row["id"], serde_json::Value::Null);
    assert_eq!(spooled_row["format"], 5);
    assert_eq!(spooled_row["command"], "entry retire");
    assert_eq!(spooled_row["status"], "failure");
    assert_eq!(spooled_row["target"], "preferences-1");
    assert_eq!(spooled_row["error_code"], "not_interactive");
    assert_eq!(spooled_row["author"], "opus");
    assert_eq!(spooled_row["instruction_session"], "s-9");
    assert_eq!(spooled_row["instruction_message"], "m-10");
    assert_eq!(spooled_row["duration_ms"], 9);
    assert_eq!(spooled_row["spool"], pending_name);
    let decoded_issue: IssueAudit =
        serde_json::from_str(spooled_row["issue"].as_str().unwrap()).unwrap();
    assert_eq!(decoded_issue, pending.issue.clone().unwrap());

    // Drain the spool via a write, then put the already-drained spool file
    // back on disk (simulating a crash before spool file removal) alongside a
    // second still-pending spool file.
    store
        .update_control(|s| control::switch(s, &"s-2".parse().unwrap(), &Switch::On, NOW))
        .unwrap();
    assert!(!spool_dir.join(&pending_name).exists());
    put(
        &root,
        &format!("{}/{pending_name}", crate::store::write::AUDIT_SPOOL_DIR),
        &pending_bytes,
    );

    let holder = db::open_new_writer(&root, Duration::from_secs(1)).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let second = AuditEvent {
        at: NOW,
        command: String::from("entry add"),
        status: AuditStatus::Failure,
        target: None,
        error_code: Some(String::from("reserved_author")),
        author: Some(String::from("user")),
        instruction_session: None,
        instruction_message: None,
        issue: None,
        duration_ms: 4,
    };
    store.record_audit(&second).unwrap();
    holder.execute_batch("ROLLBACK").unwrap();
    drop(holder);

    let jsonl_mixed = dir.path().join("jsonl-mixed");
    let exported = store.export(&jsonl_mixed, ExportFormat::Jsonl).unwrap();
    assert_eq!(exported.audit_lines, 7);
    let rows: Vec<serde_json::Value> = fs::read_to_string(jsonl_mixed.join("audit.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 7);
    let matching_drained: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| r["spool"].as_str() == Some(pending_name.as_str()))
        .collect();
    assert_eq!(
        matching_drained.len(),
        1,
        "already-drained spool file must not be duplicated: {matching_drained:?}"
    );
    assert!(
        matching_drained[0]["id"].is_i64(),
        "drained row carries its SQLite row id: {:?}",
        matching_drained[0]
    );
    assert_eq!(matching_drained[0]["issue"], spooled_row["issue"]);
    assert_eq!(rows[6]["id"], serde_json::Value::Null);
    assert_eq!(rows[6]["command"], "entry add");
    assert_eq!(rows[6]["error_code"], "reserved_author");
    assert!(rows[6]["spool"].as_str().is_some());

    // A spool file from an older writer keeps its format too.
    let legacy_line = AUDIT_FIXTURE.lines().next().unwrap();
    put(&root, "audit-spool/000-legacy.json", legacy_line.as_bytes());
    let markdown_mixed = dir.path().join("markdown-mixed");
    let exported = store
        .export(&markdown_mixed, ExportFormat::Markdown)
        .unwrap();
    assert_eq!(exported.audit_lines, 8);
    let mut expected = fixture_lines();
    let legacy = expected[0].clone();
    expected.extend([(5, pending), legacy, (5, second.clone())]);
    assert_eq!(exported_audit_lines(&markdown_mixed), expected);

    // A store that never wrote, with only a spooled audit file, exports
    // that line: there is no file store to refuse.
    let fresh_dir = private_tempdir();
    let fresh_root = fresh_dir.path().join("store");
    let fresh = Store::open(&fresh_root);
    fresh.record_audit(&second).unwrap();
    let exported = fresh
        .export(&fresh_dir.path().join("jsonl"), ExportFormat::Jsonl)
        .unwrap();
    assert_eq!(exported.audit_lines, 1);
}
#[test]
fn put_entry_refuses_unreadable_spoken_entry_without_writing() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = seed(&root, &Seed::default());
    let mut invalid = crate::tests::entry("preferences-1", T0);
    invalid.author = Author::agent("opus".parse().unwrap());
    invalid.content_from = ContentFrom::Agent;
    invalid.cite = None;
    invalid.said.clear();
    invalid.quote = Some(String::from("short updates please"));
    invalid.stated_by = Some(Author::agent("sonnet".parse().unwrap()));
    invalid.updated_by = None;
    let conn = db::open_writer(&root, store.lock_wait).unwrap();
    let db_path = root.join("scribe.db");
    let result = db::docs::put_entry(&conn, &db_path, &invalid, true, |e| {
        db::db_error("write", &db_path, e)
    });
    assert!(matches!(result, Err(Error::Corrupt { .. })), "{result:?}");
    let (entries, skipped) = store.load_entries().unwrap();
    assert!(entries.is_empty() && skipped.is_empty());
}
#[test]
fn migrate_counts_spool_arriving_before_drain() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = seeded(&root);
    archive(&root, "pre-sqlite-2026-10-01", 0);
    let line = AUDIT_FIXTURE.lines().next().unwrap().as_bytes().to_vec();
    let late_root = root.clone();
    crate::store::write::BEFORE_SPOOL_DRAIN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            put(&late_root, "audit-spool/late.json", &line);
        }));
    });
    let moved = store.migrate_store(NOW).unwrap();
    assert_eq!(moved.audit_lines, 1);
    assert_eq!(store.audit_events().unwrap().0.len(), 6);
}
#[test]
fn migrate_does_not_count_replayed_spool_key() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = seeded(&root);
    archive(&root, "pre-sqlite-2026-10-01", 0);
    let (format, event) = fixture_lines().remove(0);
    let line = AUDIT_FIXTURE.lines().next().unwrap().as_bytes().to_vec();
    put(&root, "audit-spool/replayed.json", &line);
    let replay_root = root.clone();
    crate::store::write::BEFORE_SPOOL_DRAIN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let conn = db::open_writer(&replay_root, std::time::Duration::from_secs(1)).unwrap();
            let db_path = replay_root.join(db::DB_FILE);
            db::docs::put_audit(
                &conn,
                &db_path,
                format,
                &event,
                Some("replayed.json"),
                |e| db::db_error("test", &db_path, e),
            )
            .unwrap();
        }));
    });
    let moved = store.migrate_store(NOW).unwrap();
    assert_eq!(moved.audit_lines, 0);
    assert_eq!(store.audit_events().unwrap().0.len(), 6);
    assert!(!root.join("audit-spool/replayed.json").exists());
}
