//! Stored text reaches a pipe as text, never as terminal control
//! (STD-01 R9): an entry or tag description holding ESC, BEL or CR prints
//! them as U+FFFD in human output, while `--json` keeps the stored text
//! verbatim behind its own escaping. Through the built binary against a
//! temp `HOME` and data root.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::Value;
use support::{Fixture, text};

const CONTENT: &str = "Keep \x1b[31mred\x1b[0m stable\r over\x07 time.";
const DESCRIPTION: &str = "Agent \x1b]0;pwned\x07 notes";

fn no_control(label: &str, bytes: &[u8]) {
    let out = text(bytes);
    let bad: Vec<char> = out
        .chars()
        .filter(|c| c.is_control() && *c != '\n' && *c != '\t')
        .collect();
    assert!(bad.is_empty(), "{label} printed {bad:?}: {out}");
}

#[test]
fn stored_escapes_print_as_replacement_characters_and_json_keeps_them() {
    let fx = Fixture::new();
    let added: Value = serde_json::from_slice(
        &fx.ok(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--json",
            CONTENT,
        ])
        .stdout,
    )
    .unwrap();
    let id = added["id"].as_str().unwrap().to_owned();
    fx.ok(&[
        "tag",
        "add",
        "notes",
        "--author",
        "opus",
        "--description",
        DESCRIPTION,
    ]);

    for args in [
        vec!["entry", "list"],
        vec!["entry", "show", id.as_str()],
        vec!["tag", "list"],
        vec!["tag", "show", "notes"],
    ] {
        let out = fx.ok(&args);
        no_control(&format!("{args:?}"), &out.stdout);
        no_control(&format!("{args:?} stderr"), &out.stderr);
    }
    let shown = text(&fx.ok(&["entry", "show", id.as_str()]).stdout);
    assert!(shown.contains("Keep \u{FFFD}[31mred"), "{shown}");

    let json: Value =
        serde_json::from_slice(&fx.ok(&["entry", "show", id.as_str(), "--json"]).stdout).unwrap();
    assert_eq!(json["content"], CONTENT);
}
