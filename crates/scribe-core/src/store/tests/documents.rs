//! Entries, claims, tags, marks, pickups and the audit log in `scribe.db`
//! (2_design.md §15.7, §15.11, phase 7): the move, `export`, and a write
//! and its audit line as one transaction.

use super::{SESSION_BODY, on_disk, private_tempdir, session_text};
use crate::annotation::{MarkRecord, MarkState};
use crate::consistency::{Draft, ReapplyReport, Settle, Settled};
use crate::control::{self, Switch};
use crate::entry::{Cite, ContentFrom, Entry, EntryStatus};
use crate::error::Error;
use crate::instruction::Instructed;
use crate::proposal::{EntryCandidate, Proposal, ProposalKind, ProposalState, Quote};
use crate::record::{Label, Project};
use crate::store::audit::{self, AuditEvent, AuditStatus};
use crate::store::{ExportFormat, Store, db, format};
use crate::{Author, Instruction, Speaker, StoreTag};
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

#[test]
fn migrate_and_its_audit_line_commit_or_roll_back_together() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    file_store(&root);
    let store = Store::open(&root);
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

    // Failure after migrate_store and record_audit rolls back both.
    let failed: Result<(), Error> = store.transaction(|| {
        store.migrate_store(NOW)?;
        store.record_audit(&line)?;
        Err(Error::corrupt(&root, "interrupted after migrate"))
    });
    assert!(failed.is_err());
    assert!(root.join("sessions/2026/11/s-1.md").is_file());
    assert!(root.join("entries/preferences/preferences-1.md").is_file());
    assert!(!root.join("pre-sqlite-2026-11-02").exists());
    assert_eq!(migrate_lines(&store), 0);

    // Calling migrate_store after another write already began inside a
    // transaction is still refused and rolls back that write.
    let nested = store.transaction(|| {
        store.settle(&Settle::Approve("p-1".parse().unwrap()), &cite(), NOW)?;
        store.migrate_store(NOW)?;
        Ok::<_, Error>(())
    });
    assert!(nested.is_err());
    assert_eq!(store.proposals().unwrap()[0].state, ProposalState::Open);
    assert!(root.join("sessions/2026/11/s-1.md").is_file());
    assert!(!root.join("pre-sqlite-2026-11-02").exists());

    // Success commits the migration and its audit row together, moves the
    // files aside after commit, and populates moved_to.
    let moved = store
        .transaction(|| {
            let moved = store.migrate_store(NOW)?;
            store.record_audit(&line)?;
            Ok::<_, Error>(moved)
        })
        .unwrap();
    assert_eq!(moved.moved_to.as_deref(), Some("pre-sqlite-2026-11-02"));
    assert!(!root.join("sessions").exists());
    assert!(!root.join("entries").exists());
    assert!(
        root.join("pre-sqlite-2026-11-02/sessions/2026/11/s-1.md")
            .is_file()
    );
    assert_eq!(migrate_lines(&store), 1);
}

fn candidate_proposal(
    id: &str,
    entry_id: &str,
    state: ProposalState,
    quote: &str,
    context: &str,
    reason: &str,
    inst_quote: Option<&str>,
) -> Proposal {
    let inst = inst_quote.map(|q| Instruction {
        session: "s-9".parse().unwrap(),
        message: String::from("m-9"),
        at: NOW,
        quote: Some(q.to_owned()),
    });
    let approved = state == ProposalState::Approved;
    Proposal {
        id: id.parse().unwrap(),
        kind: ProposalKind::Candidate,
        older: entry_id.parse().unwrap(),
        newer: None,
        reason: reason.to_owned(),
        quotes: Vec::new(),
        state,
        opened_at: T0,
        settled_at: if approved || state == ProposalState::Rejected {
            Some(T0)
        } else {
            None
        },
        settled_by: inst.as_ref().map(|_| cite()),
        author: Author::session_pass(),
        updated_by: None,
        settled_author: inst
            .as_ref()
            .map(|_| Author::agent("opus".parse().unwrap())),
        closed_reason: None,
        instruction: inst,
        candidate: Some(EntryCandidate {
            tag: "preferences".parse().unwrap(),
            role: Speaker::User,
            quote: quote.to_owned(),
            context: Some(context.to_owned()),
            statement: None,
            subject: Some(format!("subject of {entry_id}")),
            evidence: None,
            cite: cite(),
            project: Project::sanitized("scribe"),
            provider: Label::sanitized("claude-code", "claude-code"),
            host: Label::sanitized("host", "host"),
            admitted: None,
        }),
    }
}

