//! `transcript list` and `transcript show` through the built binary
//! (2_design.md §5.8): a read-only store, and `session catalog` refusing
//! a transcript that is not there.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use support::{Fixture, text};

const SESSION: &str = "c0ffee00-1111-4222-8333-444455556666";

#[test]
fn list_and_show_read_a_read_only_root_and_a_missing_catalog_is_not_found() {
    let fx = Fixture::new();
    let root = fx.root();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = root.join("config.toml");
    std::fs::write(
        &config,
        "[projects.orbit]\npaths = [\"codebases/orbit\"]\n\n[projects.scribe]\npaths = [\"codebases/scribe\"]\n",
    )
    .unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    let project = fx.home().join(".claude/projects/-work");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join(format!("{SESSION}.jsonl")),
        format!(
            "{}\n",
            json!({
                "type": "user",
                "uuid": "u-1",
                "timestamp": "2026-10-15T10:00:00Z",
                "cwd": "/work/constellation",
                "message": {"role": "user", "content": "Look at codebases/orbit and codebases/orbit/src."}
            })
        ),
    )
    .unwrap();

    let before = support::snapshot(&root);
    // Owner read-only: the store refuses group and other bits (STD-05 R9).
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
    let listed = fx.ok(&[
        "transcript",
        "list",
        "--since",
        "2026-10-01",
        "--min-turns",
        "1",
        "--json",
    ]);
    let shown = fx.ok(&["transcript", "show", "c0ff", "--json"]);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(support::snapshot(&root), before, "a read writes nothing");

    let list = serde_json::from_slice::<serde_json::Value>(&listed.stdout).unwrap();
    assert_eq!(list["transcripts"][0]["session"], SESSION);
    assert_eq!(list["transcripts"][0]["project"], "orbit");
    assert_eq!(list["transcripts"][0]["project_mentions"]["orbit"], 2);
    assert_eq!(list["transcripts"][0]["daniel_turns"], 1);
    assert!(list["transcripts"][0].get("daniel_first_turns").is_none());

    let show = serde_json::from_slice::<serde_json::Value>(&shown.stdout).unwrap();
    assert_eq!(show["daniel_first_turns"].as_array().unwrap().len(), 1);

    let missing = fx.run(&[
        "session",
        "catalog",
        "no-such-session",
        "--author",
        "daniel",
        "--json",
    ]);
    assert_eq!(missing.status.code(), Some(1), "{}", text(&missing.stderr));
    let err = serde_json::from_slice::<serde_json::Value>(&missing.stderr).unwrap();
    assert_eq!(err["code"], "transcript_not_found");
    assert!(
        !root.join("control.json").exists(),
        "a missing transcript is not switched on"
    );
    assert!(!root.join("sessions").exists());
    let audit = std::fs::read_to_string(root.join("audit.jsonl")).unwrap();
    assert!(audit.contains("session catalog"), "{audit}");
    assert!(audit.contains("transcript_not_found"), "{audit}");
}
