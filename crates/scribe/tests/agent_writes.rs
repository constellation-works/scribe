//! Direct agent writes through the built binary, isolated from host state.
#![allow(clippy::expect_used, clippy::unwrap_used)]
mod support;
use support::{Fixture, text};
fn json(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).unwrap()
}

#[test]
fn agent_entry_verbs_record_authors_redact_and_audit_only_changes() {
    let fx = Fixture::new();
    let added = json(
        &fx.ok(&[
            "entry",
            "add",
            "--tag",
            "dec",
            "--author",
            "opus",
            "agent note ghp_abcdefghijklmnop",
            "--json",
        ])
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
        "decisions-1",
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
    let updated = json(&fx.ok(&update).stdout);
    assert_eq!(updated["author"], added["author"]);
    assert_eq!(updated["updated_by"]["name"], "opus");
    let audit = std::fs::read(fx.root().join("audit.jsonl")).unwrap();
    let noop = fx.ok(&update);
    assert_eq!(json(&noop.stdout), updated);
    assert_eq!(text(&noop.stderr).trim(), "nothing changed");
    assert_eq!(std::fs::read(fx.root().join("audit.jsonl")).unwrap(), audit);
    let retired = json(
        &fx.ok(&[
            "entry",
            "retire",
            "decisions-1",
            "--author",
            "opus",
            "--reason",
            "obsolete",
            "--json",
        ])
        .stdout,
    );
    assert_eq!(retired["status"], "retired");
    assert_eq!(retired["retirement_reason"], "obsolete");
    let lines = std::fs::read_to_string(fx.root().join("audit.jsonl")).unwrap();
    assert_eq!(lines.lines().count(), 3);
    assert!(!lines.contains("obsolete"));
    assert!(!lines.contains("new content"));
}

#[test]
fn daniel_guards_preserve_records_but_allow_metadata_and_open_proposals() {
    let fx = Fixture::new();
    fx.seed();
    let config = fx.root().join("config.toml");
    let text = std::fs::read_to_string(&config)
        .unwrap()
        .replace("env = [\"GITHUB_TOKEN\"]", "env = []");
    std::fs::write(config, text).unwrap();
    let before = std::fs::read(fx.root().join("entries/decisions/decisions-2.md")).unwrap();
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
        let out = fx.run(&args);
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(json(&out.stderr)["code"], "daniel_only");
        assert_eq!(
            std::fs::read(fx.root().join("entries/decisions/decisions-2.md")).unwrap(),
            before
        );
    }
    let p = json(
        &fx.ok(&[
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
        ])
        .stdout,
    );
    let id = p["id"].as_str().unwrap();
    for action in ["--approve", "--reject"] {
        let before = std::fs::read(fx.root().join("proposals.json")).unwrap();
        let out = fx.run(&[
            "proposal", "settle", id, action, "--author", "opus", "--json",
        ]);
        assert_eq!(json(&out.stderr)["code"], "daniel_only");
        assert_eq!(
            std::fs::read(fx.root().join("proposals.json")).unwrap(),
            before
        );
    }
    let updated = json(
        &fx.ok(&[
            "entry",
            "update",
            "decisions-2",
            "--author",
            "opus",
            "--add-topic",
            "api",
            "--json",
        ])
        .stdout,
    );
    assert_eq!(updated["author"]["kind"], "daniel");
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
        fx.ok(&[
            "entry",
            "add",
            "--tag",
            "decisions",
            "--author",
            "opus",
            content,
        ]);
    }
    let p = json(
        &fx.ok(&[
            "proposal",
            "add",
            "--kind",
            "supersedes",
            "--older",
            "decisions-1",
            "--newer",
            "decisions-2",
            "--reason",
            "replacement",
            "--author",
            "opus",
            "--json",
        ])
        .stdout,
    );
    assert_eq!(p["author"]["kind"], "agent");
    let updated = json(
        &fx.ok(&[
            "proposal",
            "update",
            "p-1",
            "--author",
            "opus",
            "--reason",
            "better explanation",
            "--json",
        ])
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
    let settled = json(&fx.ok(&args).stdout);
    assert_eq!(
        settled["settled_author"],
        serde_json::json!({"kind":"agent","name":"sonnet"})
    );
    assert_eq!(settled["settled_by"], serde_json::Value::Null);
    assert_eq!(settled["updated_by"]["name"], "sonnet");
    let entry = json(&fx.ok(&["entry", "show", "decisions-1", "--json"]).stdout);
    assert_eq!(entry["status"], "superseded");
    assert_eq!(entry["superseded_by"], "decisions-2");
    let before = support::snapshot(&fx.root())
        .into_iter()
        .filter(|(p, _, _)| !p.ends_with(".lock"))
        .collect::<Vec<_>>();
    let replay = fx.ok(&args);
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
        &fx.ok(&[
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
        ])
        .stdout,
    );
    assert_eq!(tag["author"]["kind"], "agent");
    assert!(fx.root().join("tags.json").is_file());
    fx.ok(&[
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
    ]);
    let added = json(
        &fx.ok(&[
            "entry",
            "add",
            "--tag",
            "memo",
            "--author",
            "opus",
            "remember this",
            "--json",
        ])
        .stdout,
    );
    assert_eq!(added["tag"], "notes");
    for name in ["decisions", "pref", "note"] {
        let out = fx.run(&[
            "tag",
            "add",
            name,
            "--author",
            "opus",
            "--description",
            "conflict",
            "--json",
        ]);
        assert_eq!(json(&out.stderr)["code"], "tag_conflict");
    }
    let out = fx.run(&[
        "tag",
        "update",
        "preferences",
        "--author",
        "opus",
        "--description",
        "no",
        "--json",
    ]);
    assert_eq!(json(&out.stderr)["code"], "tag_configured");
    let config = fx.root().join("config.toml");
    std::fs::write(&config, "[tags.other]\naliases = [\"memo\"]\n").unwrap();
    support::make_private(&config);
    let before = support::snapshot(&fx.root());
    let out = fx.ok(&["tag", "list", "--json"]);
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
        vec!["entry", "add", "--tag", "decisions", "content"],
        vec!["entry", "update", "decisions-1", "--content", "new"],
        vec!["entry", "retire", "decisions-1", "--reason", "old"],
        vec!["tag", "add", "notes", "--description", "notes"],
        vec!["tag", "update", "notes", "--description", "new"],
        vec![
            "proposal",
            "add",
            "--kind",
            "stale",
            "--older",
            "decisions-1",
            "--reason",
            "old",
        ],
        vec!["proposal", "update", "p-1", "--reason", "new"],
        vec!["proposal", "settle", "p-1", "--approve"],
    ] {
        assert_eq!(fx.run(&args).status.code(), Some(2), "{args:?}");
    }
    assert_eq!(
        json(&fx.run(&["entry", "show", "7", "--json"]).stderr)["code"],
        "usage_error"
    );
    assert!(!fx.root().exists());
    for content in ["one", "two"] {
        fx.ok(&[
            "entry",
            "add",
            "--tag",
            "decisions",
            "--author",
            "opus",
            content,
        ]);
    }
    let out = fx.run(&[
        "entry",
        "update",
        "deci",
        "--author",
        "opus",
        "--content",
        "new",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out.stderr)["code"], "id_ambiguous");
}

