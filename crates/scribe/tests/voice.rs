//! `[user] name` through the built binary: a configured name stays out of
//! tag output (docs/design/scribe/specs/store.md#config).
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::io::Write;
use support::{Fixture, text};

/// A configured name reaches prompts only: `tag show` and `tag list` print
/// the shipped description and criteria as "the user" (2_design.md §15.8).
#[test]
fn a_configured_name_stays_out_of_tag_output() {
    let fx = Fixture::new();
    fx.seed();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(fx.root().join("config.toml"))
        .unwrap();
    file.write_all(b"\n[user]\nname = \"Sam\"\n").unwrap();
    let shown: serde_json::Value =
        serde_json::from_slice(&fx.ok(&["tag", "show", "preferences", "--json"]).stdout).unwrap();
    assert_eq!(shown["description"], "how the user likes things done");
    assert_eq!(
        shown["capture"]["criteria"],
        "A standing rule from the user that applies beyond the work at hand."
    );
    let listed = fx.ok(&["tag", "list", "--json"]).stdout;
    let listed = text(&listed);
    assert!(
        !listed.contains("Sam") && !listed.contains("{user}"),
        "{listed}"
    );
}
