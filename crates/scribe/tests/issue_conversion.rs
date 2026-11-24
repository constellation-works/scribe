//! The issue surfaces are gone, and stored issue records are no longer
//! converted to `issue` entries (2_design.md §15.9, phase 8): a
//! `pre-issue-*` archive or a leftover `issues/` file is imported into
//! `archive_records` like any other file.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::Value;
use support::{Fixture, make_private, text};

const ISSUE: &str = include_str!("fixtures/issue-store/issue-1.md");
const SESSION: &str = "session:7e2d4c1a-9b3f-4e8d-a6c5-1f0e2d3c4b5a";

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

/// Leave `pre-issue-2026-11-22/issue-1.md`, the archive an issue
/// conversion left before phase 8.
fn place_archive(fx: &Fixture) -> std::path::PathBuf {
    let dir = fx.root().join("pre-issue-2026-11-22");
    fx.create_private_dir_all(&dir);
    let file = dir.join("issue-1.md");
    std::fs::write(&file, ISSUE).unwrap();
    make_private(&file);
    dir
}

fn a_write(fx: &Fixture, words: &str) {
    // The fixture's config redacts `GITHUB_TOKEN`, so a write needs a value.
    let out = fx
        .cmd(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            words,
        ])
        .env("GITHUB_TOKEN", "test-github-token-1234")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
}

#[test]
fn a_pre_issue_archive_is_imported_as_is_and_makes_no_issue_entry() {
    let fx = Fixture::new();
    fx.seed();
    let dir = place_archive(&fx);
    a_write(&fx, "Keep the API stable.");

    // No conversion: the issue record is archived whole, and no entry
    // is made of it.
    assert!(!dir.exists(), "the write imported and removed the archive");
    let listed = json(&fx.ok(&["entry", "list", "--tag", "issue", "--json"]).stdout);
    assert_eq!(listed["total"], 0, "{listed}");
    assert!(
        fx.run(&["entry", "show", "issue-1", "--json"])
            .status
            .code()
            != Some(0),
        "no issue-1 entry"
    );
    let conn = rusqlite::Connection::open(fx.root().join("scribe.db")).unwrap();
    let (kind, record): (String, String) = conn
        .query_row(
            "SELECT kind, record FROM archive_records WHERE path = 'issue-1.md'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "issue");
    let record: Value = serde_json::from_str(&record).unwrap();
    assert_eq!(record["front"]["id"], "issue-1");
    assert_eq!(record["front"]["related"][1], SESSION);
    assert!(
        record["body"]
            .as_str()
            .unwrap()
            .contains("Pushes from parallel drains share one token."),
        "{record}"
    );

    // A later write imports nothing more.
    let before: i64 = conn
        .query_row("SELECT count(*) FROM archive_records", [], |r| r.get(0))
        .unwrap();
    a_write(&fx, "Prefer small pull requests.");
    let after: i64 = conn
        .query_row("SELECT count(*) FROM archive_records", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn a_leftover_issues_directory_is_archived_not_converted() {
    let fx = Fixture::new();
    fx.seed();
    let dir = fx.root().join("issues");
    fx.create_private_dir_all(&dir);
    let file = dir.join("issue-1.md");
    std::fs::write(&file, ISSUE).unwrap();
    make_private(&file);
    a_write(&fx, "Keep the API stable.");
    assert!(!file.exists(), "the write imported the leftover file");
    let listed = json(&fx.ok(&["entry", "list", "--tag", "issue", "--json"]).stdout);
    assert_eq!(listed["total"], 0, "no conversion: {listed}");
    let archived: i64 = rusqlite::Connection::open(fx.root().join("scribe.db"))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM archive_records WHERE path = 'issues/issue-1.md'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archived, 1);
}

#[test]
fn the_issue_noun_and_search_kind_are_unknown() {
    let fx = Fixture::new();
    for args in [
        &["issue", "list"][..],
        &["issue", "stats"],
        &["issue", "add", "--help"],
        &["search", "rate limit", "--kind", "issue"],
    ] {
        let out = fx.run(args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn a_route_agent_issue_table_warns_and_catches_nothing() {
    let fx = Fixture::new();
    fx.config("[tags.issue.capture]\nroute = \"agent\"\ntriggers = [\"root cause\"]\n");
    let stderr = text(&fx.ok(&["tag", "list"]).stderr);
    assert!(stderr.contains("route \"agent\""), "{stderr}");
    assert!(stderr.contains("specs/catchers.md"), "{stderr}");
    let shown = json(&fx.ok(&["tag", "show", "issue", "--json"]).stdout);
    assert!(shown["capture"]["route"].is_null(), "{shown}");
    assert!(shown["capture"]["triggers"].is_null(), "{shown}");
}
