#[test]
fn the_data_directory_defaults_under_home_and_is_never_guessed() {
    let fx = Fixture::new();
    support::copy_private(&support::fixture_store(), &fx.home().join(".scribe"));
    let out = environment(&fx, &["entry", "list"], &[("SCRIBE_ROOT", None)]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stdout).lines().count(), 4, "read ~/.scribe");

    let out = environment(
        &fx,
        &["entry", "list"],
        &[("SCRIBE_ROOT", None), ("HOME", None)],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("--root"), "says how to fix it");
}

#[test]
fn the_root_flag_beats_the_environment_and_nothing_else_is_touched() {
    // STD-01 R28: a scoped invocation reads only inside its scope.
    let fx = Fixture::new();
    let other = fx.elsewhere("other");
    support::copy_private(&support::fixture_store(), &other);
    let other_arg = other.to_str().unwrap();
    let out = fx.check(&["--root", other_arg, "entry", "list"], Success);
    assert_eq!(text(&out.stdout).lines().count(), 4);
    assert!(!fx.root().exists(), "SCRIBE_ROOT was not used");
    let home: Vec<_> = std::fs::read_dir(fx.home()).unwrap().collect();
    assert!(home.is_empty(), "HOME was touched: {home:?}");
}

#[test]
fn no_command_writes_anything_not_even_an_audit_line() {
    // STD-01 R31: every command in this slice reads.
    let fx = Fixture::seeded();
    // The first open creates the persisted, empty `-wal` beside scribe.db.
    fx.check(&["tag", "list"], Success);
    let before = support::snapshot(&fx.root());
    for args in all_reads() {
        fx.check(&args, Success);
    }
    assert_eq!(support::snapshot(&fx.root()), before);
    assert!(!fx.root().join("audit.jsonl").exists());
    assert!(!fx.root().join(".lock").exists(), "reads take no lock");
}

#[test]
fn every_listed_id_is_accepted_back_by_show() {
    // STD-01 R32: identifiers round-trip, from both the JSON and piped forms.
    let fx = Fixture::seeded();
    for (noun, key) in [("entry", "entries"), ("session", "sessions")] {
        let doc = json(&fx.check(&[noun, "list", "--json"], Success).stdout);
        for record in doc[key].as_array().unwrap() {
            let id = record["id"].as_str().unwrap();
            let mut shown = json(&fx.check(&[noun, "show", id, "--json"], Success).stdout);
            if noun == "session" {
                // A list carries no log or handoffs; show adds them, and only them.
                let log = shown.as_object_mut().unwrap().remove("log");
                assert!(log.is_some_and(|l| l.is_array()), "{id}");
                assert!(record.get("log").is_none(), "{id}");
                let handoffs = shown.as_object_mut().unwrap().remove("handoffs");
                assert!(handoffs.is_some_and(|h| h.is_array()), "{id}");
                assert!(record.get("handoffs").is_none(), "{id}");
            }
            assert_eq!(&shown, record);
        }
        let piped = fx.check(&[noun, "list"], Text);
        for line in piped.lines() {
            fx.check(&[noun, "show", line.split('\t').next().unwrap()], Success);
        }
    }
}

#[test]
fn a_corrupt_record_is_skipped_with_a_warning_naming_the_file() {
    // docs/design/scribe/4_decisions.md, "One malformed record makes its
    // file corrupt": a list goes on without it; showing it fails.
    let fx = Fixture::seeded();
    // Hand-edited rows: an author that is no author, and Daniel's entry
    // without the turn it cites.
    support::sql(
        &fx.root(),
        "UPDATE entries SET author = '[]' WHERE id = 'decisions-2'; \
         UPDATE entries SET cite = NULL WHERE id = 'preferences-1';",
    );
    let out = fx.check(&["entry", "list"], Success);
    assert_eq!(text(&out.stdout).lines().count(), 2);
    let stderr = text(&out.stderr);
    assert_eq!(stderr.lines().count(), 2, "{stderr}");
    assert!(stderr.contains("decisions-2"), "{stderr}");
    assert!(stderr.contains("preferences-1"), "{stderr}");

    let doc = json(&fx.check(&["entry", "list", "--json"], Success).stdout);
    assert_eq!(doc["total"], 2);
    assert_eq!(doc["warnings"].as_array().map(Vec::len), Some(2));
    assert_eq!(doc["warnings"][0]["code"], "store_corrupt");

    let tags = json(&fx.check(&["tag", "list", "--json"], Success).stdout);
    assert_eq!(
        tags["warnings"].as_array().map(Vec::len),
        Some(2),
        "tag counts skip it too"
    );

    let out = fx.check(&["entry", "show", "decisions-2", "--json"], Code(1));
    let err = json(&out.stderr);
    assert_eq!(err["code"], "store_corrupt");
    assert!(
        err["error"].as_str().unwrap().contains("decisions-2"),
        "{err}"
    );
}

