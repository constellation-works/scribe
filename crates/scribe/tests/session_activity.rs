//! End-to-end tests of S10's manual route through the built binary
//! (docs/design/scribe/2_design.md §5.7, §10 S10): `scribe session
//! deactivate` and `scribe session activate` change `active` and
//! `keep_active`, record `active_change` with route `manual`, and write one
//! audit line; a repeat changes nothing and writes none; deactivating a
//! pinned session unpins it and says so.
//!
//! Every run is the built binary with a temp `HOME` and data root.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::Value;
use support::{Fixture, text};

const SESSION: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

/// Run with the `[redact] env` variable the fixture config names, so a
/// write may go ahead.
fn run(fx: &Fixture, args: &[&str]) -> std::process::Output {
    fx.cmd(args)
        .env("GITHUB_TOKEN", "fixture-token-value-1234")
        .output()
        .unwrap()
}

fn ok(fx: &Fixture, args: &[&str]) -> std::process::Output {
    let out = run(fx, args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{args:?}: {}",
        text(&out.stderr)
    );
    out
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|e| panic!("{e}: {}", text(bytes)))
}

fn audit(fx: &Fixture) -> Vec<Value> {
    support::audit_lines(&fx.root())
}

/// The record as its file reads (`scribe export`).
fn record_bytes(fx: &Fixture) -> Vec<u8> {
    support::record_text(&fx.root(), SESSION)
        .expect("the record")
        .into_bytes()
}

#[test]
fn deactivate_hides_the_session_records_why_and_audits_once() {
    let fx = Fixture::new();
    fx.seed();
    let before = json(&ok(&fx, &["session", "show", SESSION, "--json"]).stdout);

    let out = ok(
        &fx,
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            "the merge policy was replaced",
            "--cite",
            "decisions-2",
            "--json",
        ],
    );
    let doc = json(&out.stdout);
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
    assert_eq!(doc["id"], SESSION);
    assert_eq!(doc["active"], false);
    assert_eq!(doc["keep_active"], false);
    let change = &doc["active_change"];
    assert_eq!(change["route"], "manual");
    assert_eq!(change["by"]["kind"], "agent");
    assert_eq!(change["by"]["name"], "opus");
    assert_eq!(change["reason"], "the merge policy was replaced");
    assert_eq!(change["cite"], "decisions-2");
    assert_eq!(doc["updated"], before["updated"], "updated never moves");
    assert_eq!(doc["log"], before["log"], "the text never changes");

    let lines = audit(&fx);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["command"], "session deactivate");
    assert_eq!(lines[0]["status"], "success");
    assert_eq!(lines[0]["target"], format!("session:{SESSION}"));
    let raw = support::audit_text(&fx.root());
    assert!(!raw.contains("merge policy"), "no argument values: {raw}");

    let listed = json(&ok(&fx, &["session", "list", "--json"]).stdout);
    assert!(
        !listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == SESSION),
        "hidden from the list"
    );

    // A repeat: the record unchanged, `nothing changed`, no audit line.
    let bytes = record_bytes(&fx);
    let out = ok(
        &fx,
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            "again",
        ],
    );
    assert!(
        text(&out.stderr).contains("nothing changed"),
        "{}",
        text(&out.stderr)
    );
    assert!(!out.stdout.is_empty(), "the record is still printed");
    assert_eq!(record_bytes(&fx), bytes);
    assert_eq!(audit(&fx).len(), 1, "a repeat writes no audit line");
}

#[test]
fn activate_pins_and_a_pinned_session_deactivated_by_hand_is_unpinned() {
    let fx = Fixture::new();
    fx.seed();
    let activate = |extra: &[&str]| {
        let args = [
            &["session", "activate", SESSION, "--author", "opus"][..],
            extra,
        ]
        .concat();
        ok(&fx, &args)
    };

    let out = activate(&["--json"]);
    assert!(text(&out.stderr).contains("nothing changed"));
    assert!(audit(&fx).is_empty(), "already active: no audit line");

    let out = activate(&["--pin", "--reason", "still the reference", "--json"]);
    let doc = json(&out.stdout);
    assert_eq!(doc["active"], true);
    assert_eq!(doc["keep_active"], true);
    assert_eq!(doc["active_change"]["route"], "manual");
    assert_eq!(doc["active_change"]["reason"], "still the reference");
    assert_eq!(doc["active_change"]["cite"], Value::Null);
    assert_eq!(audit(&fx).len(), 1);

    let out = ok(
        &fx,
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            "gone after all",
            "--json",
        ],
    );
    let doc = json(&out.stdout);
    assert_eq!(doc["active"], false);
    assert_eq!(doc["keep_active"], false);
    assert!(
        text(&out.stderr).contains("unpinned"),
        "says so: {}",
        text(&out.stderr)
    );

    let doc = json(&activate(&["--json"]).stdout);
    assert_eq!(doc["active"], true);
    assert_eq!(doc["keep_active"], false);
    assert_eq!(doc["active_change"]["reason"], "activated by hand");
    let commands: Vec<String> = audit(&fx)
        .iter()
        .map(|l| l["command"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        commands,
        ["session activate", "session deactivate", "session activate"]
    );
}

#[test]
fn bad_arguments_are_usage_errors_and_an_unknown_session_is_not_found() {
    let fx = Fixture::new();
    fx.seed();
    let bytes = record_bytes(&fx);
    let segment = format!("session:{SESSION}#1");
    let long = "x".repeat(301);
    for args in [
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            " ",
            "--json",
        ][..],
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            &long,
            "--json",
        ][..],
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "Opus!",
            "--reason",
            "x",
            "--json",
        ][..],
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            "x",
            "--cite",
            "a b",
            "--json",
        ][..],
        &[
            "session",
            "deactivate",
            &segment,
            "--author",
            "opus",
            "--reason",
            "x",
            "--json",
        ][..],
        &["session", "deactivate", SESSION, "--reason", "x", "--json"][..],
        &[
            "session", "activate", SESSION, "--author", "opus", "--pin", "--unpin", "--json",
        ][..],
    ] {
        let out = run(&fx, args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert_eq!(json(&out.stderr)["code"], "usage_error", "{args:?}");
    }
    assert_eq!(record_bytes(&fx), bytes);

    let out = run(
        &fx,
        &[
            "session",
            "deactivate",
            "1111aaaa-0000-4000-8000-000000000000",
            "--author",
            "opus",
            "--reason",
            "x",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json(&out.stderr)["code"], "session_not_found");

    // The cite form of a whole session is accepted.
    let cite = format!("session:{SESSION}");
    let doc = json(
        &ok(
            &fx,
            &[
                "session",
                "deactivate",
                &cite,
                "--author",
                "opus",
                "--reason",
                "x",
                "--json",
            ],
        )
        .stdout,
    );
    assert_eq!(doc["active"], false);
}
