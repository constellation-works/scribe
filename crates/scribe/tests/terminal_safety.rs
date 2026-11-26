//! Stored text reaches a pipe as text, never as terminal control
//! (STD-01 R9): an entry or tag description holding ESC, BEL or CR prints
//! them as visible escapes (`\u{1b}`) in human output, while `--json` keeps
//! the stored text verbatim behind its own escaping. Through the built binary against a
//! temp `HOME` and data root.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "support/terminal_safety.rs"]
mod subject;
mod support;

use crate::support::command::Input::Cli;
use crate::support::command::{Json, Text};
use serde_json::Value;
use subject::*;
use support::Fixture;
use support::command::Exit::Success;

const CONTENT: &str = "Keep \x1b[31mred\x1b[0m stable\r over\x07 time.";
const DESCRIPTION: &str = "Agent \x1b]0;pwned\x07 notes";

#[test]
fn stored_escapes_print_as_visible_escapes_and_json_keeps_them() {
    let fx = Fixture::new();
    let added: Value = fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--json",
            CONTENT,
        ],
        Json,
    );
    let id = added["id"].as_str().unwrap().to_owned();
    fx.check(
        Cli(&[
            "tag",
            "add",
            "notes",
            "--author",
            "opus",
            "--description",
            DESCRIPTION,
        ]),
        Success,
    );

    for args in [
        vec!["entry", "list"],
        vec!["entry", "show", id.as_str()],
        vec!["tag", "list"],
        vec!["tag", "show", "notes"],
    ] {
        let out = fx.check(Cli(&args), Success);
        no_control(&format!("{args:?}"), &out.stdout);
        no_control(&format!("{args:?} stderr"), &out.stderr);
    }
    let shown = fx.check(&["show", id.as_str()], Text);
    assert!(shown.contains("Keep \\u{1b}[31mred"), "{shown}");

    let json: Value = fx.check(&["show", id.as_str(), "--json"], Json);
    assert_eq!(json["content"], CONTENT);
}