#[test]
fn interrupted_agent_approval_reapplies_with_its_author() {
    let fx = Fixture::new();
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "decisions",
        "--author",
        "opus",
        "old note",
    ]);
    let path = fx.root().join("entries/decisions/decisions-1.md");
    let before = std::fs::read(&path).unwrap();
    fx.ok(&[
        "proposal",
        "add",
        "--kind",
        "stale",
        "--older",
        "decisions-1",
        "--author",
        "opus",
        "--reason",
        "expired",
    ]);
    fx.ok(&[
        "proposal",
        "settle",
        "p-1",
        "--approve",
        "--author",
        "astra",
    ]);
    // Deterministic crash boundary: proposal committed, entry still old.
    std::fs::write(&path, before).unwrap();
    let store = scribe_core::Store::open(fx.root());
    let reapplied = store
        .reapply_approved(time::OffsetDateTime::now_utc())
        .unwrap();
    assert_eq!(reapplied.len(), 1);
    let entry = json(&fx.ok(&["entry", "show", "decisions-1", "--json"]).stdout);
    assert_eq!(entry["status"], "retired");
    assert_eq!(entry["updated_by"]["name"], "astra");
    assert!(
        store
            .reapply_approved(time::OffsetDateTime::now_utc())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn mismatched_provenance_and_newer_tag_documents_fail_closed_without_writes() {
    let fx = Fixture::new();
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "decisions",
        "--author",
        "opus",
        "agent note",
    ]);
    let path = fx.root().join("entries/decisions/decisions-1.md");
    let original = std::fs::read_to_string(&path).unwrap();
    // A null cite cannot masquerade as Daniel's typed words.
    std::fs::write(&path, original.replace("kind: agent", "kind: daniel")).unwrap();
    let before = support::snapshot(&fx.root());
    let out = fx.run(&["entry", "show", "decisions-1", "--json"]);
    assert_eq!(json(&out.stderr)["code"], "store_corrupt");
    assert_eq!(support::snapshot(&fx.root()), before);
    std::fs::write(&path, original).unwrap();
    fx.ok(&[
        "tag",
        "add",
        "notes",
        "--author",
        "opus",
        "--description",
        "notes",
    ]);
    let path = fx.root().join("tags.json");
    let mut tags: serde_json::Value = json(&std::fs::read(&path).unwrap());
    tags["format"] = serde_json::json!(999);
    std::fs::write(&path, serde_json::to_vec(&tags).unwrap()).unwrap();
    let before = support::snapshot(&fx.root());
    let out = fx.run(&["tag", "list", "--json"]);
    assert_eq!(json(&out.stderr)["code"], "store_too_new");
    assert_eq!(support::snapshot(&fx.root()), before);
    let before = std::fs::read(&path).unwrap();
    let out = fx.run(&[
        "tag",
        "update",
        "notes",
        "--author",
        "opus",
        "--description",
        "replacement",
        "--json",
    ]);
    assert_eq!(json(&out.stderr)["code"], "store_too_new");
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn invalid_or_settled_proposals_never_silently_drop_changed_inputs() {
    let fx = Fixture::new();
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "decisions",
        "--author",
        "opus",
        "note",
    ]);
    let out = fx.run(&[
        "proposal",
        "add",
        "--kind",
        "supersedes",
        "--older",
        "decisions-1",
        "--author",
        "opus",
        "--reason",
        "replacement",
        "--json",
    ]);
    assert_eq!(json(&out.stderr)["code"], "proposal_invalid");
    assert!(!fx.root().join("proposals.json").exists());
    fx.ok(&[
        "proposal",
        "add",
        "--kind",
        "stale",
        "--older",
        "decisions-1",
        "--author",
        "opus",
        "--reason",
        "review",
    ]);
    fx.ok(&["proposal", "settle", "p-1", "--reject", "--author", "astra"]);
    let before = std::fs::read(fx.root().join("proposals.json")).unwrap();
    for args in [
        vec![
            "proposal", "update", "p-1", "--author", "opus", "--reason", "new", "--json",
        ],
        vec![
            "proposal", "settle", "p-1", "--reject", "--author", "opus", "--reason", "new",
            "--json",
        ],
    ] {
        let out = fx.run(&args);
        assert_eq!(json(&out.stderr)["code"], "proposal_settled");
        assert_eq!(
            std::fs::read(fx.root().join("proposals.json")).unwrap(),
            before
        );
    }
    let entry = json(&fx.ok(&["entry", "show", "decisions-1", "--json"]).stdout);
    assert_eq!(entry["status"], "active");
    let log = std::fs::read_to_string(fx.root().join("audit.jsonl")).unwrap();
    assert_eq!(
        log.lines().count(),
        6,
        "one line for each change or refusal"
    );
}

