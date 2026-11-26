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
    let out = catalog_in_terminal(&fx, id);
    let shown = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(shown.contains("reserved_author"), "{shown}");
    assert!(!support::has_control(&fx.root()));
    assert!(!fx.root().join("sessions").exists());
}

#[test]
fn settling_a_scribe_proposal_cannot_replace_its_reason() {
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
    fx.check(
        Cli(&[
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
        ]),
        Success,
    );
    // Make it a routine-opened (scribe) proposal, as `consistency scan` writes.
    support::sql(
        &fx.root(),
        "UPDATE claims SET author = '{\"kind\":\"scribe\",\"name\":\"consistency\"}' WHERE n = 1",
    );

    let out = fx.check(
        Cli(&[
            "proposal",
            "settle",
            "p-1",
            "--reject",
            "--author",
            "sonnet",
            "--reason",
            "rewritten by agent",
            "--json",
        ]),
        Capture,
    );
    assert_eq!(json(&out.stderr)["code"], "proposal_not_agent");
    let list = json(
        &fx.check(Cli(&["proposal", "list", "--json"]), Success)
            .stdout,
    );
    assert_eq!(list["proposals"][0]["state"], "open");
    assert_eq!(list["proposals"][0]["reason"], "routine reason");

    // Settling without a new reason keeps the routine's reason.
    let settled = json(
        &fx.check(
            Cli(&[
                "proposal", "settle", "p-1", "--reject", "--author", "sonnet", "--json",
            ]),
            Success,
        )
        .stdout,
    );
    assert_eq!(settled["state"], "rejected");
    assert_eq!(settled["reason"], "routine reason");
    assert_eq!(settled["settled_author"]["name"], "sonnet");
}

#[test]
fn a_finding_without_evidence_is_refused_and_writes_nothing() {
    let fx = Fixture::new();
    // Without config, `finding` does not ship and is refused as unknown.
    let unconfigured = fx.check(
        &[
            "add",
            "--tag",
            "finding",
            "--author",
            "opus",
            "--evidence",
            "exp-1",
            "The reviewer mix alone predicts a 7-point gap",
            "--json",
        ],
        Code(1),
    );
    assert_eq!(json(&unconfigured.stderr)["code"], "tag_not_found");

    write_finding_config(&fx);
    let out = fx.check(
        &[
            "add",
            "--tag",
            "finding",
            "--author",
            "opus",
            "The reviewer mix alone predicts a 7-point gap",
            "--json",
        ],
        Code(1),
    );
    let err = json(&out.stderr);
    assert_eq!(err["code"], "evidence_required", "{err}");
    assert!(
        text(&out.stderr).contains("--evidence"),
        "{}",
        text(&out.stderr)
    );
    let list = json(
        &fx.check(&["list", "--tag", "finding", "--json"], Success)
            .stdout,
    );
    assert_eq!(list["total"], 0, "a refused finding writes nothing");

    // Evidence belongs to a require_evidence tag only.
    let out = fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--evidence",
            "exp-1",
            "Use the stable API",
            "--json",
        ],
        Code(1),
    );
    assert_eq!(json(&out.stderr)["code"], "evidence_required");
}

#[test]
fn a_finding_records_its_pointer_as_of_date_and_agent_author() {
    let fx = Fixture::new();
    write_finding_config(&fx);
    let added = json(
        &fx.check(
            &[
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
            ],
            Success,
        )
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
        &fx.check(
            Cli(&["search", "reviewer mix predicts gap", "--json"]),
            Success,
        )
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
        fx.check(
            &[
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
            ],
            Capture,
        )
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
        &fx.check(&["list", "--tag", "finding", "--json"], Success)
            .stdout,
    );
    assert_eq!(list["total"], 4);
}

#[test]
fn stored_remind_and_finding_entries_stay_readable_searchable_and_asked_without_config() {
    let fx = Fixture::new();
    // Without tag config, `tag list` lists neither `remind` nor `finding`.
    let empty_tags = json(&fx.check(Cli(&["tag", "list", "--json"]), Success).stdout);
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
    fx.check(
        &[
            "add",
            "--tag",
            "finding",
            "--author",
            "opus",
            "--evidence",
            "exp-99",
            "Observed 44% against 67% on the reviewer mix",
        ],
        Success,
    );
    fx.check(
        &[
            "add",
            "--tag",
            "remind",
            "--author",
            "opus",
            "Re-check the widget cache warmup after deploy",
        ],
        Success,
    );
    std::fs::remove_file(&config).unwrap();

    for id in ["finding-1", "remind-1"] {
        let listed = json(&fx.check(&["list", "--json"], Success).stdout);
        assert!(
            listed["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == id),
            "{id} missing from entry list"
        );
        let shown = json(&fx.check(&["show", id, "--json"], Success).stdout);
        assert_eq!(shown["id"], id);
    }
    let finding_hit = json(
        &fx.check(Cli(&["search", "reviewer mix", "--json"]), Success)
            .stdout,
    );
    assert_eq!(finding_hit["hits"][0]["id"], "finding-1");
    assert_eq!(finding_hit["hits"][0]["evidence"], "exp-99");
    let remind_hit = json(
        &fx.check(Cli(&["search", "widget cache warmup", "--json"]), Success)
            .stdout,
    );
    assert_eq!(remind_hit["hits"][0]["id"], "remind-1");
}