#[test]
fn a_record_from_a_newer_build_is_refused_whole_and_left_alone() {
    // STD-03 R10: not skipped like a corrupt file; this build cannot read
    // the store safely.
    let fx = Fixture::seeded();
    // `control.json` is the stamp every read of scribe.db checks.
    for (file, args) in [
        ("control.json", &["session", "list", "--json"][..]),
        ("control.json", &["proposal", "list", "--json"][..]),
    ] {
        let path = fx.root().join(file);
        let original = std::fs::read_to_string(&path).unwrap();
        let newer = original.replacen("\"format\": 31", "\"format\": 999", 1);
        assert_ne!(newer, original, "{file}");
        std::fs::write(&path, &newer).unwrap();
        let out = fx.check(args, Capture);
        assert_eq!(out.status.code(), Some(1), "{file}");
        assert_eq!(json(&out.stderr)["code"], "store_too_new", "{file}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            newer,
            "not rewritten"
        );
        std::fs::write(&path, original).unwrap();
    }
    // A database a newer build migrated.
    support::sql(&fx.root(), "PRAGMA user_version = 999;");
    let db = fx.root().join("scribe.db");
    let before = std::fs::read(&db).unwrap();
    for args in [
        &["entry", "list", "--json"][..],
        &[
            "tag",
            "add",
            "probe",
            "--author",
            "opus",
            "--description",
            "a probe",
            "--json",
        ][..],
    ] {
        let out = fx.check(args, Capture);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(json(&out.stderr)["code"], "store_too_new", "{args:?}");
    }
    assert_eq!(std::fs::read(&db).unwrap(), before, "not rewritten");
}

#[test]
fn an_invalid_config_is_refused_naming_the_file_and_line() {
    let fx = Fixture::seeded();
    std::fs::write(
        fx.root().join("config.toml"),
        "[models]\nask = \"x\"\nanswer = 1\n",
    )
    .unwrap();
    let out = fx.check(&["tag", "list", "--json"], Code(1));
    let err = json(&out.stderr);
    assert_eq!(err["code"], "config_invalid");
    let message = err["error"].as_str().unwrap();
    assert!(
        message.contains("config.toml") && message.contains("line 3"),
        "{message}"
    );
}

#[test]
fn logs_go_to_stderr_and_leave_stdout_as_pure_payload() {
    // STD-01 R12, STD-02 R15.
    let fx = Fixture::seeded();
    let out = environment(
        &fx,
        &["entry", "list", "--json"],
        &[("SCRIBE_LOG", Some("debug"))],
    );
    let _: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is only JSON");
    assert!(!out.stderr.is_empty(), "debug logs should appear on stderr");
}