#[test]
fn corrupt_entries_still_count_when_resolving_prefixes() {
    let fx = Fixture::new();
    for content in ["one", "two"] {
        fx.ok(&[
            "entry",
            "add",
            "--tag",
            "decisions",
            "--author",
            "opus",
            content,
        ]);
    }
    let path = fx.root().join("entries/decisions/decisions-2.md");
    std::fs::write(&path, "not a record").unwrap();
    let out = fx.run(&[
        "entry",
        "update",
        "deci",
        "--author",
        "opus",
        "--content",
        "replacement",
        "--json",
    ]);
    assert_eq!(json(&out.stderr)["code"], "id_ambiguous");
    let out = fx.run(&["entry", "show", "decisions-2", "--json"]);
    assert_eq!(json(&out.stderr)["code"], "store_corrupt");
    let unchanged = json(&fx.ok(&["entry", "show", "decisions-1", "--json"]).stdout);
    assert_eq!(unchanged["content"], "one");
}

#[test]
fn refused_writes_audit_the_target_and_author_but_never_the_arguments() {
    let fx = Fixture::new();
    fx.seed();
    let config = fx.root().join("config.toml");
    let text = std::fs::read_to_string(&config)
        .unwrap()
        .replace("env = [\"GITHUB_TOKEN\"]", "env = []");
    std::fs::write(config, text).unwrap();
    let proposal = json(
        &fx.ok(&[
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
        ])
        .stdout,
    );
    let proposal_id = proposal["id"].as_str().unwrap().to_owned();
    // Each refusal names the record it was aimed at: the resolved target id and
    // the agent's own --author name, never the content, reason or other arguments.
    let refusals: [(Vec<&str>, &str, &str); 3] = [
        (
            vec![
                "entry",
                "update",
                "decisions-2",
                "--author",
                "opus",
                "--content",
                "rewritten secret text",
                "--json",
            ],
            "entry update",
            "decisions-2",
        ),
        (
            vec![
                "entry",
                "retire",
                "decisions-2",
                "--author",
                "opus",
                "--reason",
                "obsolete reason text",
                "--json",
            ],
            "entry retire",
            "decisions-2",
        ),
        (
            vec![
                "proposal",
                "settle",
                proposal_id.as_str(),
                "--approve",
                "--author",
                "opus",
                "--json",
            ],
            "proposal settle",
            proposal_id.as_str(),
        ),
    ];
    for (args, _, _) in &refusals {
        let out = fx.run(args);
        assert_eq!(json(&out.stderr)["code"], "daniel_only");
    }
    let audit = std::fs::read_to_string(fx.root().join("audit.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = audit
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let refused = &lines[lines.len() - refusals.len()..];
    for (line, (_, command, target)) in refused.iter().zip(&refusals) {
        assert_eq!(line["command"], *command);
        assert_eq!(line["status"], "failure");
        assert_eq!(line["error_code"], "daniel_only");
        assert_eq!(line["target"], *target);
        assert_eq!(line["author"], "opus");
    }
    assert!(!audit.contains("rewritten secret text"));
    assert!(!audit.contains("obsolete reason text"));
    assert!(!audit.contains("review needed"));
}

#[test]
fn a_session_id_that_resolves_to_no_record_leaves_the_audit_target_null() {
    // A refused rebuild names no record it could find: its line carries the
    // author but not the typed id, which resolved to nothing (the decision in
    // 4_decisions.md: an unresolved id leaves `target` null).
    let fx = Fixture::new();
    fx.seed();
    let out = fx.run(&[
        "session",
        "rebuild",
        "s-missing-1",
        "--author",
        "opus",
        "--confirm",
        "--json",
    ]);
    assert!(!out.status.success());
    let audit = std::fs::read_to_string(fx.root().join("audit.jsonl")).unwrap();
    let line: serde_json::Value = serde_json::from_str(audit.lines().last().unwrap()).unwrap();
    assert_eq!(line["command"], "session rebuild");
    assert_eq!(line["status"], "failure");
    assert_eq!(line["target"], serde_json::Value::Null);
    assert_eq!(line["author"], "opus");
    assert!(!audit.contains("s-missing-1"));
}

#[test]
fn reserved_author_names_are_refused_on_every_agent_write() {
    let fx = Fixture::new();
    for name in ["daniel", "scribe", "consistency", "session-pass"] {
        for args in [
            vec![
                "entry",
                "add",
                "--tag",
                "decisions",
                "--author",
                name,
                "note",
                "--json",
            ],
            vec![
                "entry",
                "update",
                "decisions-1",
                "--author",
                name,
                "--content",
                "x",
                "--json",
            ],
            vec![
                "entry",
                "retire",
                "decisions-1",
                "--author",
                name,
                "--reason",
                "old",
                "--json",
            ],
            vec![
                "tag",
                "add",
                "notes",
                "--author",
                name,
                "--description",
                "notes",
                "--json",
            ],
            vec![
                "tag",
                "update",
                "notes",
                "--author",
                name,
                "--description",
                "new",
                "--json",
            ],
            vec![
                "proposal",
                "add",
                "--kind",
                "stale",
                "--older",
                "decisions-1",
                "--reason",
                "old",
                "--author",
                name,
                "--json",
            ],
            vec![
                "proposal", "update", "p-1", "--author", name, "--reason", "new", "--json",
            ],
            vec![
                "proposal",
                "settle",
                "p-1",
                "--approve",
                "--author",
                name,
                "--json",
            ],
            vec![
                "session",
                "catalog",
                "0a1b2c3d-0000-4000-8000-0000000000c1",
                "--author",
                name,
                "--json",
            ],
            vec![
                "session",
                "rebuild",
                "0a1b2c3d-0000-4000-8000-0000000000c1",
                "--author",
                name,
                "--confirm",
                "--json",
            ],
            vec![
                "session",
                "deactivate",
                "0a1b2c3d-0000-4000-8000-0000000000c1",
                "--author",
                name,
                "--reason",
                "gone",
                "--json",
            ],
            vec![
                "session",
                "activate",
                "0a1b2c3d-0000-4000-8000-0000000000c1",
                "--author",
                name,
                "--json",
            ],
        ] {
            let out = fx.run(&args);
            assert_eq!(
                json(&out.stderr)["code"],
                "reserved_author",
                "{args:?}: {}",
                text(&out.stderr)
            );
        }
    }
    // Reads still accept the name as a filter.
    fx.ok(&["entry", "list", "--author", "daniel", "--json"]);
    assert!(!fx.root().join("entries").exists());
    assert!(!fx.root().join("control.json").exists());
}

#[test]
fn agent_names_are_attribution_so_any_agent_may_tidy_another_agents_work() {
    let fx = Fixture::new();
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "decisions",
        "--author",
        "opus",
        "opus note",
    ]);
    // Another agent may rewrite and retag an agent note; the record keeps
    // its author and who changed it.
    let updated = json(
        &fx.ok(&[
            "entry",
            "update",
            "decisions-1",
            "--author",
            "sonnet",
            "--content",
            "rewritten",
            "--add-topic",
            "api",
            "--json",
        ])
        .stdout,
    );
    assert_eq!(updated["content"], "rewritten");
    assert_eq!(updated["author"]["name"], "opus");
    assert_eq!(updated["updated_by"]["name"], "sonnet");
    let retired = json(
        &fx.ok(&[
            "entry",
            "retire",
            "decisions-1",
            "--author",
            "sonnet",
            "--reason",
            "done",
            "--json",
        ])
        .stdout,
    );
    assert_eq!(retired["status"], "retired");
    assert_eq!(retired["author"]["name"], "opus");
    // A retired note still refuses further updates, from anyone.
    let out = fx.run(&[
        "entry",
        "update",
        "decisions-1",
        "--author",
        "opus",
        "--content",
        "back",
        "--json",
    ]);
    assert_eq!(json(&out.stderr)["code"], "entry_not_active");

    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "decisions",
        "--author",
        "opus",
        "second note",
    ]);
    fx.ok(&[
        "proposal",
        "add",
        "--kind",
        "stale",
        "--older",
        "decisions-2",
        "--reason",
        "review",
        "--author",
        "opus",
    ]);
    // Another agent may revise an agent proposal's reason.
    let revised = json(
        &fx.ok(&[
            "proposal", "update", "p-1", "--author", "sonnet", "--reason", "revised", "--json",
        ])
        .stdout,
    );
    assert_eq!(revised["reason"], "revised");
    assert_eq!(revised["author"]["name"], "opus");
    assert_eq!(revised["updated_by"]["name"], "sonnet");
    // The proposer may settle its own proposal; the settler is recorded.
    let settled = json(
        &fx.ok(&[
            "proposal", "settle", "p-1", "--reject", "--author", "opus", "--json",
        ])
        .stdout,
    );
    assert_eq!(settled["state"], "rejected");
    assert_eq!(settled["reason"], "revised");
    assert_eq!(settled["author"]["name"], "opus");
    assert_eq!(settled["settled_author"]["name"], "opus");
}

