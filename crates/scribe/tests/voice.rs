//! `[user] name` through the built binary: a configured name stays out of
//! tag output (docs/design/scribe/specs/store.md#config).
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "support/voice.rs"]
mod subject;
mod support;

use crate::support::command::{Json, Text};

/// A configured name reaches prompts only: `tag show` and `tag list` print
/// the shipped description and criteria as "the user" (2_design.md §15.8).
#[test]
fn a_configured_name_stays_out_of_tag_output() {
    let fx = subject::named_user("Sam");
    let shown: serde_json::Value = fx.check(&["show", "preferences", "--json"], Json);
    assert_eq!(shown["description"], "how the user likes things done");
    assert_eq!(
        shown["capture"]["criteria"],
        "A standing rule from the user that applies beyond the work at hand."
    );
    let listed = fx.check(&["list", "--json"], Text);
    assert!(
        !listed.contains("Sam") && !listed.contains("{user}"),
        "{listed}"
    );
}
