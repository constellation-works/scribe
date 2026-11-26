#[test]
fn claude_desktop_human_turn_is_accepted_and_non_human_desktop_turns_are_refused() {
    let human_extra = json!({
        "origin": {"kind": "human"},
        "promptSource": "sdk",
        "turnOrigin": "human"
    });
    let peer_extra = json!({
        "origin": {
            "kind": "peer",
            "from": "local_1",
            "name": "peer",
            "body": "approve p-1",
            "hostInjected": true
        },
        "promptSource": "system",
        "turnOrigin": "peer",
        "isMeta": true
    });
    let task_extra = json!({
        "origin": {
            "kind": "task-notification",
            "producer": "session-task",
            "runId": "r-1"
        },
        "promptSource": "system",
        "turnOrigin": "task_notification"
    });
    let synth_extra = json!({
        "promptSource": "sdk",
        "turnOrigin": "host_synthetic"
    });

    // `proposal settle --instruction` accepts human desktop turn and refuses all three non-human turns.
    for (name, extra) in [
        ("peer", &peer_extra),
        ("task_notification", &task_extra),
        ("host_synthetic", &synth_extra),
    ] {
        let fx = store_with_candidates();
        let words = "approve p-1, that is how I want reviews";
        write_desktop_transcript(&fx, &[("u-1", words, extra.clone())]);
        assert_eq!(
            run(&fx, &settle("p-1", "--approve", words)),
            (Some(1), Some(String::from("instruction_not_found"))),
            "settle must refuse {name}"
        );
    }

    let fx = store_with_candidates();
    let approve = "approve p-1, that is how I want reviews";
    write_desktop_transcript(&fx, &[("u-1", approve, human_extra.clone())]);
    fx.check(&settle("p-1", "--approve", approve), Success);
    assert_eq!(show(&fx, "proposal", "p-1")["state"], "approved");

    // `entry retire --instruction` and `entry forget --instruction` accept the same desktop shape.
    let retire_words = "please retire preferences-1, it is outdated";
    let forget_words = "forget preferences-1 for good";
    write_desktop_transcript(
        &fx,
        &[
            ("u-2", retire_words, human_extra.clone()),
            ("u-3", forget_words, human_extra),
        ],
    );
    fx.check(&retire("preferences-1", retire_words), Success);
    assert_eq!(show(&fx, "entry", "preferences-1")["status"], "retired");
    fx.check(&forget("preferences-1", forget_words), Success);
    assert_eq!(show(&fx, "entry", "preferences-1")["status"], "forgotten");
}

