//! Regression tests for the QA CLI polish defects (ORB-23999) through the
//! built binary.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::Value;
use std::process::Output;
use support::{Fixture, text};

const SESSION: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

fn run(fx: &Fixture, args: &[&str]) -> Output {
    fx.cmd(args)
        .env("GITHUB_TOKEN", "fixture-token-value-1234")
        .output()
        .unwrap()
}

fn run_without_token(fx: &Fixture, args: &[&str]) -> Output {
    fx.cmd(args).output().unwrap()
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|e| panic!("{e}: {}", text(bytes)))
}

fn err(out: &Output) -> Value {
    // A tracing warning may precede the JSON error object on stderr.
    let last = text(&out.stderr)
        .lines()
        .rfind(|l| l.trim_start().starts_with('{'))
        .unwrap_or_default()
        .to_owned();
    serde_json::from_str(&last).unwrap()
}

#[test]
fn json_usage_error_keeps_the_argument_detail() {
    let fx = Fixture::new();
    fx.seed();
    let out = run(&fx, &["entry", "add", "--tag", "decisions", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    let e = err(&out);
    assert_eq!(e["code"], "usage_error");
    let message = e["error"].as_str().unwrap();
    assert!(message.contains("--author"), "{message}");
    assert!(message.contains("CONTENT"), "{message}");
}

#[test]
fn deactivate_cite_must_name_a_known_record() {
    let fx = Fixture::new();
    fx.seed();
    for cite in [
        "bogus",
        "session:1111aaaa-0000-4000-8000-000000000000",
        "decisions-99",
        "session:7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02#99",
    ] {
        let out = run(
            &fx,
            &[
                "session",
                "deactivate",
                SESSION,
                "--author",
                "opus",
                "--reason",
                "gone",
                "--cite",
                cite,
                "--json",
            ],
        );
        assert_eq!(out.status.code(), Some(2), "{cite}: {}", text(&out.stderr));
        assert_eq!(err(&out)["code"], "usage_error", "{cite}");
        let shown = json(&run(&fx, &["session", "show", SESSION, "--json"]).stdout);
        assert_eq!(shown["active"], true, "bad cite {cite} changed the record");
    }
    // A valid entry cite and a valid segment cite are stored canonically.
    let out = run(
        &fx,
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            "gone",
            "--cite",
            "decisions-2",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(json(&out.stdout)["active_change"]["cite"], "decisions-2");
}

#[test]
fn id_prefixes_are_consistent_across_nouns() {
    let fx = Fixture::new();
    fx.seed();
    // Too-short prefixes are usage errors (exit 2) for every noun.
    for args in [
        &["session", "show", "7c", "--json"][..],
        &["entry", "show", "dec", "--json"][..],
        &[
            "proposal", "update", "p-", "--author", "opus", "--reason", "x", "--json",
        ][..],
        &[
            "session",
            "deactivate",
            "7c",
            "--author",
            "opus",
            "--reason",
            "x",
            "--json",
        ][..],
    ] {
        let out = run(&fx, args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert_eq!(err(&out)["code"], "usage_error", "{args:?}");
    }
    // Not-found errors name the id typed and do not leak the store path.
    let root = fx.root().display().to_string();
    for (args, code, id) in [
        (
            &["entry", "show", "zzzz", "--json"][..],
            "entry_not_found",
            "zzzz",
        ),
        (
            &["session", "show", "absent-session", "--json"][..],
            "session_not_found",
            "absent-session",
        ),
        (
            &["entry", "show", "decisions-7", "--json"][..],
            "entry_not_found",
            "decisions-7",
        ),
        (
            &[
                "proposal", "update", "p-999", "--author", "opus", "--reason", "x", "--json",
            ][..],
            "proposal_not_found",
            "p-999",
        ),
    ] {
        let out = run(&fx, args);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{args:?}: {}",
            text(&out.stderr)
        );
        let e = err(&out);
        assert_eq!(e["code"], code, "{args:?}");
        let message = e["error"].as_str().unwrap();
        assert!(message.contains(id), "{message}");
        assert!(!message.contains(&root), "leaks store path: {message}");
    }
}

#[test]
fn detail_view_separates_long_labels_from_their_values() {
    let fx = Fixture::new();
    fx.seed();
    let added = json(
        &run(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "decisions",
                "--author",
                "opus",
                "retire me",
                "--json",
            ],
        )
        .stdout,
    );
    let id = added["id"].as_str().unwrap().to_owned();
    let out = run(
        &fx,
        &[
            "entry", "retire", &id, "--author", "opus", "--reason", "obsolete",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let out = run(&fx, &["entry", "show", &id, "--format", "table"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("retirement_reason obsolete"), "{stdout}");
    assert!(!stdout.contains("retirement_reasonobsolete"), "{stdout}");
}

#[test]
fn adding_an_existing_tag_says_it_already_exists() {
    let fx = Fixture::new();
    fx.seed();
    let first = run(
        &fx,
        &[
            "tag",
            "add",
            "qa-tag",
            "--author",
            "opus",
            "--description",
            "test",
            "--json",
        ],
    );
    assert_eq!(first.status.code(), Some(0), "{}", text(&first.stderr));
    for name in ["qa-tag", "decisions"] {
        let out = run(
            &fx,
            &[
                "tag",
                "add",
                name,
                "--author",
                "opus",
                "--description",
                "again",
                "--json",
            ],
        );
        assert_eq!(out.status.code(), Some(1), "{name}");
        let message = err(&out)["error"].as_str().unwrap().to_owned();
        assert!(message.contains("tag already exists"), "{message}");
        assert!(!message.contains("claimed by both"), "{message}");
    }
}

#[test]
fn argument_validation_precedes_the_redact_env_check() {
    let fx = Fixture::new();
    fx.seed();
    // The fixture config names GITHUB_TOKEN under [redact] env; these runs
    // do not set it, so only argument validation may fire first.
    for (args, code) in [
        (
            &[
                "entry",
                "add",
                "--tag",
                "nosuchtag",
                "--author",
                "opus",
                "hello",
                "--json",
            ][..],
            "tag_not_found",
        ),
        (
            &[
                "entry",
                "add",
                "--tag",
                "decisions",
                "--author",
                "opus",
                "",
                "--json",
            ][..],
            "invalid_text",
        ),
        (
            &[
                "proposal", "add", "--kind", "stale", "--older", "zzzz", "--reason", "x",
                "--author", "opus", "--json",
            ][..],
            "entry_not_found",
        ),
        (
            &[
                "tag",
                "add",
                "qa-tag",
                "--author",
                "opus",
                "--description",
                "test",
                "--json",
            ][..],
            // qa-tag does not exist yet, so this one reaches the redact check.
            "redact_env_unset",
        ),
    ] {
        let out = run_without_token(&fx, args);
        assert_eq!(err(&out)["code"], code, "{args:?}: {}", text(&out.stderr));
    }
    // A valid write still fails closed on the unset redact name.
    let out = run_without_token(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "decisions",
            "--author",
            "opus",
            "hello",
            "--json",
        ],
    );
    assert_eq!(err(&out)["code"], "redact_env_unset");
}
