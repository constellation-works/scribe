//! Direct agent writes through the built binary, isolated from host state.
#![allow(clippy::expect_used, clippy::unwrap_used)]
#[path = "agent_writes/records.rs"]
mod records;
#[path = "support/agent_writes.rs"]
mod subject;
mod support;

use crate::support::command::Exit::Code;
use crate::support::command::Input::Cli;
use scribe_core::ProposalVerbs;
use subject::*;
use support::command::Exit::{Capture, Success};
use support::{Fixture, text};
fn json(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).unwrap()
}

#[test]
fn agent_entry_verbs_record_authors_redact_and_audit_only_changes() {
    let fx = Fixture::new();
    let added = json(
        &fx.check(
            &[
                "add",
                "--tag",
                "pref",
                "--author",
                "opus",
                "agent note ghp_abcdefghijklmnop",
                "--json",
            ],
            Success,
        )
        .stdout,
    );
    assert_eq!(
        added["author"],
        serde_json::json!({"kind":"agent","name":"opus"})
    );
    assert_eq!(added["cite"], serde_json::Value::Null);
    assert_eq!(added["said"], "");
    assert_eq!(added["content_from"]["kind"], "agent");
    assert!(!added["content"].as_str().unwrap().contains("ghp_"));
    let update = [
        "entry",
        "update",
        "preferences-1",
        "--author",
        "opus",
        "--content",
        "new content",
        "--add-topic",
        "api",
        "--add-related",
        "repo:scribe",
        "--json",
    ];
    let updated = json(&fx.check(Cli(&update), Success).stdout);
    assert_eq!(updated["author"], added["author"]);
    assert_eq!(updated["updated_by"]["name"], "opus");
    let audit = support::audit_text(&fx.root());
    let noop = fx.check(Cli(&update), Success);
    assert_eq!(json(&noop.stdout), updated);
    assert_eq!(text(&noop.stderr).trim(), "nothing changed");
    assert_eq!(support::audit_text(&fx.root()), audit);
    let retired = json(
        &fx.check(
            &[
                "retire",
                "preferences-1",
                "--author",
                "opus",
                "--reason",
                "obsolete",
                "--json",
            ],
            Success,
        )
        .stdout,
    );
    assert_eq!(retired["status"], "retired");
    assert_eq!(retired["retirement_reason"], "obsolete");
    let lines = support::audit_text(&fx.root());
    assert_eq!(lines.lines().count(), 3);
    assert!(!lines.contains("obsolete"));
    assert!(!lines.contains("new content"));
}

#[test]
fn entry_add_refuses_a_statement_an_active_entry_already_covers() {
    let fx = Fixture::new();
    fx.config(support::IDEA_CATCHER);
    let statement = "use rg in scripts today";
    let add = |tag: &str, project: Option<&str>, words: &str, as_json: bool| {
        let mut args = vec!["entry", "add", "--tag", tag, "--author", "opus"];
        if let Some(project) = project {
            args.extend(["--project", project]);
        }
        if as_json {
            args.push("--json");
        }
        args.push(words);
        fx.check(Cli(&args), Capture)
    };
    let added = json(&add("pref", None, statement, true).stdout);
    assert_eq!(added["id"], "preferences-1");

    let plain = add("preferences", None, statement, false);
    assert_eq!(plain.status.code(), Some(1));
    assert_eq!(
        text(&plain.stderr).trim(),
        "error: already covered by preferences-1; nothing written"
    );
    let json_out = fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            statement,
            "--json",
        ],
        Code(1),
    );
    let err = json(&json_out.stderr);
    assert_eq!(err["code"], "already_covered");
    assert_eq!(
        err["error"],
        "already covered by preferences-1; nothing written"
    );
    let audit = support::audit_text(&fx.root());
    let refusal = audit.lines().last().unwrap();
    assert!(refusal.contains("already_covered"), "{refusal}");
    assert!(
        !refusal.contains(statement),
        "the audit line carries the code, not the statement: {refusal}"
    );
    assert_eq!(
        json(&fx.check(&["list", "--json"], Success).stdout)["total"],
        1
    );

    assert_eq!(add("idea", None, statement, false).status.code(), Some(0));
    assert_eq!(
        add("preferences", Some("scribe"), statement, false)
            .status
            .code(),
        Some(0),
        "a different project is not the same entry"
    );

    fx.check(
        &[
            "retire",
            "preferences-1",
            "--author",
            "opus",
            "--reason",
            "replaced",
        ],
        Success,
    );
    let rewritten = json(&add("preferences", None, statement, true).stdout);
    assert_eq!(rewritten["id"], "preferences-3");
    let newer = json(&add("preferences", None, "keep merge commits for releases", true).stdout);
    assert_eq!(newer["id"], "preferences-4");
    fx.check(
        Cli(&[
            "proposal",
            "add",
            "--kind",
            "supersedes",
            "--older",
            "preferences-3",
            "--newer",
            "preferences-4",
            "--reason",
            "replacement",
            "--author",
            "opus",
        ]),
        Success,
    );
    fx.check(
        Cli(&["proposal", "settle", "p-1", "--approve", "--author", "opus"]),
        Success,
    );
    let after = json(&add("preferences", None, statement, true).stdout);
    assert_eq!(after["id"], "preferences-5");
    assert_eq!(
        json(
            &fx.check(&["show", "preferences-3", "--json"], Success)
                .stdout
        )["status"],
        "superseded"
    );
}

