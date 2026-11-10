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
            "daniel",
            "agent note ghp_abcdefghijklmnop",
            "--json",
        ])
        .stdout,
    );
    assert_eq!(
        added["author"],
        serde_json::json!({"kind":"agent","name":"daniel"})
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
            "daniel",
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
            "astra",
            "--reason",
            "better explanation",
            "--json",
        ])
        .stdout,
    );
    assert_eq!(updated["reason"], "better explanation");
    assert_eq!(updated["author"]["name"], "opus");
    assert_eq!(updated["updated_by"]["name"], "astra");
    let args = [
        "proposal",
        "settle",
        "p-1",
        "--approve",
        "--author",
        "daniel",
        "--reason",
        "accepted",
        "--json",
    ];
    let settled = json(&fx.ok(&args).stdout);
    assert_eq!(
        settled["settled_author"],
        serde_json::json!({"kind":"agent","name":"daniel"})
    );
    assert_eq!(settled["settled_by"], serde_json::Value::Null);
    assert_eq!(settled["updated_by"]["name"], "daniel");
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
    assert_eq!(tags["tags"][0]["source"], "store");
    assert_eq!(tags["tags"][1]["source"], "config");
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
    fx.ok(&["proposal", "settle", "p-1", "--reject", "--author", "opus"]);
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
