//! `scribe store migrate` and `scribe export` from the built binary.
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/store_commands.rs"]
mod subject;
mod support;

use crate::support::command::Exit::Code;
use crate::support::command::Input::Cli;
use crate::support::command::with_env;
use serde_json::Value;
use subject::*;
use support::command::Exit::{Capture, Success};
use support::{Fixture, text};

#[test]
fn migrate_refuses_while_a_session_is_on_then_imports_the_archive() {
    let fx = Fixture::seeded();
    leave_archive(&fx);
    let out = fx.check(&["migrate", "--json"], Code(1));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "migrate_blocked", "{err}");
    assert!(
        fx.root()
            .join(ARCHIVE)
            .join(format!("sessions/2026/11/{ON}.md"))
            .is_file(),
        "a refused migrate leaves the archive"
    );
    assert_eq!(archive_rows(&fx), 0);

    support::set_switch(&fx.root(), ON, "on", "0");
    let before = json(&fx.check(
        Cli(&["session", "list", "--include-inactive", "--json"]),
        Success,
    ));
    let moved = json(&fx.check(&["migrate", "--json"], Success));
    assert_eq!(moved["archives"], 1, "{moved}");
    // 3 sessions, 4 entries, control.json, proposals, tags, marks, pickups.
    assert_eq!(moved["archived"], 12, "{moved}");
    // Nothing moves from files any more.
    for count in ["sessions", "switches", "entries", "claims", "tags", "marks"] {
        assert_eq!(moved[count], 0, "{count}: {moved}");
    }
    assert_eq!(moved["moved_to"], Value::Null, "{moved}");
    assert!(!fx.root().join(ARCHIVE).exists(), "the files are removed");
    assert_eq!(archive_rows(&fx), 12);
    assert_eq!(
        json(&fx.check(
            Cli(&["session", "list", "--include-inactive", "--json"]),
            Success
        )),
        before
    );
    let again = fx.check(&["migrate", "--json"], Code(1));
    let err: Value = serde_json::from_slice(&again.stderr).unwrap();
    assert_eq!(err["code"], "migrate_blocked", "{err}");
}

#[test]
fn migrate_and_its_success_audit_commit_or_roll_back_together() {
    let fx = Fixture::seeded();
    leave_archive(&fx);
    switch_off(&fx);
    support::sql(
        &fx.root(),
        "CREATE TRIGGER block_migrate_success_audit
         BEFORE INSERT ON audit
         WHEN NEW.command = 'store migrate' AND NEW.status = 'success'
         BEGIN
             SELECT RAISE(ABORT, 'injected audit failure');
         END;",
    );

    let aborted = fx.check(&["migrate", "--json"], Capture);
    assert_eq!(
        aborted.status.code(),
        Some(1),
        "audit failure must roll back the import: {}",
        text(&aborted.stdout)
    );
    assert!(
        fx.root().join(ARCHIVE).join("entries").is_dir(),
        "files must stay when the import rolls back"
    );
    assert!(fx.root().join(ARCHIVE).join("proposals.json").is_file());
    assert_eq!(archive_rows(&fx), 0, "archive_records must roll back");
    let audits: i64 = rusqlite::Connection::open(fx.root().join("scribe.db"))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM audit WHERE command = 'store migrate' AND status = 'success'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(audits, 0, "success audit must not exist");

    support::sql(&fx.root(), "DROP TRIGGER block_migrate_success_audit;");
    let moved = json(&fx.check(&["migrate", "--json"], Success));
    assert_eq!(moved["archives"], 1, "{moved}");
    assert!(!fx.root().join(ARCHIVE).exists());
    assert_eq!(archive_rows(&fx), 12);

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
    let fx = Fixture::seeded();
    fx.check(Cli(&["session", "list", "--json"]), Success); // the first open creates the WAL
    let before = support::snapshot(&fx.root());
    let target = fx.elsewhere("export");
    let out = json(&fx.check(
        Cli(&["export", target.to_str().unwrap(), "--json"]),
        Success,
    ));
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
        assert!(exported.starts_with("---\nformat: 31\n"), "{exported}");
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
    // JSONL reads the same store.
    let jsonl = fx.elsewhere("jsonl");
    let out = json(&fx.check(
        Cli(&["export", "--as", "jsonl", jsonl.to_str().unwrap(), "--json"]),
        Success,
    ));
    assert_eq!(out["sessions"], 3, "{out}");
    assert!(out["entries"] == 4 && out["claims"] == 2, "{out}");
    let sessions = std::fs::read_to_string(jsonl.join("sessions.jsonl")).unwrap();
    assert_eq!(sessions.lines().count(), 3, "{sessions}");
    assert_eq!(
        support::snapshot(&fx.root()),
        before,
        "export writes nothing"
    );
    // A non-empty target is refused.
    let out = fx.check(
        Cli(&["export", target.to_str().unwrap(), "--json"]),
        Code(1),
    );
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "export_target_exists", "{err}");
}

#[test]
fn jsonl_export_includes_pending_and_already_drained_spooled_audit_once() {
    let fx = Fixture::seeded();
    leave_archive(&fx);
    switch_off(&fx);

    let moved = json(&fx.check(&["migrate", "--json"], Success));
    assert_eq!(moved["archives"], 1, "{moved}");
    assert_eq!(moved["audit_lines"], 0, "{moved}");

    // Hold a write lock so a failing stepwise command spools its audit event.
    let holder = rusqlite::Connection::open(fx.root().join("scribe.db")).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    fx.check(
        Cli(&[
            "session",
            "pass",
            "--session",
            "00000000-0000-0000-0000-000000000099",
            "--json",
        ]),
        Code(1),
    );
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
    let out = json(&fx.check(
        Cli(&[
            "export",
            "--as",
            "jsonl",
            jsonl_pending.to_str().unwrap(),
            "--json",
        ]),
        Success,
    ));
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
    fx.check(
        with_env(
            &[
                "tag",
                "add",
                "notes",
                "--author",
                "opus",
                "--description",
                "Agent notes after spool drain",
                "--json",
            ],
            "GITHUB_TOKEN",
            "test-github-token-1234",
        ),
        Success,
    );
    let restored = spool_dir.join(&spool_name);
    assert!(!restored.exists());
    std::fs::write(&restored, &spool_bytes).unwrap();
    support::make_private(&restored);

    let jsonl_drained = fx.elsewhere("jsonl-drained");
    let out = json(&fx.check(
        Cli(&[
            "export",
            "--as",
            "jsonl",
            jsonl_drained.to_str().unwrap(),
            "--json",
        ]),
        Success,
    ));
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
