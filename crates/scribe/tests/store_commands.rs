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
        assert!(exported.starts_with("---\nformat: 27\n"), "{exported}");
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
