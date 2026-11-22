//! Entries, claims, tags, marks, pickups and the audit log in `scribe.db`
//! (2_design.md §15.7, §15.11, phase 7): the move, `export`, and a write
//! and its audit line as one transaction.

use super::{SESSION_BODY, on_disk, private_tempdir, session_text};
use crate::annotation::{MarkRecord, MarkState};
use crate::consistency::{Draft, ReapplyReport, Settle, Settled};
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
fn jsonl_export_includes_pending_spooled_audit_once_and_skips_drained_spool() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    let store = Store::open(&root);
    store.migrate_store(NOW).unwrap();

    // Hold a SQLite write lock so record_audit spools instead of inserting.
    let holder = db::open_new_writer(&root, std::time::Duration::from_secs(1)).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let pending = AuditEvent {
        at: NOW,
        command: String::from("tag add"),
        status: AuditStatus::Failure,
        target: Some(String::from("notes")),
        error_code: Some(String::from("reserved_author")),
        author: Some(String::from("user")),
        instruction_session: None,
        instruction_message: None,
        issue: Some(audit::IssueAudit {
            verb: String::from("add"),
            id: Some(String::from("issue-1")),
            occurrence: Some(1),
            session: Some(String::from("s-1")),
            request_message: None,
            moved_to: None,
            trigger: None,
            request_sha256: Vec::new(),
        }),
        duration_ms: 7,
    };
    store.record_audit(&pending).unwrap();

    let spool_dir = root.join("audit-spool");
    let spool_files: Vec<_> = fs::read_dir(&spool_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(spool_files.len(), 1, "{spool_files:?}");
    let spool_name = &spool_files[0];
    let spool_bytes = fs::read(spool_dir.join(spool_name)).unwrap();

    // Moved store with 0 SQL audit rows and 1 pending spooled event.
    let jsonl_pending = dir.path().join("jsonl-pending");
    let exported = store.export(&jsonl_pending, ExportFormat::Jsonl).unwrap();
    assert_eq!(exported.audit_lines, 1, "{exported:?}");
    let rows: Vec<serde_json::Value> = fs::read_to_string(jsonl_pending.join("audit.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0]["id"].is_null(), "{rows:?}");
    assert_eq!(rows[0]["format"], 5);
    assert_eq!(rows[0]["command"], "tag add");
    assert_eq!(rows[0]["status"], "failure");
    assert_eq!(rows[0]["target"], "notes");
    assert_eq!(rows[0]["error_code"], "reserved_author");
    assert_eq!(rows[0]["author"], "user");
    assert_eq!(rows[0]["duration_ms"], 7);
    assert_eq!(rows[0]["spool"], spool_name.as_str());
    let issue_json: serde_json::Value =
        serde_json::from_str(rows[0]["issue"].as_str().unwrap()).unwrap();
    assert_eq!(issue_json["verb"], "add");
    assert_eq!(issue_json["id"], "issue-1");

    // Release the writer and let a store write drain the spool into SQLite.
    holder.execute_batch("ROLLBACK").unwrap();
    drop(holder);
    store
        .update_control(|state| {
            crate::control::switch(
                state,
                &"s-1".parse().unwrap(),
                &crate::control::Switch::On,
                NOW,
            )
        })
        .unwrap();
    assert_eq!(fs::read_dir(&spool_dir).unwrap().count(), 0);

    // Simulate a crash after commit before the drained spool file was removed.
    put(&root, &format!("audit-spool/{spool_name}"), &spool_bytes);
    let jsonl_drained = dir.path().join("jsonl-drained");
    let exported = store.export(&jsonl_drained, ExportFormat::Jsonl).unwrap();
    assert_eq!(exported.audit_lines, 1, "{exported:?}");
    let rows: Vec<serde_json::Value> = fs::read_to_string(jsonl_drained.join("audit.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0]["id"].is_i64(), "{rows:?}");
    assert_eq!(rows[0]["spool"], spool_name.as_str());
    assert_eq!(rows[0]["command"], "tag add");
}
