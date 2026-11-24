//! Direct agent writes through the built binary, isolated from host state.
#![allow(clippy::expect_used, clippy::unwrap_used)]
#[path = "agent_writes/records.rs"]
mod records;
mod support;
use scribe_core::ProposalVerbs;
use support::{Fixture, text};
/// Files (relative to `root`) whose bytes or mode differ between two snapshots.
fn changed(
    before: &[(std::path::PathBuf, Vec<u8>, u32)],
    after: &[(std::path::PathBuf, Vec<u8>, u32)],
) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for (p, b, m) in before {
        if !after.iter().any(|(q, c, n)| q == p && c == b && n == m) {
            out.push(p.clone());
        }
    }
    for (p, _, _) in after {
        if !before.iter().any(|(q, _, _)| q == p) {
            out.push(p.clone());
        }
    }
    out
}
/// Every exported record document, for "no record changed" checks that
/// tolerate scribe.db and its WAL moving bytes.
fn records(root: &std::path::Path) -> Vec<Option<String>> {
    [
        "entries/preferences/preferences-1.md",
        "proposals.json",
        "tags.json",
        "marks.json",
        "pickups.json",
    ]
    .iter()
    .map(|rel| support::exported(root, rel))
    .collect()
}
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
            "pref",
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
    let updated = json(&fx.ok(&update).stdout);
    assert_eq!(updated["author"], added["author"]);
    assert_eq!(updated["updated_by"]["name"], "opus");
    let audit = support::audit_text(&fx.root());
    let noop = fx.ok(&update);
    assert_eq!(json(&noop.stdout), updated);
    assert_eq!(text(&noop.stderr).trim(), "nothing changed");
    assert_eq!(support::audit_text(&fx.root()), audit);
    let retired = json(
        &fx.ok(&[
            "entry",
            "retire",
            "preferences-1",
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
        fx.run(&args)
    };
    let added = json(&add("pref", None, statement, true).stdout);
    assert_eq!(added["id"], "preferences-1");

    let plain = add("preferences", None, statement, false);
    assert_eq!(plain.status.code(), Some(1));
    assert_eq!(
        text(&plain.stderr).trim(),
        "error: already covered by preferences-1; nothing written"
    );
    let json_out = fx.run(&[
        "entry",
        "add",
        "--tag",
        "preferences",
        "--author",
        "opus",
        statement,
        "--json",
    ]);
    assert_eq!(json_out.status.code(), Some(1));
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
        json(&fx.ok(&["entry", "list", "--json"]).stdout)["total"],
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

    fx.ok(&[
        "entry",
        "retire",
        "preferences-1",
        "--author",
        "opus",
        "--reason",
        "replaced",
    ]);
    let rewritten = json(&add("preferences", None, statement, true).stdout);
    assert_eq!(rewritten["id"], "preferences-3");
    let newer = json(&add("preferences", None, "keep merge commits for releases", true).stdout);
    assert_eq!(newer["id"], "preferences-4");
    fx.ok(&[
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
    ]);
    fx.ok(&["proposal", "settle", "p-1", "--approve", "--author", "opus"]);
    let after = json(&add("preferences", None, statement, true).stdout);
    assert_eq!(after["id"], "preferences-5");
    assert_eq!(
        json(&fx.ok(&["entry", "show", "preferences-3", "--json"]).stdout)["status"],
        "superseded"
    );
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
        let out = fx.run(&args);
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(json(&out.stderr)["code"], "user_only");
        assert_eq!(
            support::exported(&fx.root(), "entries/decisions/decisions-2.md").unwrap(),
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
        let before = support::exported(&fx.root(), "proposals.json").unwrap();
        let out = fx.run(&[
            "proposal", "settle", id, action, "--author", "opus", "--json",
        ]);
        assert_eq!(json(&out.stderr)["code"], "user_only");
        assert_eq!(
            support::exported(&fx.root(), "proposals.json").unwrap(),
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
        fx.ok(&[
            "entry",
            "add",
            "--tag",
            "preferences",
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
            "preferences-1",
            "--newer",
            "preferences-2",
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
    let entry = json(&fx.ok(&["entry", "show", "preferences-1", "--json"]).stdout);
    assert_eq!(entry["status"], "superseded");
    assert_eq!(entry["superseded_by"], "preferences-2");
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
    assert!(
        support::exported(&fx.root(), "tags.json")
            .unwrap()
            .contains("notes")
    );
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
    for name in ["preferences", "pref", "note"] {
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
            "preferences",
            "--author",
            "opus",
            content,
        ]);
    }
    let out = fx.run(&[
        "entry",
        "update",
        "prefe",
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
        "preferences",
        "--author",
        "opus",
        "old note",
    ]);
    fx.ok(&[
        "proposal",
        "add",
        "--kind",
        "stale",
        "--older",
        "preferences-1",
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
    support::sql(
        &fx.root(),
        "UPDATE entries SET status = 'active', retirement_reason = NULL, \
         inactive_since = NULL, updated_by = NULL WHERE id = 'preferences-1'",
    );
    let store = scribe_core::Store::open(fx.root());
    let reapplied = store
        .reapply_approved(time::OffsetDateTime::now_utc())
        .unwrap();
    assert_eq!(reapplied.applied.len(), 1);
    assert!(reapplied.covered.is_empty());
    let entry = json(&fx.ok(&["entry", "show", "preferences-1", "--json"]).stdout);
    assert_eq!(entry["status"], "retired");
    assert_eq!(entry["updated_by"]["name"], "astra");
    assert!(
        store
            .reapply_approved(time::OffsetDateTime::now_utc())
            .unwrap()
            .applied
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
        "preferences",
        "--author",
        "opus",
        "agent note",
    ]);
    // A null cite cannot masquerade as Daniel's typed words.
    support::sql(
        &fx.root(),
        "UPDATE entries SET author = '{\"kind\":\"daniel\"}' WHERE id = 'preferences-1'",
    );
    let before = support::snapshot(&fx.root());
    let kept = records(&fx.root());
    let out = fx.run(&["entry", "show", "preferences-1", "--json"]);
    assert_eq!(json(&out.stderr)["code"], "store_corrupt");
    // The refused read may leave bytes in the db's WAL; no record changes.
    let after = support::snapshot(&fx.root());
    let moved = changed(&before, &after);
    assert!(
        moved
            .iter()
            .all(|p| p.to_string_lossy().contains("scribe.db")),
        "{moved:?}"
    );
    assert_eq!(records(&fx.root()), kept);
    support::sql(
        &fx.root(),
        "UPDATE entries SET author = '{\"kind\":\"agent\",\"name\":\"opus\"}' WHERE id = 'preferences-1'",
    );
    fx.ok(&[
        "tag",
        "add",
        "notes",
        "--author",
        "opus",
        "--description",
        "notes",
    ]);
    // A database written by a newer scribe: the tags in it cannot be read
    // or changed by this one.
    support::sql(&fx.root(), "PRAGMA user_version = 999");
    let before = support::snapshot(&fx.root());
    let out = fx.run(&["tag", "list", "--json"]);
    assert_eq!(json(&out.stderr)["code"], "store_too_new");
    let moved = changed(&before, &support::snapshot(&fx.root()));
    assert!(
        moved
            .iter()
            .all(|p| p.to_string_lossy().contains("scribe.db")),
        "{moved:?}"
    );
    let before = support::snapshot(&fx.root());
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
    assert_eq!(support::snapshot(&fx.root()), before);
}

#[test]
fn invalid_or_settled_proposals_never_silently_drop_changed_inputs() {
    let fx = Fixture::new();
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "preferences",
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
        "preferences-1",
        "--author",
        "opus",
        "--reason",
        "replacement",
        "--json",
    ]);
    assert_eq!(json(&out.stderr)["code"], "proposal_invalid");
    // A refused proposal leaves no claim behind.
    assert!(
        support::exported(&fx.root(), "proposals.json").is_none_or(|p| {
            json(p.as_bytes())["proposals"]
                .as_array()
                .is_none_or(Vec::is_empty)
        })
    );
    fx.ok(&[
        "proposal",
        "add",
        "--kind",
        "stale",
        "--older",
        "preferences-1",
        "--author",
        "opus",
        "--reason",
        "review",
    ]);
    fx.ok(&["proposal", "settle", "p-1", "--reject", "--author", "astra"]);
    let before = support::exported(&fx.root(), "proposals.json").unwrap();
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
            support::exported(&fx.root(), "proposals.json").unwrap(),
            before
        );
    }
    let entry = json(&fx.ok(&["entry", "show", "preferences-1", "--json"]).stdout);
    assert_eq!(entry["status"], "active");
    let log = support::audit_text(&fx.root());
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
            "preferences",
            "--author",
            "opus",
            content,
        ]);
    }
    // A row that does not decode.
    support::sql(
        &fx.root(),
        "UPDATE entries SET author = '[]' WHERE id = 'preferences-2'",
    );
    let out = fx.run(&[
        "entry",
        "update",
        "prefe",
        "--author",
        "opus",
        "--content",
        "replacement",
        "--json",
    ]);
    assert_eq!(json(&out.stderr)["code"], "id_ambiguous");
    let out = fx.run(&["entry", "show", "preferences-2", "--json"]);
    assert_eq!(json(&out.stderr)["code"], "store_corrupt");
    let unchanged = json(&fx.ok(&["entry", "show", "preferences-1", "--json"]).stdout);
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
        assert_eq!(json(&out.stderr)["code"], "user_only");
    }
    let audit = support::audit_text(&fx.root());
    let lines: Vec<serde_json::Value> = audit
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let refused = &lines[lines.len() - refusals.len()..];
    for (line, (_, command, target)) in refused.iter().zip(&refusals) {
        assert_eq!(line["command"], *command);
        assert_eq!(line["status"], "failure");
        assert_eq!(line["error_code"], "user_only");
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
    let audit = support::audit_text(&fx.root());
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
    for name in [
        "user",
        "daniel",
        "scribe",
        "consistency",
        "session-pass",
        "tag-retired",
    ] {
        for args in [
            vec![
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                name,
                "note",
                "--json",
            ],
            vec![
                "entry",
                "update",
                "preferences-1",
                "--author",
                name,
                "--content",
                "x",
                "--json",
            ],
            vec![
                "entry",
                "retire",
                "preferences-1",
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
                "preferences-1",
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
    assert!(!support::has_control(&fx.root()));
}

#[test]
fn agent_names_are_attribution_so_any_agent_may_tidy_another_agents_work() {
    let fx = Fixture::new();
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "preferences",
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
            "preferences-1",
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
            "preferences-1",
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
        "preferences-1",
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
        "preferences",
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
        "preferences-2",
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
    assert!(!support::has_control(&fx.root()));
    assert!(!fx.root().join("sessions").exists());
}

#[test]
fn settling_a_scribe_proposal_cannot_replace_its_reason() {
    let fx = Fixture::new();
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "preferences",
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
        "preferences-1",
        "--reason",
        "routine reason",
        "--author",
        "opus",
    ]);
    // Make it a routine-opened (scribe) proposal, as `consistency scan` writes.
    support::sql(
        &fx.root(),
        "UPDATE claims SET author = '{\"kind\":\"scribe\",\"name\":\"consistency\"}' WHERE n = 1",
    );

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

fn write_finding_config(fx: &Fixture) {
    fx.create_private_dir_all(&fx.root());
    let config = fx.root().join("config.toml");
    std::fs::write(
        &config,
        "[tags.preferences]\n[tags.finding]\nrequire_evidence = true\n",
    )
    .unwrap();
    support::make_private(&config);
}

#[test]
fn a_finding_without_evidence_is_refused_and_writes_nothing() {
    let fx = Fixture::new();
    // Without config, `finding` does not ship and is refused as unknown.
    let unconfigured = fx.run(&[
        "entry",
        "add",
        "--tag",
        "finding",
        "--author",
        "opus",
        "--evidence",
        "exp-1",
        "The reviewer mix alone predicts a 7-point gap",
        "--json",
    ]);
    assert_eq!(unconfigured.status.code(), Some(1));
    assert_eq!(json(&unconfigured.stderr)["code"], "tag_not_found");

    write_finding_config(&fx);
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

    // Evidence belongs to a require_evidence tag only.
    let out = fx.run(&[
        "entry",
        "add",
        "--tag",
        "preferences",
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
    write_finding_config(&fx);
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
        &fx.ok(&["search", "reviewer mix predicts gap", "--json"])
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
    write_finding_config(&fx);
    let add = |session: &str, n: usize| {
        // The other session must not repeat a statement: coverage is the
        // tag and project, not the session, so an exact repeat would be
        // `already_covered` rather than a fresh cap.
        let statement = if session == "s-other" {
            format!("Measured result number {n} for the other session cache")
        } else {
            format!("Measured result number {n} for the widget cache")
        };
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
            &statement,
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

#[test]
fn stored_remind_and_finding_entries_stay_readable_searchable_and_asked_without_config() {
    let fx = Fixture::new();
    // Without tag config, `tag list` lists neither `remind` nor `finding`.
    let empty_tags = json(&fx.ok(&["tag", "list", "--json"]).stdout);
    let tag_names: Vec<&str> = empty_tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tag_names,
        [
            "decision",
            "handoff",
            "open-item",
            "preferences",
            "standard"
        ]
    );

    // Seed entries under `finding` and `remind` via a temporary config, then
    // remove config.toml so neither tag is configured.
    fx.create_private_dir_all(&fx.root());
    let config = fx.root().join("config.toml");
    std::fs::write(
        &config,
        "[tags.finding]\nrequire_evidence = true\n[tags.remind]\naction = true\n",
    )
    .unwrap();
    support::make_private(&config);
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "finding",
        "--author",
        "opus",
        "--evidence",
        "exp-99",
        "Observed 44% against 67% on the reviewer mix",
    ]);
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "remind",
        "--author",
        "opus",
        "Re-check the widget cache warmup after deploy",
    ]);
    std::fs::remove_file(&config).unwrap();

    for id in ["finding-1", "remind-1"] {
        let listed = json(&fx.ok(&["entry", "list", "--json"]).stdout);
        assert!(
            listed["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == id),
            "{id} missing from entry list"
        );
        let shown = json(&fx.ok(&["entry", "show", id, "--json"]).stdout);
        assert_eq!(shown["id"], id);
    }
    let finding_hit = json(&fx.ok(&["search", "reviewer mix", "--json"]).stdout);
    assert_eq!(finding_hit["hits"][0]["id"], "finding-1");
    assert_eq!(finding_hit["hits"][0]["evidence"], "exp-99");
    let remind_hit = json(&fx.ok(&["search", "widget cache warmup", "--json"]).stdout);
    assert_eq!(remind_hit["hits"][0]["id"], "remind-1");
}