#[test]
fn forget_removes_entry_and_claim_words_from_moved_files() {
    let dir = private_tempdir();
    let root = dir.path().join("store");

    let entry_quote = "Tuesday release train verbatim quote";
    let entry_context = "Tuesday release train context paragraph";
    let cand_reason = "candidate reason for preferences-1";
    let inst_quote = "approve p-1, Tuesday release train instruction";
    let supers_reason = "Thursday replaces Tuesday in release train";
    let kept_rejected_quote = "rejected claim quote stays for suppression";
    let kept_open_quote = "open claim quote stays untouched";
    let kept_supers_reason = "unrelated supersedes reason stays";

    let mut e1 = entry("preferences-1", 0, entry_context);
    e1.content_from = ContentFrom::Message {
        role: Speaker::User,
        message: String::from("m-9"),
        at: T0,
    };
    e1.said = String::from("scribe proposal settle p-1 --approve --author opus");
    e1.quote = Some(entry_quote.to_owned());
    e1.stated_by = Some(Author::session_pass());
    e1.approved_by = Some(Author::agent("opus".parse().unwrap()));
    e1.instruction = Some(Instruction {
        session: "s-9".parse().unwrap(),
        message: String::from("m-9"),
        at: NOW,
        quote: Some(inst_quote.to_owned()),
    });
    let e2 = entry("preferences-2", 1, "the release train leaves on Thursdays");

    for e in [&e1, &e2] {
        let rel = format!("entries/preferences/{}.md", e.id);
        put(
            &root,
            &rel,
            &format::encode_entry(&root.join(&rel), e).unwrap(),
        );
    }

    let p1 = candidate_proposal(
        "p-1",
        "preferences-1",
        ProposalState::Approved,
        entry_quote,
        entry_context,
        cand_reason,
        Some(inst_quote),
    );
    let mut p2 = supersedes();
    p2.id = "p-2".parse().unwrap();
    p2.reason = supers_reason.to_owned();
    let p3 = candidate_proposal(
        "p-3",
        "preferences-3",
        ProposalState::Rejected,
        kept_rejected_quote,
        "rejected claim context stays",
        "rejected claim reason stays",
        None,
    );
    let p4 = candidate_proposal(
        "p-4",
        "preferences-4",
        ProposalState::Open,
        kept_open_quote,
        "open claim context stays",
        "open claim reason stays",
        None,
    );
    let p5 = Draft {
        kind: ProposalKind::Supersedes,
        older: "preferences-2".parse().unwrap(),
        newer: Some("preferences-5".parse().unwrap()),
        reason: kept_supers_reason.to_owned(),
        quotes: vec![Quote {
            entry: "preferences-2".parse().unwrap(),
            text: String::from("the release train leaves on Thursdays"),
        }],
    }
    .into_proposal("p-5".parse().unwrap(), T0);

    let proposals_bytes =
        format::encode_proposals(&root.join("proposals.json"), &[p1, p2, p3, p4, p5]).unwrap();
    put(&root, "proposals.json", &proposals_bytes);

    let audit_with_text = format!(
        "{AUDIT_FIXTURE}\n{}\n{}\n",
        r#"{"format":5,"at":"2026-10-20T12:00:00Z","command":"hook stop","status":"success","target":"preferences-1","detail":"Tuesday release train context paragraph","duration_ms":1}"#,
        r#"{"format":5,"at":"2026-10-20T12:01:00Z","command":"hook stop","status":"success","target":"p-1","detail":"Tuesday release train verbatim quote","duration_ms":1}"#,
    );
    put(&root, "audit.jsonl", audit_with_text.as_bytes());
    // Also place a second pre-sqlite-* directory to verify all set-aside dirs are cleaned.
    put(
        &root,
        "pre-sqlite-2026-10-15/entries.2/preferences/preferences-1.md",
        &format::encode_entry(&root.join("preferences-1.md"), &e1).unwrap(),
    );
    put(
        &root,
        "pre-sqlite-2026-10-15/proposals.json.2",
        &proposals_bytes,
    );

    let store = Store::open(&root);
    let moved = store.migrate_store(NOW).unwrap();
    assert_eq!(moved.moved_to.as_deref(), Some("pre-sqlite-2026-11-02"));

    let instructed = Instructed {
        session: "s-9".parse().unwrap(),
        message: String::from("m-10"),
        at: NOW,
        quote: String::from("forget preferences-1 for good"),
        prompt_sha256: "b".repeat(64),
        line: 1,
    };
    let (forgotten, changed) = store
        .forget_entry("preferences-1", &instructed, "opus".parse().unwrap(), NOW)
        .unwrap();
    assert!(changed);
    assert_eq!(forgotten.status, EntryStatus::Forgotten);

    // Entry files for preferences-1 are deleted across every pre-sqlite-* dir,
    // while unrelated entries stay.
    assert!(
        !root
            .join("pre-sqlite-2026-11-02/entries/preferences/preferences-1.md")
            .exists()
    );
    assert!(
        !root
            .join("pre-sqlite-2026-10-15/entries.2/preferences/preferences-1.md")
            .exists()
    );
    assert!(
        root.join("pre-sqlite-2026-11-02/entries/preferences/preferences-2.md")
            .is_file()
    );

    // None of the forgotten entry's content, quote, or claim text remains anywhere on disk.
    for gone in [
        entry_quote,
        entry_context,
        cand_reason,
        inst_quote,
        supers_reason,
        "the release train leaves on Tuesdays",
    ] {
        assert!(!on_disk(&root, gone), "{gone:?} remained on disk");
    }

    // Proposals in pre-sqlite-*/proposals.json match scribe.db: matching claims
    // rewritten identically, unrelated claims kept intact.
    let db_claims = store.proposals().unwrap();
    for rel in [
        "pre-sqlite-2026-11-02/proposals.json",
        "pre-sqlite-2026-10-15/proposals.json.2",
    ] {
        let moved_claims =
            format::decode_proposals(&root.join(rel), &fs::read(root.join(rel)).unwrap()).unwrap();
        assert_eq!(moved_claims, db_claims, "{rel}");
    }
    for kept in [
        kept_rejected_quote,
        kept_open_quote,
        kept_supers_reason,
        "the release train leaves on Thursdays",
    ] {
        assert!(on_disk(&root, kept), "{kept:?} should stay");
    }

    // Audit lines carrying the entry's text were dropped; normal lines (including
    // target: preferences-1 and the torn line) stay.
    let moved_audit = fs::read_to_string(root.join("pre-sqlite-2026-11-02/audit.jsonl")).unwrap();
    assert!(moved_audit.contains(r#""target":"preferences-1""#));
    assert!(moved_audit.contains(r#"{"format":5,"at":"2026-11-21T09:"#));
    assert!(!moved_audit.contains(entry_context));
    assert!(!moved_audit.contains(entry_quote));
}

#[test]
fn forget_keeps_structural_audit_lines_that_share_a_common_word() {
    let dir = private_tempdir();
    let root = dir.path().join("store");

    // Each body is a word the audit log also holds as a structural value: a
    // status, an author and a command. Forgetting it must keep the unrelated
    // structural lines, and still erase a free-text copy of the same word.
    for (id, days, body) in [
        ("preferences-1", 0, "success"),
        ("preferences-2", 1, "opus"),
        ("preferences-3", 2, "hook stop"),
    ] {
        let e = entry(id, days, body);
        let rel = format!("entries/preferences/{}.md", e.id);
        put(
            &root,
            &rel,
            &format::encode_entry(&root.join(&rel), &e).unwrap(),
        );
    }

    let structural = [
        r#"{"format":5,"at":"2026-10-20T12:00:00Z","command":"hook stop","status":"success","target":"session:s-9","error_code":null,"author":null,"duration_ms":1}"#,
        r#"{"format":5,"at":"2026-10-20T12:01:00Z","command":"proposal settle","status":"success","target":"p-1","error_code":null,"author":"opus","duration_ms":1}"#,
        r#"{"format":5,"at":"2026-10-20T12:02:00Z","command":"entry retire","status":"failure","target":"preferences-3","error_code":"entry_not_found","author":"opus","duration_ms":1}"#,
    ];
    let free_text = [
        r#"{"format":5,"at":"2026-10-20T12:03:00Z","command":"hook stop","status":"success","target":"session:s-9","error_code":null,"author":null,"detail":"success","duration_ms":1}"#,
        r#"{"format":5,"at":"2026-10-20T12:04:00Z","command":"hook stop","status":"success","target":"session:s-9","error_code":null,"author":null,"detail":"hook stop","duration_ms":1}"#,
    ];
    let audit: String = structural
        .iter()
        .chain(&free_text)
        .map(|line| format!("{line}\n"))
        .collect();
    put(&root, "audit.jsonl", audit.as_bytes());

    let store = Store::open(&root);
    let moved = store.migrate_store(NOW).unwrap();
    assert_eq!(moved.moved_to.as_deref(), Some("pre-sqlite-2026-11-02"));

    let instructed = Instructed {
        session: "s-9".parse().unwrap(),
        message: String::from("m-10"),
        at: NOW,
        quote: String::from("forget these for good"),
        prompt_sha256: "b".repeat(64),
        line: 1,
    };
    for id in ["preferences-1", "preferences-2", "preferences-3"] {
        let (forgotten, changed) = store
            .forget_entry(id, &instructed, "opus".parse().unwrap(), NOW)
            .unwrap();
        assert!(changed, "{id}");
        assert_eq!(forgotten.status, EntryStatus::Forgotten);
    }

    let moved_audit = fs::read_to_string(root.join("pre-sqlite-2026-11-02/audit.jsonl")).unwrap();
    let kept: Vec<&str> = moved_audit.lines().collect();
    for line in structural {
        assert!(
            kept.contains(&line),
            "an unrelated structural line was dropped: {line}"
        );
    }
    for line in free_text {
        assert!(!kept.contains(&line), "a free-text copy survived: {line}");
    }
}

#[test]
fn settle_forget_cleans_moved_files_and_drops_pre_move_open_candidate() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let open_quote = "pre-move open candidate quote";
    let open_context = "pre-move open candidate context";
    let kept_quote = "other rejected claim quote";

    let open = candidate_proposal(
        "p-1",
        "preferences-1",
        ProposalState::Open,
        open_quote,
        open_context,
        "open candidate reason",
        None,
    );
    let kept = candidate_proposal(
        "p-2",
        "preferences-2",
        ProposalState::Rejected,
        kept_quote,
        "other rejected context",
        "other rejected reason",
        None,
    );
    put(
        &root,
        "proposals.json",
        &format::encode_proposals(&root.join("proposals.json"), &[open, kept]).unwrap(),
    );

    let store = Store::open(&root);
    store.migrate_store(NOW).unwrap();

    // Approve p-1 after the move so preferences-1 exists in scribe.db while
    // pre-sqlite-*/proposals.json still holds p-1 as open, then forget via #scribe forget.
    let settled = store
        .settle(&Settle::Approve("p-1".parse().unwrap()), &cite(), NOW)
        .unwrap();
    assert!(matches!(settled, Settled::Recorded { .. }), "{settled:?}");
    assert!(on_disk(&root, open_quote));

    let forgot = store
        .settle(
            &Settle::Forget("preferences-1".parse().unwrap()),
            &cite(),
            NOW,
        )
        .unwrap();
    assert_eq!(forgot, Settled::Forgotten("preferences-1".parse().unwrap()));

    assert!(!on_disk(&root, open_quote));
    assert!(!on_disk(&root, open_context));

    let moved_path = root.join("pre-sqlite-2026-11-02/proposals.json");
    let moved_claims =
        format::decode_proposals(&moved_path, &fs::read(&moved_path).unwrap()).unwrap();
    assert_eq!(moved_claims.len(), 1);
    assert_eq!(moved_claims[0].id.to_string(), "p-2");
    assert_eq!(
        moved_claims[0].candidate.as_ref().unwrap().quote,
        kept_quote
    );
}

#[test]
fn interrupted_move_before_archive_rename_does_not_duplicate_audit_and_preserves_new_lines() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    file_store(&root);

    // Include a duplicate of an existing event in the original audit.jsonl so
    // we verify distinct identical lines are each preserved once (not collapsed
    // and not doubled on recovery).
    let expected = fixture_lines();
    let dup_line = r#"{"format":5,"at":"2026-11-21T09:00:00Z","command":"hook user-prompt-submit","status":"success","target":"session:s-1","error_code":null,"author":null,"issue":{"verb":"fire","session":"s-1","trigger":"root cause","request_sha256":["ab"]},"duration_ms":2}"#;
    let initial_events: Vec<AuditEvent> = {
        let mut v: Vec<AuditEvent> = expected.iter().map(|(_, e)| e.clone()).collect();
        let dup: AuditEvent = serde_json::from_str(dup_line).unwrap();
        v.push(dup);
        v
    };
    let initial_audit = format!(
        "{}\n{dup_line}\n",
        AUDIT_FIXTURE.lines().take(5).collect::<Vec<_>>().join("\n"),
    );
    put(&root, "audit.jsonl", initial_audit.as_bytes());

    let store = Store::open(&root);
    crate::store::write::CRASH_BEFORE_ASIDE.with(|c| c.set(true));
    let err = store.migrate_store(NOW).unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err:?}");
    // Migration committed, but move_files_aside never ran.
    assert!(root.join("audit.jsonl").is_file());
    assert!(!root.join("pre-sqlite-2026-11-02").exists());
    assert_eq!(store.audit_events().unwrap().0, initial_events);

    // An older build appends a genuinely new event before the next write,
    // which itself crashes before move_files_aside.
    let new_line_1 = r#"{"format":5,"at":"2026-11-22T01:00:00Z","command":"entry add","status":"success","target":"preferences-3","error_code":null,"duration_ms":7}"#;
    let new_event_1: AuditEvent = serde_json::from_str(new_line_1).unwrap();
    put(
        &root,
        "audit.jsonl",
        format!("{initial_audit}{new_line_1}\n").as_bytes(),
    );

    crate::store::write::CRASH_BEFORE_ASIDE.with(|c| c.set(true));
    let err = store
        .update_control(|s| control::switch(s, &"s-2".parse().unwrap(), &Switch::On, NOW))
        .unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err:?}");
    assert!(root.join("audit.jsonl").is_file());

    // Another genuinely new event arrives before the final recovery write.
    let new_line_2 = r#"{"format":5,"at":"2026-11-22T02:00:00Z","command":"entry add","status":"success","target":"preferences-4","error_code":null,"duration_ms":8}"#;
    let new_event_2: AuditEvent = serde_json::from_str(new_line_2).unwrap();
    put(
        &root,
        "audit.jsonl",
        format!("{initial_audit}{new_line_1}\n{new_line_2}\n").as_bytes(),
    );

    store
        .update_control(|s| control::switch(s, &"s-3".parse().unwrap(), &Switch::On, NOW))
        .unwrap();

    assert!(!root.join("audit.jsonl").exists());
    let moved_audit_present = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            e.file_name().to_string_lossy().starts_with("pre-sqlite-")
                && e.path().join("audit.jsonl").is_file()
        });
    assert!(moved_audit_present, "audit.jsonl moved aside");

    let mut want = initial_events;
    want.push(new_event_1);
    want.push(new_event_2);
    assert_eq!(store.audit_events().unwrap(), (want, 0));
}

