//! Phase 2 of 2_design.md §15.9: stored issue records become `issue`
//! entries with no model call (§15.11, specs/catchers.md), and the issue
//! surfaces are gone.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::Value;
use support::{Fixture, make_private, text};

const ISSUE: &str = include_str!("fixtures/issue-store/issue-1.md");
const SESSION: &str = "session:7e2d4c1a-9b3f-4e8d-a6c5-1f0e2d3c4b5a";

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

/// Put the format-23 issue fixture at `issues/issue-1.md`.
fn place_issue(fx: &Fixture) {
    let dir = fx.root().join("issues");
    fx.create_private_dir_all(&dir);
    let file = dir.join("issue-1.md");
    std::fs::write(&file, ISSUE).unwrap();
    make_private(&file);
}

/// The files under `pre-issue-<date>/`, with that directory's name.
fn moved_aside(fx: &Fixture) -> Vec<String> {
    let mut names = Vec::new();
    for dir in std::fs::read_dir(fx.root()).unwrap() {
        let dir = dir.unwrap();
        let name = dir.file_name().into_string().unwrap();
        if name.starts_with("pre-issue-") {
            for file in std::fs::read_dir(dir.path()).unwrap() {
                let file = file.unwrap().file_name().into_string().unwrap();
                names.push(format!("{name}/{file}"));
            }
        }
    }
    names.sort();
    names
}

fn a_write(fx: &Fixture, words: &str) {
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "preferences",
        "--author",
        "opus",
        words,
    ]);
}

#[test]
fn a_stored_issue_becomes_an_issue_entry_model_free_and_its_file_moves_aside() {
    let fx = Fixture::new();
    place_issue(&fx);
    // No `claude` on the path: a model call would fail the write.
    a_write(&fx, "Keep the API stable.");

    let entry = json(&fx.ok(&["entry", "show", "issue-1", "--json"]).stdout);
    assert_eq!(entry["tag"], "issue");
    assert_eq!(entry["status"], "active");
    assert_eq!(
        entry["content"],
        "GitHub API secondary rate limit during drains\n\
         Cause: Pushes from parallel drains share one token.\n\
         Fix: Serialise pushes per token.",
        "the header knew neither, so the newest occurrence's are used"
    );
    assert_eq!(
        entry["quote"], "can you investigate why the drain keeps failing with HTTP 429",
        "the first occurrence's verified request"
    );
    assert_eq!(entry["related"], serde_json::json!(["task:ORB-1", SESSION]));
    assert_eq!(entry["author"]["kind"], "agent");
    assert_eq!(entry["author"]["name"], "opus");
    assert_eq!(entry["project"], "orbit");
    assert_eq!(entry["host"], "host-a");
    assert_eq!(entry["content_from"]["kind"], "agent");
    assert!(entry["cite"].is_null(), "an issue kept no prompt hash");

    assert!(
        std::fs::read_dir(fx.root().join("issues"))
            .unwrap()
            .next()
            .is_none(),
        "issues/ is emptied"
    );
    let aside = moved_aside(&fx);
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert!(aside[0].ends_with("/issue-1.md"), "{aside:?}");
    let kept = std::fs::read_to_string(fx.root().join(&aside[0])).unwrap();
    assert_eq!(kept, ISSUE, "the file is moved, not rewritten");

    // Running again is a no-op: no second entry, nothing moved.
    let before = support::snapshot(&fx.root().join("entries/issue"));
    a_write(&fx, "Prefer small pull requests.");
    assert_eq!(support::snapshot(&fx.root().join("entries/issue")), before);
    assert_eq!(moved_aside(&fx), aside);

    // A crash between the write and the move leaves the file: the next
    // write finds the entry already there and only moves it.
    place_issue(&fx);
    a_write(&fx, "Name the units in sizes.");
    let listed = json(&fx.ok(&["entry", "list", "--tag", "issue", "--json"]).stdout);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(support::snapshot(&fx.root().join("entries/issue")), before);
}

#[test]
fn an_unparsable_issue_file_stays_and_the_write_still_lands() {
    let fx = Fixture::new();
    let dir = fx.root().join("issues");
    fx.create_private_dir_all(&dir);
    let file = dir.join("issue-1.md");
    std::fs::write(&file, "---\nformat: 23\nkind: issue\n---\nnot an issue\n").unwrap();
    make_private(&file);
    a_write(&fx, "Keep the API stable.");
    assert!(file.exists(), "left for the next write");
    assert!(moved_aside(&fx).is_empty());
    let listed = json(&fx.ok(&["entry", "list", "--tag", "issue", "--json"]).stdout);
    assert_eq!(listed["total"], 0);
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