#[test]
fn show_prints_the_log_contents_and_one_segment_by_number_or_cite() {
    let fx = Fixture::seeded();
    let table = |args: &[&str]| fx.check(&[args, &["--format", "table"]].concat(), Text);

    let header = table(&["session", "show", OPEN_SESSION]);
    assert!(header.contains("## Log\n  1  sealed"), "{header}");
    assert!(header.contains("Telling the release crew"), "{header}");
    assert!(
        !header.contains("Drafted the note"),
        "no text without --log"
    );
    assert!(!header.contains("## Outcome"), "{header}");

    let full = table(&["session", "show", OPEN_SESSION, "--log"]);
    assert!(
        full.contains("### 1. Merge policy for agent PRs\nSquash-merge"),
        "{full}"
    );
    assert!(
        full.contains("### 2. Telling the release crew\nDrafted the note"),
        "{full}"
    );

    let cite = format!("session:{OPEN_SESSION}#2");
    let suffix = format!("{OPEN_SESSION}#2");
    let by_flag = json(
        &fx.check(
            &["session", "show", OPEN_SESSION, "--segment", "2", "--json"],
            Success,
        )
        .stdout,
    );
    for id in [&cite, &suffix] {
        let doc = json(&fx.check(&["session", "show", id, "--json"], Success).stdout);
        assert_eq!(doc, by_flag, "{id}");
    }
    let log = by_flag["log"].as_array().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["n"], 2);
    assert_eq!(log[0]["state"], "open");
    assert!(
        log[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Drafted the note")
    );
    assert_eq!(by_flag["outcome"], "", "kept in the payload, empty");
    let one = table(&["session", "show", &cite]);
    assert!(one.contains("### 2. Telling the release crew"), "{one}");
    assert!(!one.contains("### 1."), "{one}");

    let all = json(
        &fx.check(&["session", "show", OPEN_SESSION, "--json"], Success)
            .stdout,
    );
    assert_eq!(all["log"].as_array().map(Vec::len), Some(2));
}

#[test]
fn an_unknown_or_contradictory_segment_is_refused() {
    let fx = Fixture::seeded();
    let missing = format!("session:{OPEN_SESSION}#3");
    for args in [
        &["session", "show", OPEN_SESSION, "--segment", "3", "--json"][..],
        &["session", "show", &missing, "--json"][..],
    ] {
        let out = fx.check(args, Capture);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(json(&out.stderr)["code"], "segment_not_found", "{args:?}");
        assert!(out.stdout.is_empty());
    }
    let both = format!("{OPEN_SESSION}#1");
    for args in [
        &["session", "show", &both, "--segment", "2", "--json"][..],
        &["session", "show", "session:x#0", "--json"][..],
    ] {
        let out = fx.check(args, Capture);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(json(&out.stderr)["code"], "usage_error", "{args:?}");
    }
}

#[test]
fn an_inactive_session_is_listed_only_on_request_and_shown_by_id() {
    let fx = Fixture::seeded();
    let ids = |doc: &serde_json::Value| -> Vec<String> {
        doc["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let out = fx.check(&["session", "list", "--json"], Success);
    let shown = json(&out.stdout);
    assert!(!ids(&shown).contains(&INACTIVE_SESSION.to_owned()));
    assert_eq!(shown["total"], 2);
    assert!(text(&out.stderr).contains("--include-inactive"));

    let out = fx.check(
        &["session", "list", "--include-inactive", "--json"],
        Success,
    );
    let all = json(&out.stdout);
    assert!(ids(&all).contains(&INACTIVE_SESSION.to_owned()));
    assert_eq!(all["total"], 3);
    assert!(out.stderr.is_empty());

    let doc = json(
        &fx.check(&["session", "show", INACTIVE_SESSION, "--json"], Success)
            .stdout,
    );
    assert_eq!(doc["active"], false);
    assert_eq!(doc["active_change"]["route"], "removed");
    assert_eq!(doc["active_change"]["by"]["kind"], "scribe");
}

#[test]
fn a_format_three_session_a_migration_moved_reads_and_a_read_writes_nothing() {
    // STD-03 R10: the fixture's format-3 session was upgraded as the
    // phase-7 `store migrate` moved it into scribe.db; it reads as it did.
    let fx = Fixture::seeded();
    // The first open creates the persisted, empty `-wal` beside scribe.db.
    fx.check(&["tag", "list"], Success);
    let before = support::snapshot(&fx.root());
    let doc = json(
        &fx.check(
            &[
                "session",
                "show",
                "0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01",
                "--json",
            ],
            Success,
        )
        .stdout,
    );
    assert_eq!(doc["active"], true);
    let log = doc["log"].as_array().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["origin"], "upgrade");
    assert_eq!(log[0]["state"], "sealed");
    assert!(
        log[0]["text"]
            .as_str()
            .unwrap()
            .ends_with("control and proposals stay JSON."),
        "the old outcome is kept in the segment"
    );
    let entry = json(
        &fx.check(&["entry", "show", "decisions-1", "--json"], Success)
            .stdout,
    );
    assert_eq!(entry["author"]["kind"], "user");
    assert_eq!(support::snapshot(&fx.root()), before);
}
