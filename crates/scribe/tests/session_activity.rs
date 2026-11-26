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
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/session_activity.rs"]
mod subject;
mod support;

use crate::support::command::Exit::{Capture, Success};
use serde_json::Value;
use subject::*;
use support::{Fixture, text};

#[test]
fn deactivate_hides_the_session_records_why_and_audits_once() {
    let fx = Fixture::seeded();
    let before = json(
        &fx.check(&["session", "show", SESSION, "--json"], Success)
            .stdout,
    );

    let out = fx.check(
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
        Success,
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

    let listed = json(&fx.check(&["session", "list", "--json"], Success).stdout);
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
    let out = fx.check(
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            "again",
        ],
        Success,
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
    let fx = Fixture::seeded();
    let activate = |extra: &[&str]| {
        let args = [
            &["session", "activate", SESSION, "--author", "opus"][..],
            extra,
        ]
        .concat();
        fx.check(&args, Success)
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

    let out = fx.check(
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
        Success,
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
    let fx = Fixture::seeded();
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
        let out = fx.check(args, Capture);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert_eq!(json(&out.stderr)["code"], "usage_error", "{args:?}");
    }
    assert_eq!(record_bytes(&fx), bytes);

    let out = fx.check(
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
        Capture,
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json(&out.stderr)["code"], "session_not_found");

    // The cite form of a whole session is accepted.
    let cite = format!("session:{SESSION}");
    let doc = json(
        &fx.check(
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
            Success,
        )
        .stdout,
    );
    assert_eq!(doc["active"], false);
}

/// docs/guide.md: a `session:<id>` reference keeps an id no session matches
/// as written, while a command that acts on a session refuses it.
#[test]
fn a_session_reference_is_kept_as_typed_and_a_lookup_of_a_session_is_not_found() {
    let fx = Fixture::new();
    fx.check(
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--related",
            "session:abcd",
            "Use the stable API",
        ],
        Success,
    );
    let listed = fx.check(&["entry", "list", "--related", "session:abcd"], Success);
    assert!(
        text(&listed.stdout).contains("Use the stable API"),
        "{}",
        text(&listed.stdout)
    );

    for (args, code) in [
        (
            &["session", "show", "abcd", "--json"][..],
            "session_not_found",
        ),
        (
            &[
                "session",
                "deactivate",
                "abcd",
                "--author",
                "opus",
                "--reason",
                "x",
                "--json",
            ][..],
            "session_not_found",
        ),
        (
            &["session", "activate", "abcd", "--author", "opus", "--json"][..],
            "session_not_found",
        ),
        (
            &["session", "pass", "--session", "abcd", "--json"][..],
            "session_not_on",
        ),
    ] {
        let out = fx.check(args, Capture);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert_eq!(json(&out.stderr)["code"], code, "{args:?}");
    }
}
