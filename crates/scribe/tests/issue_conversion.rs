//! The issue surfaces are gone, and stored issue records are no longer
//! converted to `issue` entries (2_design.md §15.9, phase 8): a
//! `pre-issue-*` archive or a leftover `issues/` file is imported into
//! `archive_records` like any other file.
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/issue_conversion.rs"]
mod subject;
mod support;

use crate::support::command::Input::Cli;
use serde_json::Value;
use subject::*;
use support::command::Exit::{Capture, Success};
use support::{Fixture, make_private, text};

#[test]
fn a_pre_issue_archive_is_imported_as_is_and_makes_no_issue_entry() {
    let fx = Fixture::seeded();
    let dir = place_archive(&fx);
    a_write(&fx, "Keep the API stable.");

    // No conversion: the issue record is archived whole, and no entry
    // is made of it.
    assert!(!dir.exists(), "the write imported and removed the archive");
    let listed = json(
        &fx.check(&["list", "--tag", "issue", "--json"], Success)
            .stdout,
    );
    assert_eq!(listed["total"], 0, "{listed}");
    assert!(
        fx.check(&["show", "issue-1", "--json"], Capture)
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
    let fx = Fixture::seeded();
    let dir = fx.root().join("issues");
    fx.create_private_dir_all(&dir);
    let file = dir.join("issue-1.md");
    std::fs::write(&file, ISSUE).unwrap();
    make_private(&file);
    a_write(&fx, "Keep the API stable.");
    assert!(!file.exists(), "the write imported the leftover file");
    let listed = json(
        &fx.check(&["list", "--tag", "issue", "--json"], Success)
            .stdout,
    );
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
        let out = fx.check(Cli(args), Capture);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn a_route_agent_issue_table_is_refused_as_config_invalid() {
    for (toml, needle) in [
        (
            "[tags.issue.capture]\nroute = \"agent\"\ntriggers = [\"root cause\"]\n",
            "line 2: tags.issue.capture.route: 'agent' is not one of: propose, write",
        ),
        (
            "[tags.issue.capture]\ntriggers = [\"root cause\"]\n",
            "tags.issue.capture needs from, route",
        ),
    ] {
        let fx = Fixture::new();
        fx.config(toml);
        let out = fx.check(Cli(&["tag", "list", "--json"]), Capture);
        assert_eq!(out.status.code(), Some(1), "{toml}");
        let err: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(err["code"], "config_invalid", "{toml}: {err}");
        assert!(
            err["error"].as_str().unwrap().contains(needle),
            "{toml}: {err}"
        );
    }
}