#[test]
fn post_move_audit_jsonl_still_imports_new_events() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    file_store(&root);
    let store = Store::open(&root);
    store.migrate_store(NOW).unwrap();
    assert!(!root.join("audit.jsonl").exists());
    assert!(root.join("pre-sqlite-2026-11-02/audit.jsonl").is_file());

    let mut want: Vec<AuditEvent> = fixture_lines().into_iter().map(|(_, e)| e).collect();
    // Simulate a post-move audit.jsonl created at offset 0 by an older build.
    let late_line = r#"{"format":5,"at":"2026-11-22T03:00:00Z","command":"entry add","status":"failure","target":null,"error_code":"store_too_new","duration_ms":2}"#;
    let late_event: AuditEvent = serde_json::from_str(late_line).unwrap();
    put(&root, "audit.jsonl", format!("{late_line}\n").as_bytes());

    store
        .update_control(|s| control::switch(s, &"s-2".parse().unwrap(), &Switch::On, NOW))
        .unwrap();

    assert!(!root.join("audit.jsonl").exists());
    let moved_late_present = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            e.file_name().to_string_lossy().starts_with("pre-sqlite-")
                && e.path().join("audit.jsonl").is_file()
        });
    assert!(moved_late_present, "post-move audit.jsonl moved aside");
    want.push(late_event);
    assert_eq!(store.audit_events().unwrap(), (want, 0));
}

