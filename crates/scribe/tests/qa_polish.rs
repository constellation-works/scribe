//! Regression tests for the QA sweep's small CLI defects, each through the
//! built binary against a temp `HOME` and data root:
//!
//! 1. a `--json` usage error keeps clap's detail (the argument name);
//! 2. `session deactivate --cite` accepts only a record that exists;
//! 3. every id lookup refuses a too-short prefix as a usage error, and every
//!    not-found error names the id and the store path searched;
//! 4. the table detail view never runs a long label into its value;
//! 5. `tag add` of an existing tag says so;
//! 6. a bad argument is reported before an unset `[redact] env` name.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "support/qa_polish.rs"]
mod subject;
mod support;

use crate::support::command::Exit::{Code, Success};
use crate::support::command::with_env;
use subject::*;
use support::text;

#[test]
fn a_json_usage_error_keeps_the_missing_argument_name() {
    let fx = support::Fixture::seeded();
    let err = failure(&fx, &["entry", "add", "--tag", "preferences"], 2);
    assert_eq!(err["code"], "usage_error");
    let message = err["error"].as_str().unwrap();
    assert!(message.contains("--author"), "{message}");
    assert!(message.contains("CONTENT"), "{message}");
    assert!(!message.contains("Usage:"), "{message}");
    // Not only missing arguments: a rejected value names the argument too.
    let err = failure(&fx, &["entry", "show", "decisions-1", "--bogus"], 2);
    assert!(err["error"].as_str().unwrap().contains("--bogus"));
}

#[test]
fn a_cite_must_name_a_record_that_exists() {
    let fx = support::Fixture::seeded();
    let base = [
        "session",
        "deactivate",
        SESSION,
        "--author",
        "opus",
        "--reason",
        "gone",
    ];
    for bad in [
        "bogus",
        "session:",
        "session:not-a-session",
        "session:0f8e7a52#9",
        "decisions-99",
    ] {
        let mut args = base.to_vec();
        args.extend(["--cite", bad]);
        let err = failure(&fx, &args, 2);
        assert_eq!(err["code"], "usage_error", "{bad}");
        assert!(err["error"].as_str().unwrap().contains("--cite"), "{bad}");
    }
    // Nothing was stored by the refused attempts.
    let shown = json(
        &fx.check(&["session", "show", SESSION, "--json"], Success)
            .stdout,
    );
    assert_eq!(shown["active"], true);

    let mut args = base.to_vec();
    args.extend(["--cite", "session:0f8e#1"]);
    let stored = json(&fx.check(&[&args[..], &["--json"]].concat(), Success).stdout);
    assert_eq!(
        stored["active_change"]["cite"], "session:0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01#1",
        "a cite is stored in full"
    );
}

#[test]
fn an_entry_id_is_a_valid_cite() {
    let fx = support::Fixture::seeded();
    let out = fx.check(
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            "gone",
            "--cite",
            "decisions-1",
            "--json",
        ],
        Success,
    );
    assert_eq!(json(&out.stdout)["active_change"]["cite"], "decisions-1");
}

#[test]
fn a_too_short_prefix_is_a_usage_error_for_every_noun() {
    let fx = support::Fixture::seeded();
    for args in [
        &["session", "show", "7c"][..],
        &[
            "session",
            "deactivate",
            "7c",
            "--author",
            "opus",
            "--reason",
            "gone",
        ],
        &["entry", "show", "dec"],
        &[
            "proposal", "update", "p-", "--author", "opus", "--reason", "x",
        ],
    ] {
        let err = failure(&fx, args, 2);
        assert_eq!(err["code"], "usage_error", "{args:?}");
        assert!(
            err["error"].as_str().unwrap().contains("four characters"),
            "{args:?}: {err}"
        );
    }
}