/// `session catalog --author daniel` under a pseudo-terminal, as
/// `script` fakes one for any caller: refused like through a pipe.
#[cfg(unix)]
#[test]
fn a_faked_terminal_cannot_catalog_as_daniel() {
    let fx = Fixture::new();
    let id = "0a1b2c3d-0000-4000-8000-0000000000c1";
    let project = fx.home().join(".claude/projects/-work");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join(format!("{id}.jsonl")),
        format!(
            "{}\n",
            serde_json::json!({"type": "user", "uuid": "u-1",
                "timestamp": "2026-01-01T00:00:00Z", "cwd": "/nonexistent/work",
                "message": {"role": "user", "content": "an old session"}})
        ),
    )
    .unwrap();
    let command = format!(
        "{} session catalog {id} --author daniel --json",
        support::BIN
    );
    let mut script = std::process::Command::new("script");
    if cfg!(target_os = "macos") {
        script.args(["-q", "/dev/null", "/bin/sh", "-c", &command]);
    } else {
        script.args(["-q", "-e", "-c", &command, "/dev/null"]);
    }
    let out = script
        .env_clear()
        .env("HOME", fx.home())
        .env("SCRIBE_ROOT", fx.root())
        .env("PATH", "/usr/bin:/bin")
        .env("SHELL", "/bin/sh")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run script(1)");
    let shown = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(shown.contains("reserved_author"), "{shown}");
    assert!(!fx.root().join("control.json").exists());
    assert!(!fx.root().join("sessions").exists());
}