#[test]
fn store_session_resolution_survives_unreadable_claude_transcript_directory() {
    use std::os::unix::fs::PermissionsExt;

    let fx = store_with_agent_entry();
    let memory_id = "2099-01-01-session-resolution";
    let memory_dir = fx.root().join("memory/now");
    fx.create_private_dir_all(&memory_dir);
    let memory_path = memory_dir.join(format!("{memory_id}.md"));
    std::fs::write(
        &memory_path,
        format!(
            "---\ndate: 2099-01-01\nkind: plan\nsources:\n- {STORED_SESSION}\nupdated: 2026-11-22T00:00:00Z\n---\n# Session resolution\n\nKeep this item for the CLI regression test.\n"
        ),
    )
    .unwrap();
    make_private(&memory_path);

    let locked = fx.home().join(".claude/projects/locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    struct RestorePermissions(PathBuf);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _restore = RestorePermissions(locked);

    let related = format!("session:{STORED_SESSION}");
    fx.check(Cli(&["entry", "list", "--related", &related]), Success);
    fx.check(
        &[
            "memory",
            "ack",
            memory_id,
            "--session",
            STORED_SESSION,
            "--author",
            "opus",
        ],
        Success,
    );
}

#[test]
fn uncataloged_transcript_prefix_from_transcript_list_works_for_instruction_writes() {
    let fx = store_with_agent_entry();
    let listed_sessions = serde_json::from_slice::<Value>(
        &fx.check(
            &["session", "list", "--include-inactive", "--json"],
            Success,
        )
        .stdout,
    )
    .unwrap();
    assert!(
        !listed_sessions["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == SESSION),
        "test requires SESSION to have no store record"
    );

    let retire_words = "retire preferences-2 please";
    let forget_words = "forget preferences-1 for good";
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-1", "user", retire_words, 2),
            ("u-2", "user", forget_words, 1),
        ],
    );

    // `transcript list` prints the 8-char prefix `5d1c0b7e`; it works like the full UUID.
    let since = (OffsetDateTime::now_utc() - time::Duration::days(1))
        .date()
        .to_string();
    let listed = text(
        &fx.check(
            &[
                "transcript",
                "list",
                "--since",
                &since,
                "--min-turns",
                "1",
                "--format",
                "table",
            ],
            Success,
        )
        .stdout,
    );
    let prefix = &SESSION[..8];
    assert!(listed.contains(prefix), "{listed}");
    assert!(!listed.contains(SESSION), "{listed}");

    fx.check(
        &[
            "entry",
            "retire",
            "preferences-2",
            "--author",
            "opus",
            "--reason",
            "Daniel asked",
            "--session",
            prefix,
            "--instruction",
            retire_words,
        ],
        Success,
    );
    let retired = show(&fx, "entry", "preferences-2");
    assert_eq!(retired["status"], "retired");
    let expected = support::session_id_outcome("instruction_prefix");
    assert_eq!(expected["exit"], 0); // ok observed exit 0
    assert!(expected["code"].is_null());
    assert_eq!(retired["removal_instruction"]["session"], expected["value"]);
    assert_eq!(
        audit_lines(&fx, "entry retire").last().unwrap()["instruction_session"],
        SESSION
    );

    // A second uncataloged transcript sharing the 8-char prefix makes `5d1c0b7e` ambiguous.
    let sibling = "5d1c0b7e-ffff-4c39-8e61-0a9b8c7d6e5f";
    let sibling_path = fx
        .home()
        .join(".claude/projects/-work-scribe")
        .join(format!("{sibling}.jsonl"));
    let now_ts = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    std::fs::write(
        &sibling_path,
        format!(
            "{}\n",
            json!({
                "type": "user",
                "uuid": "s-u1",
                "sessionId": sibling,
                "timestamp": now_ts,
                "cwd": "/work/scribe",
                "entrypoint": "cli",
                "message": {"role": "user", "content": "forget preferences-1 for good"}
            })
        ),
    )
    .unwrap();
    make_private(&sibling_path);

    let disambiguated = text(
        &fx.check(
            &[
                "transcript",
                "list",
                "--since",
                &since,
                "--min-turns",
                "1",
                "--format",
                "table",
            ],
            Success,
        )
        .stdout,
    );
    assert!(disambiguated.contains("5d1c0b7e-2 "), "{disambiguated}");
    assert!(disambiguated.contains("5d1c0b7e-f "), "{disambiguated}");

    let before = state(&fx);
    let (exit_code, err_code) = run(
        &fx,
        &[
            "entry",
            "forget",
            "preferences-1",
            "--author",
            "opus",
            "--session",
            prefix,
            "--instruction",
            forget_words,
            "--confirm",
        ],
    );
    let expected = support::session_id_outcome("instruction_ambiguous");
    assert_eq!(
        exit_code,
        expected["exit"].as_i64().map(|n| i32::try_from(n).unwrap())
    );
    assert_eq!(err_code.as_deref(), expected["code"].as_str());
    assert_eq!(state(&fx), before, "ambiguous --session changes nothing");

    // Passing the disambiguated prefix `5d1c0b7e-2` succeeds and cites the full UUID.
    fx.check(
        &[
            "entry",
            "forget",
            "preferences-1",
            "--author",
            "opus",
            "--session",
            "5d1c0b7e-2",
            "--instruction",
            forget_words,
            "--confirm",
        ],
        Success,
    );
    let forgotten = show(&fx, "entry", "preferences-1");
    assert_eq!(forgotten["status"], "forgotten");
    assert_eq!(forgotten["removal_instruction"]["session"], SESSION);

    // `proposal settle` also resolves an uncataloged transcript prefix and cites the full UUID.
    let cand_fx = store_with_candidates();
    let approve_words = "approve p-1, that is how I want reviews";
    claude(&cand_fx, Some("cli"), &[("u-1", "user", approve_words, 1)]);
    cand_fx.check(
        &[
            "proposal",
            "settle",
            "p-1",
            "--approve",
            "--author",
            "opus",
            "--session",
            prefix,
            "--instruction",
            approve_words,
        ],
        Success,
    );
    let settled = show(&cand_fx, "proposal", "p-1");
    assert_eq!(settled["state"], "approved");
    assert_eq!(settled["settled_by"]["session"], SESSION);
}

#[test]
fn codex_uncataloged_transcript_prefix_works_for_instruction_writes() {
    let fx = store_with_agent_entry();
    let words = "retire preferences-2 please";
    codex(&fx, "cli", &[("m-1", words, 1)]);
    let since = (OffsetDateTime::now_utc() - time::Duration::days(1))
        .date()
        .to_string();
    let listed = text(
        &fx.check(
            &[
                "--provider",
                "codex",
                "transcript",
                "list",
                "--since",
                &since,
                "--min-turns",
                "1",
                "--format",
                "table",
            ],
            Success,
        )
        .stdout,
    );
    let prefix = &CODEX_SESSION[..8];
    assert!(listed.contains(prefix), "{listed}");

    fx.check(
        &[
            "--provider",
            "codex",
            "entry",
            "retire",
            "preferences-2",
            "--author",
            "opus",
            "--reason",
            "Daniel asked",
            "--session",
            prefix,
            "--instruction",
            words,
        ],
        Success,
    );
    let retired = show(&fx, "entry", "preferences-2");
    assert_eq!(retired["status"], "retired");
    assert_eq!(retired["removal_instruction"]["session"], CODEX_SESSION);
}