#[test]
fn daniel_guards_preserve_records_but_allow_metadata_and_open_proposals() {
    let fx = Fixture::seeded();
    let config = fx.root().join("config.toml");
    let text = std::fs::read_to_string(&config)
        .unwrap()
        .replace("env = [\"GITHUB_TOKEN\"]", "env = []");
    std::fs::write(config, text).unwrap();
    let before = support::exported(&fx.root(), "entries/decisions/decisions-2.md").unwrap();
    for args in [
        vec![
            "entry",
            "update",
            "decisions-2",
            "--content",
            "rewritten",
            "--author",
            "opus",
            "--json",
        ],
        vec![
            "entry",
            "retire",
            "decisions-2",
            "--reason",
            "obsolete",
            "--author",
            "opus",
            "--json",
        ],
    ] {
        let out = fx.check(Cli(&args), Code(1));
        assert_eq!(json(&out.stderr)["code"], "user_only");
        assert_eq!(
            support::exported(&fx.root(), "entries/decisions/decisions-2.md").unwrap(),
            before
        );
    }
    let p = json(
        &fx.check(
            Cli(&[
                "proposal",
                "add",
                "--kind",
                "stale",
                "--older",
                "decisions-2",
                "--reason",
                "review needed",
                "--author",
                "opus",
                "--json",
            ]),
            Success,
        )
        .stdout,
    );
    let id = p["id"].as_str().unwrap();
    for action in ["--approve", "--reject"] {
        let before = support::exported(&fx.root(), "proposals.json").unwrap();
        let out = fx.check(
            Cli(&[
                "proposal", "settle", id, action, "--author", "opus", "--json",
            ]),
            Capture,
        );
        assert_eq!(json(&out.stderr)["code"], "user_only");
        assert_eq!(
            support::exported(&fx.root(), "proposals.json").unwrap(),
            before
        );
    }
    let updated = json(
        &fx.check(
            &[
                "update",
                "decisions-2",
                "--author",
                "opus",
                "--add-topic",
                "api",
                "--json",
            ],
            Success,
        )
        .stdout,
    );
    assert_eq!(updated["author"]["kind"], "user");
    assert_eq!(updated["updated_by"]["kind"], "agent");
    assert!(
        updated["topics"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("api"))
    );
}