#[test]
fn every_not_found_error_names_the_id_and_the_path_searched() {
    let fx = support::Fixture::seeded();
    let root = fx.root();
    let root = root.to_str().unwrap();
    let write = ["--author", "opus", "--reason", "x"];
    let cases: Vec<(Vec<&str>, &str, &str, &str)> = vec![
        (
            vec!["entry", "show", "decisions-99"],
            "entry_not_found",
            "decisions-99",
            "entries/decisions/decisions-99.md",
        ),
        (
            vec!["entry", "show", "zzzz"],
            "entry_not_found",
            "zzzz",
            "entries",
        ),
        (
            vec!["session", "show", "feedface"],
            "session_not_found",
            "feedface",
            "sessions",
        ),
        (
            vec!["proposal", "update", "p-99"],
            "proposal_not_found",
            "p-99",
            "proposals.json",
        ),
        (
            vec!["proposal", "update", "zzzz"],
            "proposal_not_found",
            "zzzz",
            "proposals.json",
        ),
    ];
    for (mut args, code, id, place) in cases {
        if args[0] == "proposal" {
            args.extend(write);
        }
        let err = failure(&fx, &args, 1);
        assert_eq!(err["code"], code, "{args:?}");
        let message = err["error"].as_str().unwrap();
        assert!(message.contains(id), "{args:?}: {message}");
        assert!(
            message.contains(&format!("{root}/{place}")),
            "{args:?}: {message}"
        );
    }
}

#[test]
fn a_missing_transcript_names_the_id_and_the_directory() {
    let fx = support::Fixture::seeded();
    let projects = fx.elsewhere("claude/projects");
    std::fs::create_dir_all(&projects).unwrap();
    let out = fx.check(
        with_env(
            &[
                "transcript",
                "show",
                "deadbeef-0000-4000-8000-000000000000",
                "--json",
            ],
            "CLAUDE_CONFIG_DIR",
            fx.elsewhere("claude"),
        ),
        Code(1),
    );
    let err = json(&out.stderr);
    assert_eq!(err["code"], "transcript_not_found");
    let message = err["error"].as_str().unwrap();
    assert!(message.contains("deadbeef-0000-4000-8000-000000000000"));
    assert!(message.contains(projects.to_str().unwrap()), "{message}");
}

#[test]
fn the_table_detail_view_separates_a_long_label_from_its_value() {
    let fx = support::Fixture::seeded();
    fx.check(
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "squash merges",
        ],
        Success,
    );
    fx.check(
        &[
            "entry",
            "retire",
            "preferences-2",
            "--author",
            "opus",
            "--reason",
            "obsolete",
        ],
        Success,
    );
    let out = fx.check(
        &["entry", "show", "preferences-2", "--format", "table"],
        Success,
    );
    let shown = text(&out.stdout);
    let line = shown
        .lines()
        .find(|l| l.starts_with("retirement_reason"))
        .unwrap_or_else(|| panic!("no retirement_reason line:\n{shown}"));
    assert_eq!(
        line.split_whitespace().collect::<Vec<_>>(),
        ["retirement_reason", "obsolete"]
    );
    assert!(!shown.contains("retirement_reasonobsolete"), "{shown}");
}

#[test]
fn adding_an_existing_tag_says_it_exists() {
    let fx = support::Fixture::seeded();
    let args = [
        "tag",
        "add",
        "qa-tag",
        "--author",
        "opus",
        "--description",
        "qa notes",
    ];
    fx.check(&args, Success);
    let err = failure(&fx, &args, 1);
    assert_eq!(err["code"], "tag_conflict");
    let message = err["error"].as_str().unwrap();
    assert!(message.contains("'qa-tag' already exists"), "{message}");
    assert!(!message.contains("claimed by both"), "{message}");
    // A configured tag, by name and by alias, is told apart.
    let configured = failure(
        &fx,
        &[
            "tag",
            "add",
            "preferences",
            "--author",
            "opus",
            "--description",
            "x",
        ],
        1,
    );
    assert!(
        configured["error"]
            .as_str()
            .unwrap()
            .contains("already exists"),
        "{configured}"
    );
    let alias = failure(
        &fx,
        &[
            "tag",
            "add",
            "pref",
            "--author",
            "opus",
            "--description",
            "x",
        ],
        1,
    );
    assert!(
        alias["error"].as_str().unwrap().contains("alias"),
        "{alias}"
    );
}

