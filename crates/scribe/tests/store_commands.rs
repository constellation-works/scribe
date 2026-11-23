//! `scribe store migrate` and `scribe export` from the built binary.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::Value;
use support::{Fixture, make_private, text};

const ON: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

fn json(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn migrate_refuses_while_a_session_is_on_then_moves_every_session() {
    let fx = Fixture::new();
    fx.seed();
    let out = fx.run(&["store", "migrate", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "migrate_blocked", "{err}");
    assert!(
        fx.root()
            .join(format!("sessions/2026/11/{ON}.md"))
            .is_file()
    );

    // Switched off by hand, as `/scribe off` would leave it.
    let control = fx.root().join("control.json");
    let text_now = std::fs::read_to_string(&control).unwrap();
    std::fs::write(
        &control,
        text_now.replace(
            r#"{"on": true, "since": "2026-11-02T18:55:00Z", "due": true"#,
            r#"{"on": false, "since": "2026-11-02T18:55:00Z", "due": false"#,
        ),
    )
    .unwrap();
    make_private(&control);
    let before = json(&fx.ok(&["session", "list", "--include-inactive", "--json"]));
    let moved = json(&fx.ok(&["store", "migrate", "--json"]));
    assert_eq!(moved["sessions"], 3, "{moved}");
    assert_eq!(moved["switches"], 2, "{moved}");
    assert_eq!(moved["entries"], 4, "{moved}");
    assert_eq!(moved["claims"], 2, "{moved}");
    assert_eq!(moved["tags"], 0, "{moved}");
    assert_eq!(moved["marks"], 0, "{moved}");
    assert_eq!(moved["pickups"], 0, "{moved}");
    // The refused migrate's line, spooled while the store was files.
    assert_eq!(moved["audit_lines"], 1, "{moved}");
    assert_eq!(moved["audit_torn"], 0, "{moved}");
    let aside = moved["moved_to"].as_str().unwrap();
    assert!(aside.starts_with("pre-sqlite-"), "{aside}");
    assert!(fx.root().join(aside).join("control.json").is_file());
    assert!(!fx.root().join("sessions").exists());
    assert!(!fx.root().join("entries").exists());
    assert!(fx.root().join(aside).join("entries").is_dir());
    assert!(fx.root().join(aside).join("proposals.json").is_file());
    assert_eq!(
        json(&fx.ok(&["session", "list", "--include-inactive", "--json"])),
        before
    );
    let again = fx.run(&["store", "migrate", "--json"]);
    assert_eq!(again.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&again.stderr).unwrap();
    assert_eq!(err["code"], "migrate_blocked", "{err}");
}

#[test]
fn migrate_and_its_success_audit_commit_or_roll_back_together() {
    let fx = Fixture::new();
    fx.seed();
    // Create scribe.db at the current schema without migrating records yet.
    let refused = fx.run(&["store", "migrate", "--json"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused.stderr));

    let control = fx.root().join("control.json");
    let text_now = std::fs::read_to_string(&control).unwrap();
    std::fs::write(
        &control,
        text_now.replace(
            r#"{"on": true, "since": "2026-11-02T18:55:00Z", "due": true"#,
            r#"{"on": false, "since": "2026-11-02T18:55:00Z", "due": false"#,
        ),
    )
    .unwrap();
    make_private(&control);

    {
        let conn = rusqlite::Connection::open(fx.root().join("scribe.db")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER block_migrate_success_audit
             BEFORE INSERT ON audit
             WHEN NEW.command = 'store migrate' AND NEW.status = 'success'
             BEGIN
                 SELECT RAISE(ABORT, 'injected audit failure');
             END;",
        )
        .unwrap();
    }

    let aborted = fx.run(&["store", "migrate", "--json"]);
    assert_eq!(
        aborted.status.code(),
        Some(1),
        "audit failure must roll back migration: {}",
        text(&aborted.stdout)
    );
    assert!(fx.root().join("sessions").is_dir());
    assert!(fx.root().join("entries").is_dir());
    assert!(fx.root().join("proposals.json").is_file());
    let has_aside = std::fs::read_dir(fx.root())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().starts_with("pre-sqlite-"));
    assert!(!has_aside, "files must stay when migration rolls back");

    {
        let conn = rusqlite::Connection::open(fx.root().join("scribe.db")).unwrap();
        let kinds: i64 = conn
            .query_row("SELECT count(*) FROM record_kinds", [], |r| r.get(0))
            .unwrap();
        let sessions: i64 = conn
            .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        let entries: i64 = conn
            .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))
            .unwrap();
        let audits: i64 = conn
            .query_row(
                "SELECT count(*) FROM audit WHERE command = 'store migrate' AND status = 'success'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kinds, 0, "record_kinds must roll back");
        assert_eq!(sessions, 0, "sessions must roll back");
        assert_eq!(entries, 0, "entries must roll back");
        assert_eq!(audits, 0, "success audit must not exist");
        conn.execute_batch("DROP TRIGGER block_migrate_success_audit;")
            .unwrap();
    }

    let moved = json(&fx.ok(&["store", "migrate", "--json"]));
    assert_eq!(moved["sessions"], 3, "{moved}");
    let aside = moved["moved_to"].as_str().unwrap();
    assert!(aside.starts_with("pre-sqlite-"), "{aside}");
    assert!(!fx.root().join("sessions").exists());
    assert!(!fx.root().join("entries").exists());

    let conn = rusqlite::Connection::open(fx.root().join("scribe.db")).unwrap();
    let success_audits: i64 = conn
        .query_row(
            "SELECT count(*) FROM audit WHERE command = 'store migrate' AND status = 'success' AND target = 'store:records'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(success_audits, 1);
}

#[test]
fn export_writes_every_session_and_writes_nothing_to_the_store() {
    let fx = Fixture::new();
    fx.seed();
    let before = support::snapshot(&fx.root());
    let target = fx.elsewhere("export");
    let out = json(&fx.ok(&["export", target.to_str().unwrap(), "--json"]));
    assert_eq!(out["sessions"], 3, "{out}");
    assert_eq!(
        support::snapshot(&fx.root()),
        before,
        "export writes nothing"
    );
    for (month, id) in [
        ("2026/10", "0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01"),
        ("2026/10", "3d5e7f90-1a2b-4c3d-8e9f-0a1b2c3d4e05"),
        ("2026/11", ON),
    ] {
        let exported =
            std::fs::read_to_string(target.join(format!("sessions/{month}/{id}.md"))).unwrap();
        assert!(exported.starts_with("---\nformat: 29\n"), "{exported}");
        assert!(exported.contains(&format!("\nid: {id}\n")), "{exported}");
    }
    for rel in [
        "entries/decisions/decisions-1.md",
        "entries/preferences/preferences-1.md",
        "proposals.json",
    ] {
        assert!(target.join(rel).is_file(), "{rel} exported");
    }
    assert!(out["entries"] == 4 && out["claims"] == 2, "{out}");
    // JSONL needs the sessions in scribe.db.
    let jsonl = fx.elsewhere("jsonl");
    let out = fx.run(&["export", "--as", "jsonl", jsonl.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "export_needs_migrate", "{err}");
    // A non-empty target is refused.
    let out = fx.run(&["export", target.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "export_target_exists", "{err}");
}

#[test]
fn jsonl_export_reads_phase_six_database_with_no_live_files() {
    let fx = Fixture::new();
    fx.seed();
    std::fs::remove_dir_all(fx.root().join("entries")).unwrap();
    std::fs::remove_file(fx.root().join("proposals.json")).unwrap();
    let control = fx.root().join("control.json");
    let text_now = std::fs::read_to_string(&control).unwrap();
    std::fs::write(
        &control,
        text_now.replace(
            r#"{"on": true, "since": "2026-11-02T18:55:00Z", "due": true"#,
            r#"{"on": false, "since": "2026-11-02T18:55:00Z", "due": false"#,
        ),
    )
    .unwrap();
    make_private(&control);

    fx.ok(&["store", "migrate", "--json"]);
    {
        let conn = rusqlite::Connection::open(fx.root().join("scribe.db")).unwrap();
        let mut persist = 1;
        conn.file_control(
            rusqlite::DEFAULT_NAME,
            rusqlite::FileControl::PersistWal(&mut persist),
        )
        .unwrap();
        support::remove_group_capture_schema(&fx.root());
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
    }
    for stamp in ["proposals.json", "tags.json", "marks.json", "pickups.json"] {
        let _ = std::fs::remove_file(fx.root().join(stamp));
    }
    std::fs::write(
        &control,
        r#"{"format": 26, "sessions": {}, "codex_cursors": {}}"#,
    )
    .unwrap();
    make_private(&control);

    let before = support::snapshot(&fx.root());
    let jsonl = fx.elsewhere("jsonl");
    let out = json(&fx.ok(&["export", "--as", "jsonl", jsonl.to_str().unwrap(), "--json"]));
    assert_eq!(out["sessions"], 3, "{out}");
    assert_eq!(out["entries"], 0, "{out}");
    assert_eq!(out["claims"], 0, "{out}");
    assert_eq!(out["audit_lines"], 0, "{out}");
    assert_eq!(
        support::snapshot(&fx.root()),
        before,
        "export writes nothing"
    );
    let sessions = std::fs::read_to_string(jsonl.join("sessions.jsonl")).unwrap();
    assert_eq!(sessions.lines().count(), 3, "{sessions}");

    // A live document file beside the phase-six DB still requires migration.
    let audit = fx.root().join("audit.jsonl");
    std::fs::write(
        &audit,
        "{\"format\":5,\"at\":\"2026-11-21T09:00:00Z\",\"command\":\"entry add\",\"status\":\"success\",\"target\":null,\"error_code\":null,\"duration_ms\":1}\n",
    )
    .unwrap();
    make_private(&audit);
    let blocked = fx.elsewhere("blocked");
    let out = fx.run(&[
        "export",
        "--as",
        "jsonl",
        blocked.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "export_needs_migrate", "{err}");
    assert!(!blocked.exists());
}

#[test]
fn jsonl_export_includes_pending_and_already_drained_spooled_audit_once() {
    let fx = Fixture::new();
    fx.seed();
    let control = fx.root().join("control.json");
    let text_now = std::fs::read_to_string(&control).unwrap();
    std::fs::write(
        &control,
        text_now.replace(
            r#"{"on": true, "since": "2026-11-02T18:55:00Z", "due": true"#,
            r#"{"on": false, "since": "2026-11-02T18:55:00Z", "due": false"#,
        ),
    )
    .unwrap();
    make_private(&control);

    let moved = json(&fx.ok(&["store", "migrate", "--json"]));
    assert_eq!(moved["audit_lines"], 0, "{moved}");

    // Hold a write lock so a failing stepwise command spools its audit event.
    let holder = rusqlite::Connection::open(fx.root().join("scribe.db")).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let failed = fx.run(&[
        "session",
        "pass",
        "--session",
        "00000000-0000-0000-0000-000000000099",
        "--json",
    ]);
    assert_eq!(failed.status.code(), Some(1), "{}", text(&failed.stderr));
    holder.execute_batch("ROLLBACK").unwrap();
    drop(holder);

    let spool_dir = fx.root().join("audit-spool");
    let spool_files: Vec<_> = std::fs::read_dir(&spool_dir)
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    assert_eq!(spool_files.len(), 1);
    let spool_name = spool_files[0].file_name().into_string().unwrap();
    let spool_bytes = std::fs::read(spool_files[0].path()).unwrap();

    // JSONL export includes the pending spooled event (alongside `store migrate`).
    let jsonl_pending = fx.elsewhere("jsonl-pending");
    let out = json(&fx.ok(&[
        "export",
        "--as",
        "jsonl",
        jsonl_pending.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(out["audit_lines"], 2, "{out}");
    let rows: Vec<Value> = std::fs::read_to_string(jsonl_pending.join("audit.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["command"], "store migrate");
    assert_eq!(rows[1]["id"], Value::Null);
    assert_eq!(rows[1]["command"], "session pass");
    assert_eq!(rows[1]["status"], "failure");
    assert_eq!(rows[1]["error_code"], "session_not_on");
    assert_eq!(rows[1]["spool"], spool_name);

    // Trigger a write to drain the spool into scribe.db, then restore the
    // already-drained spool file on disk.
    let drain = fx
        .cmd(&[
            "tag",
            "add",
            "notes",
            "--author",
            "opus",
            "--description",
            "Agent notes after spool drain",
            "--json",
        ])
        .env("GITHUB_TOKEN", "test-github-token-1234")
        .output()
        .unwrap();
    assert_eq!(drain.status.code(), Some(0), "{}", text(&drain.stderr));
    let restored = spool_dir.join(&spool_name);
    assert!(!restored.exists());
    std::fs::write(&restored, &spool_bytes).unwrap();
    make_private(&restored);

    let jsonl_drained = fx.elsewhere("jsonl-drained");
    let out = json(&fx.ok(&[
        "export",
        "--as",
        "jsonl",
        jsonl_drained.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(out["audit_lines"], 3, "{out}");
    let rows: Vec<Value> = std::fs::read_to_string(jsonl_drained.join("audit.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 3, "{rows:?}");
    let matching: Vec<&Value> = rows
        .iter()
        .filter(|r| r["spool"].as_str() == Some(spool_name.as_str()))
        .collect();
    assert_eq!(matching.len(), 1, "{rows:?}");
    assert!(matching[0]["id"].is_i64(), "{:?}", matching[0]);
}