#[test]
fn proposals_update_settle_and_replay_with_agent_authorship() {
    let fx = Fixture::new();
    for content in ["old note", "new note"] {
        fx.check(
            &["add", "--tag", "preferences", "--author", "opus", content],
            Success,
        );
    }
    let p = json(
        &fx.check(
            Cli(&[
                "proposal",
                "add",
                "--kind",
                "supersedes",
                "--older",
                "preferences-1",
                "--newer",
                "preferences-2",
                "--reason",
                "replacement",
                "--author",
                "opus",
                "--json",
            ]),
            Success,
        )
        .stdout,
    );
    assert_eq!(p["author"]["kind"], "agent");
    let updated = json(
        &fx.check(
            Cli(&[
                "proposal",
                "update",
                "p-1",
                "--author",
                "opus",
                "--reason",
                "better explanation",
                "--json",
            ]),
            Success,
        )
        .stdout,
    );
    assert_eq!(updated["reason"], "better explanation");
    assert_eq!(updated["author"]["name"], "opus");
    assert_eq!(updated["updated_by"]["name"], "opus");
    let args = [
        "proposal",
        "settle",
        "p-1",
        "--approve",
        "--author",
        "sonnet",
        "--reason",
        "accepted",
        "--json",
    ];
    let settled = json(&fx.check(Cli(&args), Success).stdout);
    assert_eq!(
        settled["settled_author"],
        serde_json::json!({"kind":"agent","name":"sonnet"})
    );
    assert_eq!(settled["settled_by"], serde_json::Value::Null);
    assert_eq!(settled["updated_by"]["name"], "sonnet");
    let entry = json(
        &fx.check(&["show", "preferences-1", "--json"], Success)
            .stdout,
    );
    assert_eq!(entry["status"], "superseded");
    assert_eq!(entry["superseded_by"], "preferences-2");
    let before = support::snapshot(&fx.root())
        .into_iter()
        .filter(|(p, _, _)| !p.ends_with(".lock"))
        .collect::<Vec<_>>();
    let replay = fx.check(Cli(&args), Success);
    assert_eq!(json(&replay.stdout), settled);
    assert_eq!(
        support::snapshot(&fx.root())
            .into_iter()
            .filter(|(p, _, _)| !p.ends_with(".lock"))
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn store_tags_merge_and_config_wins_without_writing_on_reads() {
    let fx = Fixture::new();
    let tag = json(
        &fx.check(
            Cli(&[
                "tag",
                "add",
                "notes",
                "--author",
                "opus",
                "--description",
                "Agent notes",
                "--alias",
                "note",
                "--json",
            ]),
            Success,
        )
        .stdout,
    );
    assert_eq!(tag["author"]["kind"], "agent");
    assert!(
        support::exported(&fx.root(), "tags.json")
            .unwrap()
            .contains("notes")
    );
    fx.check(
        Cli(&[
            "tag",
            "update",
            "notes",
            "--author",
            "astra",
            "--description",
            "Updated notes",
            "--add-alias",
            "memo",
            "--stale-after-days",
            "30",
        ]),
        Success,
    );
    let added = json(
        &fx.check(
            &[
                "add",
                "--tag",
                "memo",
                "--author",
                "opus",
                "remember this",
                "--json",
            ],
            Success,
        )
        .stdout,
    );
    assert_eq!(added["tag"], "notes");
    for name in ["preferences", "pref", "note"] {
        let out = fx.check(
            Cli(&[
                "tag",
                "add",
                name,
                "--author",
                "opus",
                "--description",
                "conflict",
                "--json",
            ]),
            Capture,
        );
        assert_eq!(json(&out.stderr)["code"], "tag_conflict");
    }
    let out = fx.check(
        Cli(&[
            "tag",
            "update",
            "preferences",
            "--author",
            "opus",
            "--description",
            "no",
            "--json",
        ]),
        Capture,
    );
    assert_eq!(json(&out.stderr)["code"], "tag_configured");
    let config = fx.root().join("config.toml");
    std::fs::write(&config, "[tags.other]\naliases = [\"memo\"]\n").unwrap();
    support::make_private(&config);
    let before = support::snapshot(&fx.root());
    let out = fx.check(Cli(&["tag", "list", "--json"]), Success);
    assert!(text(&out.stderr).contains("shadowed"));
    let tags = json(&out.stdout);
    let source = |name: &str| {
        tags["tags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .map(|t| t["source"].clone())
            .unwrap()
    };
    assert_eq!(source("notes"), "store");
    assert_eq!(source("other"), "config");
    assert_eq!(support::snapshot(&fx.root()), before);
}

#[test]
fn all_agent_verbs_require_author_and_ambiguous_prefixes_exit_two() {
    let fx = Fixture::new();
    for args in [
        vec!["entry", "add", "--tag", "preferences", "content"],
        vec!["entry", "update", "preferences-1", "--content", "new"],
        vec!["entry", "retire", "preferences-1", "--reason", "old"],
        vec!["tag", "add", "notes", "--description", "notes"],
        vec!["tag", "update", "notes", "--description", "new"],
        vec![
            "proposal",
            "add",
            "--kind",
            "stale",
            "--older",
            "preferences-1",
            "--reason",
            "old",
        ],
        vec!["proposal", "update", "p-1", "--reason", "new"],
        vec!["proposal", "settle", "p-1", "--approve"],
    ] {
        assert_eq!(
            fx.check(Cli(&args), Capture).status.code(),
            Some(2),
            "{args:?}"
        );
    }
    assert_eq!(
        json(&fx.check(&["show", "7", "--json"], Capture).stderr)["code"],
        "usage_error"
    );
    assert!(!fx.root().exists());
    for content in ["one", "two"] {
        fx.check(
            &["add", "--tag", "preferences", "--author", "opus", content],
            Success,
        );
    }
    let out = fx.check(
        &[
            "update",
            "prefe",
            "--author",
            "opus",
            "--content",
            "new",
            "--json",
        ],
        Code(2),
    );
    assert_eq!(json(&out.stderr)["code"], "id_ambiguous");
}

include!("agent_writes/proposals.rs");
include!("agent_writes/authorship_and_findings.rs");