#[test]
fn settling_a_scribe_proposal_cannot_replace_its_reason() {
    let fx = Fixture::new();
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "decisions",
        "--author",
        "opus",
        "opus note",
    ]);
    fx.ok(&[
        "proposal",
        "add",
        "--kind",
        "stale",
        "--older",
        "decisions-1",
        "--reason",
        "routine reason",
        "--author",
        "opus",
    ]);
    // Make it a routine-opened (scribe) proposal, as `consistency scan` writes.
    let path = fx.root().join("proposals.json");
    let mut doc: serde_json::Value = json(&std::fs::read(&path).unwrap());
    doc["proposals"][0]["author"] = serde_json::json!({"kind": "scribe", "name": "consistency"});
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    support::make_private(&path);

    let out = fx.run(&[
        "proposal",
        "settle",
        "p-1",
        "--reject",
        "--author",
        "sonnet",
        "--reason",
        "rewritten by agent",
        "--json",
    ]);
    assert_eq!(json(&out.stderr)["code"], "proposal_not_agent");
    let list = json(&fx.ok(&["proposal", "list", "--json"]).stdout);
    assert_eq!(list["proposals"][0]["state"], "open");
    assert_eq!(list["proposals"][0]["reason"], "routine reason");

    // Settling without a new reason keeps the routine's reason.
    let settled = json(
        &fx.ok(&[
            "proposal", "settle", "p-1", "--reject", "--author", "sonnet", "--json",
        ])
        .stdout,
    );
    assert_eq!(settled["state"], "rejected");
    assert_eq!(settled["reason"], "routine reason");
    assert_eq!(settled["settled_author"]["name"], "sonnet");
}

