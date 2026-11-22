//! Entries, claims, tags, marks, pickups and the audit log in `scribe.db`
//! (2_design.md §15.7, §15.11, phase 7): the move, `export`, and a write
//! and its audit line as one transaction.

use super::{SESSION_BODY, private_tempdir, session_text};
use crate::annotation::{MarkRecord, MarkState};
use crate::consistency::{Draft, ReapplyReport, Settle, Settled};
use crate::entry::{Cite, Entry, EntryStatus};
use crate::error::Error;
use crate::proposal::{Proposal, ProposalKind, ProposalState, Quote};
use crate::store::audit::{self, AuditEvent, AuditStatus};
use crate::store::{ExportFormat, Store, db, format};
use crate::{Author, StoreTag};
use std::fs;
use std::path::Path;
use time::OffsetDateTime;
use time::macros::datetime;

const T0: OffsetDateTime = datetime!(2026-10-01 09:00 UTC);
const NOW: OffsetDateTime = datetime!(2026-11-02 06:00 UTC);

/// One audit line of each format, 1 to 5, the issue fields of formats 4
/// and 5 included: the lines a format-26 store's `audit.jsonl` holds.
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

/// A format-26 file store holding one record of every kind.
fn file_store(root: &Path) {
    put(
        root,
        "sessions/2026/11/s-1.md",
        session_text("", SESSION_BODY).as_bytes(),
    );
    let older = entry("preferences-1", 0, "the release train leaves on Tuesdays");
    let newer = entry("preferences-2", 1, "the release train leaves on Thursdays");
    for e in [&older, &newer] {
        let rel = format!("entries/preferences/{}.md", e.id);
        put(
            root,
            &rel,
            &format::encode_entry(&root.join(&rel), e).unwrap(),
        );
    }
    put(
        root,
        "proposals.json",
        &format::encode_proposals(&root.join("proposals.json"), &[supersedes()]).unwrap(),
    );
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
    put(
        root,
        "tags.json",
        &format::encode_tags(&root.join("tags.json"), &[tag]).unwrap(),
    );
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
    put(
        root,
        "marks.json",
        &format::encode_marks(&root.join("marks.json"), &[mark]).unwrap(),
    );
    let pickup = crate::action::Pickup {
        entry: "preferences-2".parse().unwrap(),
        author: Author::agent("opus".parse().unwrap()),
        at: T0,
        result: None,
        rejected: false,
    };
    put(
        root,
        "pickups.json",
        &format::encode_pickups(&root.join("pickups.json"), &[pickup]).unwrap(),
    );
    put(root, "audit.jsonl", AUDIT_FIXTURE.as_bytes());
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

/// The audit fixture, decoded as the file store read it.
fn fixture_lines() -> Vec<(u32, AuditEvent)> {
    let (lines, torn) =
        audit::decode_formatted(Path::new("audit.jsonl"), AUDIT_FIXTURE.as_bytes()).unwrap();
    assert_eq!(torn, 1);
    assert_eq!(lines.len(), 5);
    lines
}

/// Every line of a format-26 `audit.jsonl`, formats 1 to 5 and their issue
/// fields included, reads back from the `audit` table at its own format,
/// and the torn line is counted, not imported.
#[test]
fn every_audit_line_reads_back_from_the_table_at_its_own_format() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    file_store(&root);
    let store = Store::open(&root);
    let moved = store.migrate_store(NOW).unwrap();
    assert_eq!(moved.audit_lines, 5);
    assert_eq!(moved.audit_torn, 1);
    assert!(!root.join("audit.jsonl").exists());
    assert!(root.join("pre-sqlite-2026-11-02/audit.jsonl").is_file());

    let expected = fixture_lines();
    let events: Vec<AuditEvent> = expected.iter().map(|(_, e)| e.clone()).collect();
    assert_eq!(store.audit_events().unwrap(), (events, 0));
    let issue = store.audit_events().unwrap().0[3].issue.clone().unwrap();
    assert_eq!(issue.verb, "add");
    assert_eq!(issue.occurrence, Some(1));
    let issue = store.audit_events().unwrap().0[4].issue.clone().unwrap();
    assert_eq!(issue.trigger.as_deref(), Some("root cause"));
    assert_eq!(issue.request_sha256, ["ab"]);

    let conn = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
    let formats: Vec<u32> = conn
        .prepare("SELECT format FROM audit ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(formats, [1, 2, 3, 4, 5]);
}

/// The move takes in every kind in one transaction, and the records read
/// the same after it; `export` writes every kind back out, and that export
/// opened as a store, and moved again, reads the same once more.
#[test]
fn export_round_trips_every_kind_through_the_move() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    file_store(&root);
    let store = Store::open(&root);
    let before = kinds(&store);
    assert_eq!(before.sessions.len(), 1);
    assert_eq!(before.entries.len(), 2);
    assert_eq!(before.claims.len(), 1);
    assert_eq!(before.tags.len(), 1);
    assert_eq!(before.marks.len(), 1);
    assert_eq!(before.pickups.len(), 1);
    assert_eq!(before.audit.len(), 5);

    let moved = store.migrate_store(NOW).unwrap();
    assert_eq!(
        (
            moved.sessions,
            moved.entries,
            moved.claims,
            moved.tags,
            moved.marks,
            moved.pickups
        ),
        (1, 2, 1, 1, 1, 1)
    );
    assert_eq!(moved.moved_to.as_deref(), Some("pre-sqlite-2026-11-02"));
    for gone in ["sessions", "entries"] {
        assert!(!root.join(gone).exists(), "{gone} moved aside");
    }
    assert_eq!(kinds(&store), before, "the move changes no record");

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
    let copy = Store::open(&out);
    assert_eq!(kinds(&copy), before, "the export reads as the store did");
    copy.migrate_store(NOW).unwrap();
    assert_eq!(kinds(&copy), before, "and moves in unchanged");

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
    file_store(&root);
    let store = Store::open(&root);
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
    // The rolled-back move left the files where they were.
    assert!(root.join("entries/preferences/preferences-1.md").is_file());

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