#[test]
fn an_unknown_tag_update_target_is_reported_before_an_unset_redact_name() {
    let fx = support::Fixture::seeded();
    let err = without_secret(
        &fx,
        &[
            "tag",
            "update",
            "missing-tag",
            "--author",
            "opus",
            "--add-alias",
            "fresh-alias",
        ],
    );
    assert_eq!(err["code"], "tag_not_found");

    fx.check(
        &[
            "tag",
            "add",
            "qa-tag",
            "--author",
            "opus",
            "--description",
            "qa notes",
        ],
        Success,
    );
    let err = without_secret(
        &fx,
        &[
            "tag",
            "update",
            "qa-tag",
            "--author",
            "opus",
            "--add-alias",
            "pref",
        ],
    );
    assert_eq!(err["code"], "tag_conflict");
}

#[test]
fn an_invalid_entry_topic_patch_is_reported_before_an_unset_redact_name() {
    let fx = support::Fixture::seeded();
    let mut args = vec!["entry", "update", "decisions-1", "--author", "opus"];
    args.extend([
        "--add-topic",
        "qa-one",
        "--add-topic",
        "qa-two",
        "--add-topic",
        "qa-three",
        "--add-topic",
        "qa-four",
        "--add-topic",
        "qa-five",
        "--add-topic",
        "qa-six",
    ]);
    let err = without_secret(&fx, &args);
    assert_eq!(err["code"], "invalid_topics");
}

#[test]
fn a_bad_argument_is_reported_before_an_unset_redact_name() {
    let fx = support::Fixture::seeded();
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (
            vec!["entry", "add", "--tag", "nope", "--author", "opus", "text"],
            "tag_not_found",
        ),
        (
            vec![
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "  ",
            ],
            "invalid_text",
        ),
        (
            vec![
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "daniel",
                "x",
            ],
            "reserved_author",
        ),
        (
            vec![
                "entry",
                "retire",
                "decisions-99",
                "--author",
                "opus",
                "--reason",
                "x",
            ],
            "entry_not_found",
        ),
        (
            vec![
                "entry",
                "retire",
                "decisions-1",
                "--author",
                "opus",
                "--reason",
                " ",
            ],
            "invalid_text",
        ),
        (
            vec![
                "tag",
                "add",
                "preferences",
                "--author",
                "opus",
                "--description",
                "x",
            ],
            "tag_conflict",
        ),
        (
            vec![
                "tag",
                "add",
                "new-tag",
                "--author",
                "opus",
                "--description",
                " ",
            ],
            "invalid_text",
        ),
        (
            vec![
                "proposal",
                "add",
                "--kind",
                "supersedes",
                "--older",
                "decisions-1",
                "--reason",
                "x",
                "--author",
                "opus",
            ],
            "proposal_invalid",
        ),
        (
            vec![
                "proposal", "update", "p-99", "--author", "opus", "--reason", "x",
            ],
            "proposal_not_found",
        ),
    ];
    for (args, code) in cases {
        let err = without_secret(&fx, &args);
        assert_eq!(err["code"], code, "{args:?}: {err}");
    }
    // A valid write still refuses, so nothing is stored unmasked.
    let err = without_secret(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "fine",
        ],
    );
    assert_eq!(err["code"], "redact_env_unset");
    let list = json(
        &fx.check(
            &["entry", "list", "--tag", "preferences", "--json"],
            Success,
        )
        .stdout,
    );
    assert_eq!(list["total"], 1, "no entry was written");
}
