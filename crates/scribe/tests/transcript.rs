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
    support::make_private(&project.join(format!("{SESSION}.jsonl")));

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
    assert_eq!(list["transcripts"][0]["user_turns"], 1);
    assert!(list["transcripts"][0].get("user_first_turns").is_none());

    let show = serde_json::from_slice::<serde_json::Value>(&shown.stdout).unwrap();
    assert_eq!(show["user_first_turns"].as_array().unwrap().len(), 1);

    let missing = fx.run(&[
        "session",
        "catalog",
        "no-such-session",
        "--author",
        "opus",
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
    let audit = support::audit_text(&root);
    assert!(audit.contains("session catalog"), "{audit}");
    assert!(audit.contains("transcript_not_found"), "{audit}");
}

#[test]
fn list_skips_a_transcript_whose_mtime_is_before_since() {
    let fx = Fixture::new();
    let project = fx.home().join(".claude/projects/-work");
    std::fs::create_dir_all(&project).unwrap();
    let old_id = "aaaaaaaa-1111-4222-8333-444455556666";
    let new_id = "bbbbbbbb-1111-4222-8333-444455556666";
    let write_one = |id: &str, mtime_secs: u64| {
        let path = project.join(format!("{id}.jsonl"));
        std::fs::write(
            &path,
            format!(
                "{}\n",
                json!({
                    "type": "user",
                    "uuid": "u-1",
                    "timestamp": "2026-11-15T10:00:00Z",
                    "cwd": "/work/scribe",
                    "message": {"role": "user", "content": "hello from a transcript"}
                })
            ),
        )
        .unwrap();
        support::make_private(&path);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(mtime_secs),
        )
        .unwrap();
    };
    // Both carry an in-range message timestamp; only mtime differs.
    write_one(old_id, 1_790_812_800); // 2026-10-01, before --since 2026-11-01
    write_one(new_id, 1_794_700_800); // 2026-11-15, on/after --since

    let listed = fx.ok(&[
        "transcript",
        "list",
        "--since",
        "2026-11-01",
        "--min-turns",
        "1",
        "--json",
    ]);
    let list = serde_json::from_slice::<serde_json::Value>(&listed.stdout).unwrap();
    let sessions: Vec<&str> = list["transcripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["session"].as_str().unwrap())
        .collect();
    assert_eq!(
        sessions,
        [new_id],
        "the pre-since-mtime file is skipped without being read, the newer one is listed"
    );
    assert_eq!(list["total"], 1);
}

#[test]
fn exact_provider_id_reads_transcript_when_an_unrelated_group_roster_is_corrupt() {
    const ID: &str = "d00d0000-1111-4222-8333-444455556666";
    let fx = Fixture::new();
    fx.ok(&["store", "migrate"]);
    support::sql(
        &fx.root(),
        r#"INSERT INTO groups (title, kind, project, opened, opener, related)
           VALUES ('Unrelated', 'review', 'scribe', '2026-11-22T09:00:00Z', 'other-session', '["invalid"]')"#,
    );

    let project = fx.home().join(".claude/projects/-work");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{ID}.jsonl"));
    std::fs::write(
        &path,
        format!(
            "{}\n",
            json!({
                "type": "user", "uuid": "u-1", "timestamp": "2026-10-15T10:00:00Z",
                "cwd": "/work", "message": {"role": "user", "content": "Hello"}
            })
        ),
    )
    .unwrap();
    support::make_private(&path);

    let broken_roster = fx.run(&["group", "list", "--json"]);
    assert_eq!(broken_roster.status.code(), Some(1));
    let error: serde_json::Value = serde_json::from_slice(&broken_roster.stderr).unwrap();
    assert_eq!(error["code"], "store_corrupt");

    let shown = fx.run(&["transcript", "show", ID, "--json"]);
    assert_eq!(shown.status.code(), Some(0), "{}", text(&shown.stderr));
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["session"], ID);
    assert_eq!(shown["user_first_turns"].as_array().unwrap().len(), 1);
}

#[test]
fn cataloged_transcript_alias_round_trips_and_json_keeps_provider_id() {
    const ID: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
    let fx = Fixture::new();
    fx.seed();
    let project = fx.home().join(".claude/projects/-work");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{ID}.jsonl"));
    std::fs::write(
        &path,
        format!(
            "{}\n",
            json!({
                "type": "user", "uuid": "u-1", "timestamp": "2026-10-15T10:00:00Z",
                "cwd": "/work", "message": {"role": "user", "content": "Hello"}
            })
        ),
    )
    .unwrap();
    support::make_private(&path);
    let plain = text(&fx.ok(&["transcript", "show", ID]).stdout);
    assert!(plain.starts_with("s-3\t"), "{plain}");
    let shown: serde_json::Value =
        serde_json::from_slice(&fx.ok(&["transcript", "show", "s-3", "--json"]).stdout).unwrap();
    assert_eq!(shown["session"], ID);
}
