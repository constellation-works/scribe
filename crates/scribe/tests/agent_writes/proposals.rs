#[test]
fn interrupted_agent_approval_reapplies_with_its_author() {
    let fx = Fixture::new();
    fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "old note",
        ],
        Success,
    );
    fx.check(
        Cli(&[
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
        ]),
        Success,
    );
    fx.check(
        Cli(&[
            "proposal",
            "settle",
            "p-1",
            "--approve",
            "--author",
            "astra",
        ]),
        Success,
    );
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
    let entry = json(
        &fx.check(&["show", "preferences-1", "--json"], Success)
            .stdout,
    );
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
    fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "agent note",
        ],
        Success,
    );
    // A null cite cannot masquerade as Daniel's typed words.
    support::sql(
        &fx.root(),
        "UPDATE entries SET author = '{\"kind\":\"daniel\"}' WHERE id = 'preferences-1'",
    );
    let before = support::snapshot(&fx.root());
    let kept = records(&fx.root());
    let out = fx.check(&["show", "preferences-1", "--json"], Capture);
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
    fx.check(
        Cli(&[
            "tag",
            "add",
            "notes",
            "--author",
            "opus",
            "--description",
            "notes",
        ]),
        Success,
    );
    // A database written by a newer scribe: the tags in it cannot be read
    // or changed by this one.
    support::sql(&fx.root(), "PRAGMA user_version = 999");
    let before = support::snapshot(&fx.root());
    let out = fx.check(Cli(&["tag", "list", "--json"]), Capture);
    assert_eq!(json(&out.stderr)["code"], "store_too_new");
    let moved = changed(&before, &support::snapshot(&fx.root()));
    assert!(
        moved
            .iter()
            .all(|p| p.to_string_lossy().contains("scribe.db")),
        "{moved:?}"
    );
    let before = support::snapshot(&fx.root());
    let out = fx.check(
        Cli(&[
            "tag",
            "update",
            "notes",
            "--author",
            "opus",
            "--description",
            "replacement",
            "--json",
        ]),
        Capture,
    );
    assert_eq!(json(&out.stderr)["code"], "store_too_new");
    assert_eq!(support::snapshot(&fx.root()), before);
}

#[test]
fn invalid_or_settled_proposals_never_silently_drop_changed_inputs() {
    let fx = Fixture::new();
    fx.check(
        &["add", "--tag", "preferences", "--author", "opus", "note"],
        Success,
    );
    let out = fx.check(
        Cli(&[
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
        ]),
        Capture,
    );
    assert_eq!(json(&out.stderr)["code"], "proposal_invalid");
    // A refused proposal leaves no claim behind.
    assert!(
        support::exported(&fx.root(), "proposals.json").is_none_or(|p| {
            json(p.as_bytes())["proposals"]
                .as_array()
                .is_none_or(Vec::is_empty)
        })
    );
    fx.check(
        Cli(&[
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
        ]),
        Success,
    );
    fx.check(
        Cli(&["proposal", "settle", "p-1", "--reject", "--author", "astra"]),
        Success,
    );
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
        let out = fx.check(Cli(&args), Capture);
        assert_eq!(json(&out.stderr)["code"], "proposal_settled");
        assert_eq!(
            support::exported(&fx.root(), "proposals.json").unwrap(),
            before
        );
    }
    let entry = json(
        &fx.check(&["show", "preferences-1", "--json"], Success)
            .stdout,
    );
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
        fx.check(
            &["add", "--tag", "preferences", "--author", "opus", content],
            Success,
        );
    }
    // A row that does not decode.
    support::sql(
        &fx.root(),
        "UPDATE entries SET author = '[]' WHERE id = 'preferences-2'",
    );
    let out = fx.check(
        &[
            "update",
            "prefe",
            "--author",
            "opus",
            "--content",
            "replacement",
            "--json",
        ],
        Capture,
    );
    assert_eq!(json(&out.stderr)["code"], "id_ambiguous");
    let out = fx.check(&["show", "preferences-2", "--json"], Capture);
    assert_eq!(json(&out.stderr)["code"], "store_corrupt");
    let unchanged = json(
        &fx.check(&["show", "preferences-1", "--json"], Success)
            .stdout,
    );
    assert_eq!(unchanged["content"], "one");
}