#[derive(Default)]
struct InterleaveState {
    first_read: bool,
    second_entered: bool,
    second_read: bool,
    first_done: bool,
}

fn run_interleaved_forgets(target_step: crate::store::documents::ForgetMovedStep) {
    use crate::store::documents::{ForgetMovedStep, install_forget_moved_hook};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    let dir = private_tempdir();
    let root = dir.path().join("store");

    let quote_1 = "first forgotten entry verbatim quote";
    let context_1 = "first forgotten entry context paragraph";
    let reason_1 = "candidate reason for preferences-1";
    let quote_2 = "second forgotten entry verbatim quote";
    let context_2 = "second forgotten entry context paragraph";
    let reason_2 = "candidate reason for preferences-2";
    let kept_quote = "unrelated third entry quote stays intact";
    let kept_detail = "unrelated audit free text stays intact";

    let make_entry = |id: &str, days: i64, context: &str, quote: &str, proposal_id: &str| {
        let mut e = entry(id, days, context);
        e.content_from = ContentFrom::Message {
            role: Speaker::User,
            message: String::from("m-9"),
            at: T0,
        };
        e.said = format!("scribe proposal settle {proposal_id} --approve --author opus");
        e.quote = Some(quote.to_owned());
        e.stated_by = Some(Author::session_pass());
        e.approved_by = Some(Author::agent("opus".parse().unwrap()));
        e.instruction = Some(Instruction {
            session: "s-9".parse().unwrap(),
            message: String::from("m-9"),
            at: NOW,
            quote: Some(format!("approve {proposal_id}: {quote}")),
        });
        e
    };
    let e1 = make_entry("preferences-1", 0, context_1, quote_1, "p-1");
    let e2 = make_entry("preferences-2", 1, context_2, quote_2, "p-2");
    let e3 = entry("preferences-3", 2, kept_quote);
    for e in [&e1, &e2, &e3] {
        let rel = format!("entries/preferences/{}.md", e.id);
        put(
            &root,
            &rel,
            &format::encode_entry(&root.join(&rel), e).unwrap(),
        );
    }

    let p1 = candidate_proposal(
        "p-1",
        "preferences-1",
        ProposalState::Approved,
        quote_1,
        context_1,
        reason_1,
        None,
    );
    let p2 = candidate_proposal(
        "p-2",
        "preferences-2",
        ProposalState::Approved,
        quote_2,
        context_2,
        reason_2,
        None,
    );
    let p3 = candidate_proposal(
        "p-3",
        "preferences-3",
        ProposalState::Open,
        kept_quote,
        "unrelated third entry context stays",
        "unrelated third entry reason stays",
        None,
    );
    put(
        &root,
        "proposals.json",
        &format::encode_proposals(&root.join("proposals.json"), &[p1, p2, p3]).unwrap(),
    );

    let audit = format!(
        "{AUDIT_FIXTURE}\n\
{{\"format\":5,\"at\":\"2026-10-20T11:59:00Z\",\"command\":\"entry retire\",\"status\":\"failure\",\"target\":\"preferences-2\",\"error_code\":\"entry_not_found\",\"author\":\"opus\",\"duration_ms\":1}}\n\
{{\"format\":5,\"at\":\"2026-10-20T12:00:00Z\",\"command\":\"hook stop\",\"status\":\"success\",\"target\":\"preferences-1\",\"detail\":\"{context_1}\",\"duration_ms\":1}}\n\
{{\"format\":5,\"at\":\"2026-10-20T12:01:00Z\",\"command\":\"hook stop\",\"status\":\"success\",\"target\":\"preferences-2\",\"detail\":\"{context_2}\",\"duration_ms\":1}}\n\
{{\"format\":5,\"at\":\"2026-10-20T12:02:00Z\",\"command\":\"hook stop\",\"status\":\"success\",\"target\":\"preferences-3\",\"detail\":\"{kept_detail}\",\"duration_ms\":1}}\n"
    );
    put(&root, "audit.jsonl", audit.as_bytes());

    let store = Store::open(&root);
    let moved = store.migrate_store(NOW).unwrap();
    assert_eq!(moved.moved_to.as_deref(), Some("pre-sqlite-2026-11-02"));

    let gate = Arc::new((Mutex::new(InterleaveState::default()), Condvar::new()));
    let overlapped = Arc::new(Mutex::new(false));
    let deadline = Duration::from_secs(5);
    let overlap_probe = Duration::from_millis(200);

    std::thread::scope(|s| {
        let store1 = store.clone();
        let gate1 = Arc::clone(&gate);
        let overlapped1 = Arc::clone(&overlapped);
        let t1 = s.spawn(move || {
            let _hook = install_forget_moved_hook({
                let gate = Arc::clone(&gate1);
                let overlapped = Arc::clone(&overlapped1);
                move |step| {
                    if step != target_step {
                        return;
                    }
                    let (lock, cv) = &*gate;
                    let mut st = lock.lock().unwrap();
                    st.first_read = true;
                    cv.notify_all();
                    let (st, _) = cv
                        .wait_timeout_while(st, deadline, |st| !st.second_entered)
                        .unwrap();
                    assert!(
                        st.second_entered,
                        "second forget did not commit and enter forget_moved_entry in time"
                    );
                    let (st, _) = cv
                        .wait_timeout_while(st, overlap_probe, |st| !st.second_read)
                        .unwrap();
                    *overlapped.lock().unwrap() = st.second_read;
                }
            });
            let instructed = Instructed {
                session: "s-9".parse().unwrap(),
                message: String::from("m-10"),
                at: NOW,
                quote: String::from("forget preferences-1 for good"),
                prompt_sha256: "b".repeat(64),
                line: 1,
            };
            let result =
                store1.forget_entry("preferences-1", &instructed, "opus".parse().unwrap(), NOW);
            let (lock, cv) = &*gate1;
            let mut st = lock.lock().unwrap();
            st.first_done = true;
            cv.notify_all();
            result
        });

        let store2 = store.clone();
        let gate2 = Arc::clone(&gate);
        let t2 = s.spawn(move || {
            {
                let (lock, cv) = &*gate2;
                let st = lock.lock().unwrap();
                let (st, _) = cv
                    .wait_timeout_while(st, deadline, |st| !st.first_read)
                    .unwrap();
                assert!(
                    st.first_read,
                    "first forget did not reach {target_step:?} in time"
                );
            }
            let _hook = install_forget_moved_hook({
                let gate = Arc::clone(&gate2);
                move |step| {
                    let (lock, cv) = &*gate;
                    if step == ForgetMovedStep::BeforeLock {
                        let mut st = lock.lock().unwrap();
                        st.second_entered = true;
                        cv.notify_all();
                    } else if step == target_step {
                        let mut st = lock.lock().unwrap();
                        st.second_read = true;
                        cv.notify_all();
                        let (st, _) = cv
                            .wait_timeout_while(st, deadline, |st| !st.first_done)
                            .unwrap();
                        assert!(st.first_done, "first forget did not finish in time");
                    }
                }
            });
            store2.settle(
                &Settle::Forget("preferences-2".parse().unwrap()),
                &cite(),
                NOW,
            )
        });

        let (forgotten1, changed1) = t1.join().unwrap().unwrap();
        assert!(changed1);
        assert_eq!(forgotten1.status, EntryStatus::Forgotten);
        let settled2 = t2.join().unwrap().unwrap();
        assert_eq!(
            settled2,
            Settled::Forgotten("preferences-2".parse().unwrap())
        );
    });

    for gone in [quote_1, context_1, reason_1, quote_2, context_2, reason_2] {
        assert!(!on_disk(&root, gone), "{gone:?} was restored on disk");
    }

    assert!(
        !*overlapped.lock().unwrap(),
        "second forget read the archived file while the first forget's read-modify-write was in flight"
    );

    let moved_proposals = root.join("pre-sqlite-2026-11-02/proposals.json");
    let moved_claims =
        format::decode_proposals(&moved_proposals, &fs::read(&moved_proposals).unwrap()).unwrap();
    assert_eq!(moved_claims, store.proposals().unwrap());
    assert!(on_disk(&root, kept_quote));

    let moved_audit = fs::read_to_string(root.join("pre-sqlite-2026-11-02/audit.jsonl")).unwrap();
    assert!(!moved_audit.contains(context_1));
    assert!(!moved_audit.contains(context_2));
    assert!(moved_audit.contains(kept_detail));
    assert!(moved_audit.contains(r#""target":"preferences-1""#));
    assert!(moved_audit.contains(r#""target":"preferences-2""#));
}

#[test]
fn concurrent_forgets_keep_both_erasures_in_archived_proposals() {
    run_interleaved_forgets(crate::store::documents::ForgetMovedStep::AfterProposalsRead);
}

#[test]
fn concurrent_forgets_keep_both_erasures_in_archived_audit() {
    run_interleaved_forgets(crate::store::documents::ForgetMovedStep::AfterAuditRead);
}

#[test]
fn forget_refuses_when_archive_forget_lock_is_held_past_lock_wait() {
    use crate::store::documents::ARCHIVE_FORGET_LOCK;
    use crate::store::{LOCKS_DIR, fsio};
    use std::time::Duration;

    let dir = private_tempdir();
    let root = dir.path().join("store");
    file_store(&root);
    let store = Store::open(&root).with_lock_wait(Duration::from_millis(60));
    store.migrate_store(NOW).unwrap();

    let lock_path = store.lock_path(ARCHIVE_FORGET_LOCK).unwrap();
    let _held =
        fsio::LockGuard::acquire(&lock_path, "other forget", Duration::from_secs(1)).unwrap();

    let instructed = Instructed {
        session: "s-9".parse().unwrap(),
        message: String::from("m-10"),
        at: NOW,
        quote: String::from("forget preferences-1"),
        prompt_sha256: "b".repeat(64),
        line: 1,
    };
    let err = store
        .forget_entry("preferences-1", &instructed, "opus".parse().unwrap(), NOW)
        .unwrap_err();
    let Error::Locked { path, holder, .. } = err else {
        panic!("expected Error::Locked, got {err:?}");
    };
    assert_eq!(path, root.join(LOCKS_DIR).join(ARCHIVE_FORGET_LOCK));
    assert!(holder.contains("other forget"), "{holder}");
}

#[test]
fn forget_without_moved_archive_does_not_create_archive_lock() {
    use crate::store::LOCKS_DIR;
    use crate::store::documents::ARCHIVE_FORGET_LOCK;

    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = Store::open(&root);
    let e = entry("preferences-1", 0, "fresh sqlite entry");
    store.write(|| store.write_entry(&e, true)).unwrap();

    let instructed = Instructed {
        session: "s-9".parse().unwrap(),
        message: String::from("m-10"),
        at: NOW,
        quote: String::from("forget preferences-1"),
        prompt_sha256: "b".repeat(64),
        line: 1,
    };
    let (forgotten, changed) = store
        .forget_entry("preferences-1", &instructed, "opus".parse().unwrap(), NOW)
        .unwrap();
    assert!(changed);
    assert_eq!(forgotten.status, EntryStatus::Forgotten);
    assert!(!root.join(LOCKS_DIR).join(ARCHIVE_FORGET_LOCK).exists());
}

#[test]
fn interrupted_forget_entry_finishes_archive_erasure_on_retry() {
    use crate::store::documents::ARCHIVE_FORGET_LOCK;
    use crate::store::fsio;
    use std::time::Duration;

    let dir = private_tempdir();
    let root = dir.path().join("store");

    let entry_quote = "Tuesday release train verbatim quote";
    let entry_context = "Tuesday release train context paragraph";
    let cand_reason = "candidate reason for preferences-1";
    let inst_quote = "approve p-1, Tuesday release train instruction";
    let supers_reason = "Thursday replaces Tuesday in release train";
    let kept_quote = "unrelated entry quote stays intact";

    let mut e1 = entry("preferences-1", 0, entry_context);
    e1.content_from = ContentFrom::Message {
        role: Speaker::User,
        message: String::from("m-9"),
        at: T0,
    };
    e1.said = String::from("scribe proposal settle p-1 --approve --author opus");
    e1.quote = Some(entry_quote.to_owned());
    e1.stated_by = Some(Author::session_pass());
    e1.approved_by = Some(Author::agent("opus".parse().unwrap()));
    e1.instruction = Some(Instruction {
        session: "s-9".parse().unwrap(),
        message: String::from("m-9"),
        at: NOW,
        quote: Some(inst_quote.to_owned()),
    });
    let e2 = entry("preferences-2", 1, kept_quote);
    for e in [&e1, &e2] {
        let rel = format!("entries/preferences/{}.md", e.id);
        put(
            &root,
            &rel,
            &format::encode_entry(&root.join(&rel), e).unwrap(),
        );
    }

    let p1 = candidate_proposal(
        "p-1",
        "preferences-1",
        ProposalState::Approved,
        entry_quote,
        entry_context,
        cand_reason,
        Some(inst_quote),
    );
    let mut p2 = supersedes();
    p2.id = "p-2".parse().unwrap();
    p2.reason = supers_reason.to_owned();
    let p3 = candidate_proposal(
        "p-3",
        "preferences-3",
        ProposalState::Open,
        kept_quote,
        "unrelated open context stays",
        "unrelated open reason stays",
        None,
    );
    put(
        &root,
        "proposals.json",
        &format::encode_proposals(&root.join("proposals.json"), &[p1, p2, p3]).unwrap(),
    );
    let audit = format!(
        "{AUDIT_FIXTURE}\n\
{{\"format\":5,\"at\":\"2026-10-20T12:00:00Z\",\"command\":\"hook stop\",\"status\":\"success\",\"target\":\"preferences-1\",\"detail\":\"prefix {entry_context} suffix\",\"duration_ms\":1}}\n\
{{\"format\":5,\"at\":\"2026-10-20T12:01:00Z\",\"command\":\"hook stop\",\"status\":\"success\",\"target\":\"p-1\",\"detail\":\"{entry_quote}\",\"duration_ms\":1}}\n\
{{\"format\":5,\"at\":\"2026-10-20T12:02:00Z\",\"detail\":\"torn line {cand_reason}\n"
    );
    put(&root, "audit.jsonl", audit.as_bytes());

    let store = Store::open(&root).with_lock_wait(Duration::from_millis(60));
    let moved = store.migrate_store(NOW).unwrap();
    assert_eq!(moved.moved_to.as_deref(), Some("pre-sqlite-2026-11-02"));

    // Interrupt post-commit archive cleanup by holding archive-forget.lock.
    let lock_path = store.lock_path(ARCHIVE_FORGET_LOCK).unwrap();
    let held =
        fsio::LockGuard::acquire(&lock_path, "other forget", Duration::from_secs(1)).unwrap();
    let instructed = Instructed {
        session: "s-9".parse().unwrap(),
        message: String::from("m-10"),
        at: NOW,
        quote: String::from("forget preferences-1 for good"),
        prompt_sha256: "b".repeat(64),
        line: 1,
    };
    let err = store
        .forget_entry("preferences-1", &instructed, "opus".parse().unwrap(), NOW)
        .unwrap_err();
    assert!(matches!(err, Error::Locked { .. }), "{err:?}");
    drop(held);

    // SQLite forget already committed, while the archive was left untouched.
    assert_eq!(
        store
            .entry(&"preferences-1".parse().unwrap())
            .unwrap()
            .status,
        EntryStatus::Forgotten
    );
    assert!(
        root.join("pre-sqlite-2026-11-02/entries/preferences/preferences-1.md")
            .is_file()
    );
    // Neither scribe.db nor its WAL holds the erased words in cleartext while
    // the archive-erasure obligation is pending.
    for gone in [
        entry_quote,
        entry_context,
        cand_reason,
        inst_quote,
        supers_reason,
        "the release train leaves on Tuesdays",
    ] {
        for db_file in [db::DB_FILE, "scribe.db-wal"] {
            let bytes = fs::read(root.join(db_file)).unwrap();
            assert!(
                !bytes.windows(gone.len()).any(|w| w == gone.as_bytes()),
                "{db_file} held cleartext {gone:?} while archive forget was pending"
            );
        }
    }
    let pending_count = |root: &Path| -> i64 {
        let conn = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
        conn.query_row("SELECT count(*) FROM pending_archive_forgets", [], |row| {
            row.get(0)
        })
        .unwrap()
    };
    assert_eq!(pending_count(&root), 1);

    // Reopen and retry the forget: archive erasure must complete.
    let reopened = Store::open(&root);
    let (forgotten, changed) = reopened
        .forget_entry("preferences-1", &instructed, "opus".parse().unwrap(), NOW)
        .unwrap();
    assert!(!changed);
    assert_eq!(forgotten.status, EntryStatus::Forgotten);
    assert_eq!(pending_count(&root), 0);

    assert!(
        !root
            .join("pre-sqlite-2026-11-02/entries/preferences/preferences-1.md")
            .exists()
    );
    for gone in [
        entry_quote,
        entry_context,
        cand_reason,
        inst_quote,
        supers_reason,
        "the release train leaves on Tuesdays",
    ] {
        assert!(
            !on_disk(&root, gone),
            "{gone:?} remained on disk after retry"
        );
    }
    let moved_proposals = root.join("pre-sqlite-2026-11-02/proposals.json");
    let moved_claims =
        format::decode_proposals(&moved_proposals, &fs::read(&moved_proposals).unwrap()).unwrap();
    assert_eq!(moved_claims, reopened.proposals().unwrap());
    let moved_audit = fs::read_to_string(root.join("pre-sqlite-2026-11-02/audit.jsonl")).unwrap();
    assert!(moved_audit.contains(r#""target":"preferences-1""#));
    assert!(moved_audit.contains(r#"{"format":5,"at":"2026-11-21T09:"#));
    assert!(on_disk(&root, kept_quote));
}

#[test]
fn interrupted_settle_forget_finishes_archive_erasure_on_retry() {
    let dir = private_tempdir();
    let root = dir.path().join("store");

    let open_quote = "pre-move open candidate quote";
    let open_context = "pre-move open candidate context";
    let kept_quote = "other rejected claim quote";

    let open = candidate_proposal(
        "p-1",
        "preferences-1",
        ProposalState::Open,
        open_quote,
        open_context,
        "open candidate reason",
        None,
    );
    let kept = candidate_proposal(
        "p-2",
        "preferences-2",
        ProposalState::Rejected,
        kept_quote,
        "other rejected context",
        "other rejected reason",
        None,
    );
    put(
        &root,
        "proposals.json",
        &format::encode_proposals(&root.join("proposals.json"), &[open, kept]).unwrap(),
    );
    let audit = format!(
        "{AUDIT_FIXTURE}\n\
{{\"format\":5,\"at\":\"2026-10-20T12:00:00Z\",\"command\":\"hook stop\",\"status\":\"success\",\"target\":\"preferences-1\",\"detail\":\"{open_quote}\",\"duration_ms\":1}}\n"
    );
    put(&root, "audit.jsonl", audit.as_bytes());

    let store = Store::open(&root);
    store.migrate_store(NOW).unwrap();
    let settled = store
        .settle(&Settle::Approve("p-1".parse().unwrap()), &cite(), NOW)
        .unwrap();
    assert!(matches!(settled, Settled::Recorded { .. }), "{settled:?}");

    crate::store::write::CRASH_BEFORE_ARCHIVE_FORGET.with(|c| c.set(true));
    let err = store
        .settle(
            &Settle::Forget("preferences-1".parse().unwrap()),
            &cite(),
            NOW,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err:?}");
    // Archive still holds the pre-move candidate and audit text, while scribe.db
    // has neither in cleartext.
    assert!(on_disk(&root, open_quote));
    for db_file in [db::DB_FILE, "scribe.db-wal"] {
        let bytes = fs::read(root.join(db_file)).unwrap();
        assert!(
            !bytes
                .windows(open_quote.len())
                .any(|w| w == open_quote.as_bytes())
        );
    }

    // Reopen and retry `#scribe forget`: returns Already and completes archive erasure.
    let reopened = Store::open(&root);
    let retry = reopened
        .settle(
            &Settle::Forget("preferences-1".parse().unwrap()),
            &cite(),
            NOW,
        )
        .unwrap();
    assert!(matches!(retry, Settled::Already(_)), "{retry:?}");
    assert!(!on_disk(&root, open_quote));
    assert!(!on_disk(&root, open_context));
    assert!(on_disk(&root, kept_quote));
    let moved_audit = fs::read_to_string(root.join("pre-sqlite-2026-11-02/audit.jsonl")).unwrap();
    assert!(!moved_audit.contains(open_quote));
    assert!(moved_audit.contains(r#""target":"preferences-1""#));
}

#[test]
fn interrupted_forget_finishes_on_unrelated_write_without_original_entry_cleartext() {
    let dir = private_tempdir();
    let root = dir.path().join("store");

    let entry_body = "café résumé — naïve Tuesday release train";
    let entry_quote = "verbatim café quote to erase";
    let kept_body = "Thursday release train stays";

    let mut e1 = entry("preferences-1", 0, entry_body);
    e1.content_from = ContentFrom::Message {
        role: Speaker::User,
        message: String::from("m-9"),
        at: T0,
    };
    e1.said = String::from("scribe proposal settle p-1 --approve --author opus");
    e1.quote = Some(entry_quote.to_owned());
    e1.stated_by = Some(Author::session_pass());
    e1.approved_by = Some(Author::agent("opus".parse().unwrap()));
    e1.instruction = Some(Instruction {
        session: "s-9".parse().unwrap(),
        message: String::from("m-9"),
        at: NOW,
        quote: Some(format!("approve p-1: {entry_quote}")),
    });
    let e2 = entry("preferences-2", 1, kept_body);
    for e in [&e1, &e2] {
        let rel = format!("entries/preferences/{}.md", e.id);
        put(
            &root,
            &rel,
            &format::encode_entry(&root.join(&rel), e).unwrap(),
        );
    }
    let p1 = candidate_proposal(
        "p-1",
        "preferences-1",
        ProposalState::Approved,
        entry_quote,
        entry_body,
        "candidate reason for café entry",
        None,
    );
    put(
        &root,
        "proposals.json",
        &format::encode_proposals(&root.join("proposals.json"), &[p1]).unwrap(),
    );
    let audit = format!(
        "{AUDIT_FIXTURE}\n\
{{\"format\":5,\"at\":\"2026-10-20T12:00:00Z\",\"command\":\"hook stop\",\"status\":\"success\",\"target\":\"preferences-1\",\"detail\":\"résumé prefix: {entry_body} — suffix\",\"duration_ms\":1}}\n\
{{\"format\":5,\"at\":\"2026-10-20T12:01:00Z\",\"command\":\"hook stop\",\"status\":\"success\",\"target\":\"preferences-2\",\"detail\":\"{kept_body}\",\"duration_ms\":1}}\n\
{{\"format\":5,\"detail\":\"torn {entry_quote}\n"
    );
    put(&root, "audit.jsonl", audit.as_bytes());

    let store = Store::open(&root);
    store.migrate_store(NOW).unwrap();

    let instructed = Instructed {
        session: "s-9".parse().unwrap(),
        message: String::from("m-10"),
        at: NOW,
        quote: String::from("forget preferences-1 for good"),
        prompt_sha256: "b".repeat(64),
        line: 1,
    };
    crate::store::write::CRASH_BEFORE_ARCHIVE_FORGET.with(|c| c.set(true));
    let err = store
        .forget_entry("preferences-1", &instructed, "opus".parse().unwrap(), NOW)
        .unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err:?}");

    // Remove the moved entry file before recovery to prove replay finishes
    // cited-claim and audit erasure without the original entry file in cleartext.
    fs::remove_file(root.join("pre-sqlite-2026-11-02/entries/preferences/preferences-1.md"))
        .unwrap();
    for gone in [entry_body, entry_quote] {
        for db_file in [db::DB_FILE, "scribe.db-wal"] {
            let bytes = fs::read(root.join(db_file)).unwrap();
            assert!(!bytes.windows(gone.len()).any(|w| w == gone.as_bytes()));
        }
    }

    // Any restarted write replays and completes the archive erasure.
    let reopened = Store::open(&root);
    reopened
        .update_control(|s| control::switch(s, &"s-7".parse().unwrap(), &Switch::On, NOW))
        .unwrap();

    for gone in [entry_body, entry_quote, "candidate reason for café entry"] {
        assert!(!on_disk(&root, gone), "{gone:?} remained on disk");
    }
    let moved_proposals = root.join("pre-sqlite-2026-11-02/proposals.json");
    let moved_claims =
        format::decode_proposals(&moved_proposals, &fs::read(&moved_proposals).unwrap()).unwrap();
    assert_eq!(moved_claims, reopened.proposals().unwrap());
    let moved_audit = fs::read_to_string(root.join("pre-sqlite-2026-11-02/audit.jsonl")).unwrap();
    assert!(moved_audit.contains(kept_body));
    assert!(moved_audit.contains(r#""target":"preferences-1""#));
    assert!(!moved_audit.contains(entry_body));
    assert!(!moved_audit.contains(entry_quote));

    let conn = rusqlite::Connection::open(root.join(db::DB_FILE)).unwrap();
    let remaining: i64 = conn
        .query_row("SELECT count(*) FROM pending_archive_forgets", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn interrupted_first_write_forget_before_move_aside_finishes_archive_erasure() {
    let dir = private_tempdir();
    let root = dir.path().join("store");

    let secret_body = "first write forget secret body";
    let kept_body = "unrelated second entry stays";
    let e1 = entry("preferences-1", 0, secret_body);
    let e2 = entry("preferences-2", 1, kept_body);
    for e in [&e1, &e2] {
        let rel = format!("entries/preferences/{}.md", e.id);
        put(
            &root,
            &rel,
            &format::encode_entry(&root.join(&rel), e).unwrap(),
        );
    }
    let audit = format!(
        "{AUDIT_FIXTURE}\n\
{{\"format\":5,\"at\":\"2026-10-20T12:00:00Z\",\"command\":\"hook stop\",\"status\":\"success\",\"target\":\"preferences-1\",\"detail\":\"{secret_body}\",\"duration_ms\":1}}\n"
    );
    put(&root, "audit.jsonl", audit.as_bytes());

    let store = Store::open(&root);
    let instructed = Instructed {
        session: "s-9".parse().unwrap(),
        message: String::from("m-10"),
        at: NOW,
        quote: String::from("forget preferences-1 for good"),
        prompt_sha256: "b".repeat(64),
        line: 1,
    };

    // Crash after committing both the automatic migration and the forget,
    // before `move_files_aside` creates `pre-sqlite-*`.
    crate::store::write::CRASH_BEFORE_ASIDE.with(|c| c.set(true));
    let err = store
        .forget_entry("preferences-1", &instructed, "opus".parse().unwrap(), NOW)
        .unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err:?}");
    assert!(root.join("entries/preferences/preferences-1.md").is_file());

    // A restarted write moves the files aside and immediately finishes erasing
    // preferences-1 from the newly created `pre-sqlite-*` archive.
    let reopened = Store::open(&root);
    reopened
        .update_control(|s| control::switch(s, &"s-8".parse().unwrap(), &Switch::On, NOW))
        .unwrap();

    assert!(!root.join("entries").exists());
    assert!(!on_disk(&root, secret_body));
    assert!(on_disk(&root, kept_body));
    let moved_dir = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().starts_with("pre-sqlite-"))
        .expect("pre-sqlite-* created")
        .path();
    assert!(
        !moved_dir
            .join("entries/preferences/preferences-1.md")
            .exists()
    );
    assert!(
        moved_dir
            .join("entries/preferences/preferences-2.md")
            .is_file()
    );
    let moved_audit = fs::read_to_string(moved_dir.join("audit.jsonl")).unwrap();
    assert!(!moved_audit.contains(secret_body));
}

#[test]
fn corrupt_pending_archive_forget_row_fails_closed_without_dropping() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    file_store(&root);
    let store = Store::open(&root);
    store.migrate_store(NOW).unwrap();

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
}

#[test]
fn jsonl_export_includes_pending_spooled_audit_once_and_skips_drained_spool() {
    use crate::store::audit::IssueAudit;
    use std::time::Duration;

    let dir = private_tempdir();
    let root = dir.path().join("store");
    file_store(&root);
    let store = Store::open(&root);
    store.migrate_store(NOW).unwrap();

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

    // An unmoved store with only a spooled audit file still requires migration.
    let unmoved_dir = private_tempdir();
    let unmoved_root = unmoved_dir.path().join("store");
    let unmoved = Store::open(&unmoved_root);
    unmoved.record_audit(&second).unwrap();
    let err = unmoved
        .export(&unmoved_dir.path().join("jsonl"), ExportFormat::Jsonl)
        .unwrap_err();
    assert_eq!(err.code(), "export_needs_migrate");
}

#[test]
fn put_entry_refuses_unreadable_spoken_entry_without_writing() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = Store::open(&root);
    store.migrate_store(NOW).unwrap();
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