#[test]
fn a_finding_without_evidence_is_refused_and_writes_nothing() {
    let fx = Fixture::new();
    let out = fx.run(&[
        "entry",
        "add",
        "--tag",
        "finding",
        "--author",
        "opus",
        "The reviewer mix alone predicts a 7-point gap",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let err = json(&out.stderr);
    assert_eq!(err["code"], "evidence_required", "{err}");
    assert!(
        text(&out.stderr).contains("--evidence"),
        "{}",
        text(&out.stderr)
    );
    let list = json(
        &fx.ok(&["entry", "list", "--tag", "finding", "--json"])
            .stdout,
    );
    assert_eq!(list["total"], 0, "a refused finding writes nothing");

    // Evidence belongs to a finding only.
    let out = fx.run(&[
        "entry",
        "add",
        "--tag",
        "decisions",
        "--author",
        "opus",
        "--evidence",
        "exp-1",
        "Use the stable API",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json(&out.stderr)["code"], "evidence_required");
}

#[test]
fn a_finding_records_its_pointer_as_of_date_and_agent_author() {
    let fx = Fixture::new();
    let added = json(
        &fx.ok(&[
            "entry",
            "add",
            "--tag",
            "finding",
            "--author",
            "opus",
            "--related",
            "session:s-9",
            "--evidence",
            "exp-5342a4d6",
            "--as-of",
            "2026-11-14",
            "The reviewer mix alone predicts a 7-point gap (51% against 58%)",
            "--json",
        ])
        .stdout,
    );
    assert_eq!(added["id"], "finding-1");
    assert_eq!(
        added["author"],
        serde_json::json!({"kind":"agent","name":"opus"})
    );
    assert_eq!(added["evidence"], "exp-5342a4d6");
    assert_eq!(added["as_of"], "2026-11-14");
    assert_eq!(added["action"], serde_json::Value::Null, "no worker");

    // Search returns it as that agent's claim as of its date, with the
    // pointer — never as Daniel's words.
    let found = json(
        &fx.ok(&["record", "search", "reviewer mix predicts gap", "--json"])
            .stdout,
    );
    let hit = &found["hits"][0];
    assert_eq!(hit["id"], "finding-1", "{found}");
    assert_eq!(hit["author"]["kind"], "agent");
    assert_eq!(hit["author"]["name"], "opus");
    assert_eq!(hit["evidence"], "exp-5342a4d6");
    assert_eq!(hit["as_of"], "2026-11-14");
    let snippet = hit["snippet"].as_str().unwrap();
    assert!(
        snippet.contains("agent claim (opus) as of 2026-11-14; evidence: exp-5342a4d6"),
        "{snippet}"
    );
}

#[test]
fn the_fourth_finding_of_a_session_is_refused_and_names_the_cap() {
    let fx = Fixture::new();
    let add = |session: &str, n: usize| {
        fx.run(&[
            "entry",
            "add",
            "--tag",
            "finding",
            "--author",
            "opus",
            "--related",
            &format!("session:{session}"),
            "--evidence",
            "exp-1",
            &format!("Measured result number {n} for the widget cache"),
            "--json",
        ])
    };
    for n in 1..=3 {
        let out = add("s-cap", n);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let out = add("s-cap", 4);
    assert_eq!(out.status.code(), Some(1));
    let err = json(&out.stderr);
    assert_eq!(err["code"], "finding_cap", "{err}");
    assert!(
        text(&out.stderr).contains("at most 3 findings per session"),
        "{}",
        text(&out.stderr)
    );
    // Another session still has its own budget.
    let out = add("s-other", 1);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let list = json(
        &fx.ok(&["entry", "list", "--tag", "finding", "--json"])
            .stdout,
    );
    assert_eq!(list["total"], 4);
}