#[test]
fn refused_writes_audit_the_target_and_author_but_never_the_arguments() {
    let fx = Fixture::seeded();
    let config = fx.root().join("config.toml");
    let text = std::fs::read_to_string(&config)
        .unwrap()
        .replace("env = [\"GITHUB_TOKEN\"]", "env = []");
    std::fs::write(config, text).unwrap();
    let proposal = json(
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
        let out = fx.check(Cli(args), Capture);
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
    let fx = Fixture::seeded();
    let out = fx.check(
        Cli(&[
            "session",
            "rebuild",
            "s-missing-1",
            "--author",
            "opus",
            "--confirm",
            "--json",
        ]),
        Capture,
    );
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
            let out = fx.check(Cli(&args), Capture);
            assert_eq!(
                json(&out.stderr)["code"],
                "reserved_author",
                "{args:?}: {}",
                text(&out.stderr)
            );
        }
    }
    // Reads still accept the name as a filter.
    fx.check(&["list", "--author", "daniel", "--json"], Success);
    assert!(!fx.root().join("entries").exists());
    assert!(!support::has_control(&fx.root()));
}

#[test]
fn agent_names_are_attribution_so_any_agent_may_tidy_another_agents_work() {
    let fx = Fixture::new();
    fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "opus note",
        ],
        Success,
    );
    // Another agent may rewrite and retag an agent note; the record keeps
    // its author and who changed it.
    let updated = json(
        &fx.check(
            &[
                "update",
                "preferences-1",
                "--author",
                "sonnet",
                "--content",
                "rewritten",
                "--add-topic",
                "api",
                "--json",
            ],
            Success,
        )
        .stdout,
    );
    assert_eq!(updated["content"], "rewritten");
    assert_eq!(updated["author"]["name"], "opus");
    assert_eq!(updated["updated_by"]["name"], "sonnet");
    let retired = json(
        &fx.check(
            &[
                "retire",
                "preferences-1",
                "--author",
                "sonnet",
                "--reason",
                "done",
                "--json",
            ],
            Success,
        )
        .stdout,
    );
    assert_eq!(retired["status"], "retired");
    assert_eq!(retired["author"]["name"], "opus");
    // A retired note still refuses further updates, from anyone.
    let out = fx.check(
        &[
            "update",
            "preferences-1",
            "--author",
            "opus",
            "--content",
            "back",
            "--json",
        ],
        Capture,
    );
    assert_eq!(json(&out.stderr)["code"], "entry_not_active");

    fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "second note",
        ],
        Success,
    );
    fx.check(
        Cli(&[
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
        ]),
        Success,
    );
    // Another agent may revise an agent proposal's reason.
    let revised = json(
        &fx.check(
            Cli(&[
                "proposal", "update", "p-1", "--author", "sonnet", "--reason", "revised", "--json",
            ]),
            Success,
        )
        .stdout,
    );
    assert_eq!(revised["reason"], "revised");
    assert_eq!(revised["author"]["name"], "opus");
    assert_eq!(revised["updated_by"]["name"], "sonnet");
    // The proposer may settle its own proposal; the settler is recorded.
    let settled = json(
        &fx.check(
            Cli(&[
                "proposal", "settle", "p-1", "--reject", "--author", "opus", "--json",
            ]),
            Success,
        )
        .stdout,
    );
    assert_eq!(settled["state"], "rejected");
    assert_eq!(settled["reason"], "revised");
    assert_eq!(settled["author"]["name"], "opus");
    assert_eq!(settled["settled_author"]["name"], "opus");
}
