#[test]
fn a_pass_writes_a_dated_now_item_as_its_file_and_keeps_an_undated_one_pending() {
    let s = memory_setup();
    s.scribe("/scribe on");
    let report = pass_on(&s);
    let pass = &report["passes"][0];
    assert_eq!(pass["result"], "written", "{report}");
    assert_eq!(pass["memory_written"], 1, "{report}");
    assert_eq!(pass["memory_pending"], 1, "{report}");
    assert_eq!(pass["memory_refused"], json!({}), "{report}");
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains("Return memory: at most 3 elements"),
        "{prompt}"
    );
    assert!(prompt.contains("Return signal: 0 to 3"), "{prompt}");

    let file = s.fx.root().join(format!("memory/now/{WEDDING_STEM}.md"));
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(
        written.starts_with("---\ndate: 2026-12-12\nkind: event\n"),
        "{written}"
    );
    assert!(
        written.contains(&format!("sources:\n- {ON}\n")),
        "{written}"
    );
    assert!(
        written.ends_with("---\n# Sister's wedding\n\nSister's wedding, as the user said.\n"),
        "{written}"
    );
    let shown = memory_json(&s, &["show", WEDDING_STEM]);
    assert_eq!(shown["kind"], "now", "{shown}");
    assert_eq!(shown["date"], "2026-12-12");
    assert_eq!(shown["sources"], json!([ON]));
    // The memory write stamps control.json at the memory format.
    let control: Value = support::control(&s.fx.root());
    assert_eq!(control["format"], 31);

    // The same window again writes nothing new.
    let before = std::fs::read(&file).unwrap();
    s.append(
        ON,
        &[line("user", "u-9", "One more thing about the crews.")],
    );
    let again = pass_on(&s);
    assert_eq!(again["passes"][0]["memory_written"], 0, "{again}");
    assert_eq!(std::fs::read(&file).unwrap(), before);
}

#[test]
fn a_pass_over_a_session_never_switched_on_writes_no_memory() {
    let s = memory_setup();
    let out = s.run_pass(&["session", "pass", "--session", NEVER, "--json"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
    s.run(&["session", "pass", "--json"], b"");
    assert!(s.prompts().is_empty(), "the model was never called");
    assert!(!s.fx.root().join("memory").exists());
}

#[test]
fn the_sensitive_gate_keeps_credentials_and_finance_words_out() {
    let s = Setup::new();
    let token = "deploy with ghp_abcdefghijklmnopqrstuvwxyz0123456789 by 2026-12-01";
    let salary = "my salary review is on 2026-12-03 with the manager";
    s.append(
        ON,
        &[line("user", "u-t", token), line("user", "u-f", salary)],
    );
    let mut body = answer();
    let mut first = memory_element("now", "Deploy with a token", token, "u-t");
    first["date"] = json!("2026-12-01");
    let mut second = memory_element("now", "Salary review", salary, "u-f");
    second["date"] = json!("2026-12-03");
    body["memory"] = json!([first, second]);
    s.answer(&body);
    s.scribe("/scribe on");
    let report = pass_on(&s);
    assert_eq!(report["passes"][0]["memory_written"], 0, "{report}");
    assert_eq!(
        report["passes"][0]["memory_refused"],
        json!({"memory_sensitive": 2}),
        "{report}"
    );
    let stored = memory_text(&s);
    for kept_out in ["ghp_", "alary"] {
        assert!(!stored.contains(kept_out), "{kept_out} reached memory");
    }
}

#[test]
fn a_milestone_is_written_only_when_the_user_calls_it_one() {
    let s = Setup::new();
    let big = "We shipped scribe 1.0 on 2026-11-02, a big day for me.";
    let plain = "We moved the office on 2026-11-01 without trouble.";
    s.append(ON, &[line("user", "u-b", big), line("user", "u-p", plain)]);
    let mut body = answer();
    let mut shipped = memory_element("milestone", "Shipped scribe 1.0", big, "u-b");
    shipped["date"] = json!("2026-11-02");
    let mut moved = memory_element("milestone", "Moved the office", plain, "u-p");
    moved["date"] = json!("2026-11-01");
    body["memory"] = json!([shipped, moved]);
    s.answer(&body);
    s.scribe("/scribe on");
    let report = pass_on(&s);
    assert_eq!(report["passes"][0]["memory_written"], 1, "{report}");
    assert_eq!(
        report["passes"][0]["memory_refused"],
        json!({"memory_not_stated": 1}),
        "{report}"
    );
    let block = memory_json(&s, &["show"]);
    assert_eq!(
        block["milestones"][0]["id"], "2026-11-02-shipped-scribe-1-0",
        "{block}"
    );
    assert_eq!(block["milestones"].as_array().unwrap().len(), 1);
}

#[test]
fn ack_marks_a_now_item_raised_once_per_session_and_forget_removes_it() {
    let s = memory_setup();
    s.scribe("/scribe on");
    pass_on(&s);
    let ack = |args: &[&str]| -> Value {
        let mut all = vec!["ack", WEDDING_STEM, "--session", ON, "--author", "opus"];
        all.extend_from_slice(args);
        memory_json(&s, &all)
    };
    let acked = ack(&[]);
    assert_eq!(acked["raised"]["session"], ON, "{acked}");
    let file = s.fx.root().join(format!("memory/now/{WEDDING_STEM}.md"));
    let raised = std::fs::read(&file).unwrap();
    ack(&[]);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        raised,
        "the same session changes nothing"
    );

    let before: Value = s.fx.check(
        Cli(&["search", "wedding", "--kind", "message", "--json"]),
        Json,
    );
    assert_eq!(before["total"], 1, "{before}");
    let unconfirmed = s.run_any(&["memory", "forget", WEDDING_STEM, "--author", "opus"], b"");
    assert_ne!(unconfirmed.status.code(), Some(0));
    assert!(file.exists());
    let profile = s.run_any(
        &[
            "memory",
            "forget",
            "profile",
            "--author",
            "opus",
            "--confirm",
            "--json",
        ],
        b"",
    );
    assert!(
        text(&profile.stderr).contains("usage_error"),
        "{}",
        text(&profile.stderr)
    );
    s.run(
        &[
            "memory",
            "forget",
            WEDDING_STEM,
            "--author",
            "opus",
            "--confirm",
            "--json",
        ],
        b"",
    );
    assert!(!file.exists());
    assert!(!memory_text(&s).contains("Sister's wedding"));
    // The quoted turn is gone from the messages too (§15.7), and the other
    // turn stays.
    let search = |terms: &str| -> Value {
        s.fx.check(Cli(&["search", terms, "--kind", "message", "--json"]), Json)
    };
    assert_eq!(search("wedding")["total"], 0, "{}", search("wedding"));
    assert_eq!(search("venue")["total"], 1, "{}", search("venue"));
    let db = std::fs::read(s.fx.root().join("scribe.db")).unwrap();
    let wal = std::fs::read(s.fx.root().join("scribe.db-wal")).unwrap_or_default();
    for bytes in [&db, &wal] {
        assert!(
            !String::from_utf8_lossy(bytes).contains("still on the seating"),
            "the forgotten turn's words are left in the database"
        );
    }
    let missing = s.run_any(&["memory", "show", WEDDING_STEM, "--json"], b"");
    assert!(
        text(&missing.stderr).contains("memory_not_found"),
        "{}",
        text(&missing.stderr)
    );
}

#[test]
fn the_session_start_page_carries_the_memory_block_in_interactive_sessions_only() {
    let s = memory_setup();
    s.scribe("/scribe on");
    pass_on(&s);

    let start = |entrypoint: &str| -> Value {
        let payload = json!({"session_id": NEVER, "transcript_path": s.transcript(NEVER),
            "cwd": s.cwd, "hook_event_name": "SessionStart", "source": "startup"});
        let out = s.hook_with_env(
            "session-start",
            &payload,
            "CLAUDE_CODE_ENTRYPOINT",
            entrypoint,
        );
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    };
    let interactive = start("cli");
    let page = interactive["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("{interactive}"));
    assert!(
        page.contains(&format!(
            "## Profile\n(none)\n## Now\n- {WEDDING_STEM} · due 2026-12-12 · Sister's wedding\n## Milestones\n(none)\n"
        )),
        "{page}"
    );
    assert!(page.contains("## Recent sessions"), "{page}");

    let headless = start("sdk-cli");
    assert_eq!(headless["hookSpecificOutput"], Value::Null, "{headless}");
}

#[test]
fn memory_show_and_brief_write_nothing() {
    let s = memory_setup();
    s.scribe("/scribe on");
    pass_on(&s);
    let before = store_snapshot(&s);
    for args in [
        &["memory", "show"][..],
        &["memory", "show", "--json"],
        &["memory", "show", WEDDING_STEM],
        &["memory", "rules"],
        &["brief"],
        &["brief", "--json"],
    ] {
        s.run(args, b"");
    }
    assert_eq!(store_snapshot(&s), before);
}

#[test]
fn a_newer_store_and_obsolete_memory_inputs_are_refused() {
    let s = memory_setup();
    s.scribe("/scribe on");
    pass_on(&s);
    let control = s.fx.root().join("control.json");
    let text_now = std::fs::read_to_string(&control).unwrap();
    let mut newer_stamp: Value = serde_json::from_str(&text_now).unwrap();
    assert_eq!(newer_stamp["format"], 31, "{text_now}");
    newer_stamp["format"] = json!(32);
    std::fs::write(&control, newer_stamp.to_string()).unwrap();
    let newer = s.run_any(&["memory", "refresh", "--json"], b"");
    assert_ne!(newer.status.code(), Some(0));
    assert!(
        text(&newer.stderr).contains("store_too_new"),
        "{}",
        text(&newer.stderr)
    );
    std::fs::write(&control, text_now).unwrap();

    // Obsolete `--page` flag is rejected through the normal CLI error path.
    let page_flag = s.run_any(&["memory", "show", "--page"], b"");
    assert_eq!(page_flag.status.code(), Some(2));
    assert!(
        text(&page_flag.stderr).contains("--page"),
        "{}",
        text(&page_flag.stderr)
    );

    // Obsolete `[memory]` and `instructions.memory_pairs` keys are refused as `config_invalid`.
    for (toml, key) in [
        ("[memory]\ndecay_days = 60\n", "memory"),
        ("[instructions]\nmemory_pairs = \"x\"\n", "memory_pairs"),
    ] {
        s.config(toml);
        let out = s.run_any(&["tag", "list", "--json"], b"");
        assert_eq!(out.status.code(), Some(1), "{toml}");
        let err = text(&out.stderr);
        assert!(err.contains("config_invalid"), "{toml}: {err}");
        assert!(err.contains(key), "{toml}: {err}");
    }

    // Supported `[instructions]` settings continue to load cleanly.
    s.config(
        "[instructions]\nsession = \"Keep {user}'s record.\"\nsession_header = \"Shorten for {user}.\"\nprofile = \"Emphasize {user}'s work.\"\ngroup_summary = \"Emphasize outcomes.\"\n",
    );
    s.run(&["tag", "list", "--json"], b"");
}

#[test]
fn memory_rules_prints_the_fixed_rules() {
    let s = Setup::new();
    let out = memory_json(&s, &["rules"]);
    let names: Vec<&str> = out["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["rule"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "max_per_pass",
            "max_pending",
            "now_keep_days",
            "milestone_lines",
            "profile_chars",
            "user_says"
        ]
    );
    assert!(
        out["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["origin"]["source"] == "shipped")
    );
}

#[test]
fn memory_refresh_writes_a_revision_and_has_nothing_new_on_a_second_run() {
    let s = memory_setup();
    s.config(
        "[user]\nname = \"Sam\"\n\n[instructions]\nprofile = \"Emphasize {user}'s systems work.\"\n",
    );
    s.scribe("/scribe on");
    pass_on(&s);
    let calls = s.prompts().len();

    // Deterministic `memory refresh` refuses without calling a model and points to Orbit.
    let needs_orbit = s.run_any(&["memory", "refresh", "--json"], b"");
    assert_eq!(needs_orbit.status.code(), Some(2));
    assert!(
        text(&needs_orbit.stderr).contains("orbit job run scribe-memory-refresh"),
        "{}",
        text(&needs_orbit.stderr)
    );
    assert_eq!(s.prompts().len(), calls, "no model call");

    // `memory prepare` returns bounded inputs carrying [instructions] and [user] name.
    let prepared = memory_json(&s, &["prepare"]);
    assert_eq!(prepared["nothing_new"], false, "{prepared}");
    assert_eq!(prepared["inputs"]["user_noun"], "Sam", "{prepared}");
    assert_eq!(
        prepared["inputs"]["instructions"], "Emphasize Sam's systems work.",
        "{prepared}"
    );
    assert_eq!(s.prompts().len(), calls, "prepare calls no model");

    // `memory apply` commits the fixed answer and writes revision 1.
    let apply_req = |prep: &Value, answer: Value, allow_retry: bool| {
        json!({
            "basis": prep["basis"],
            "cursor_to": prep["cursor_to"],
            "session_ids": prep["session_ids"],
            "backlog": prep["backlog"],
            "passed_stems": prep["passed_stems"],
            "answer": answer,
            "allow_retry": allow_retry,
        })
    };
    let applied = s.run(
        &["memory", "apply", "--json"],
        apply_req(&prepared, profile_answer("scribe, a memory store."), false)
            .to_string()
            .as_bytes(),
    );
    let refreshed: Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(refreshed["result"], "written", "{refreshed}");
    assert_eq!(refreshed["revision"], 1);
    assert_eq!(refreshed["sessions"], 1);
    assert_eq!(s.prompts().len(), calls, "apply calls no model");
    let profile = std::fs::read_to_string(s.fx.root().join("memory/profile.md")).unwrap();
    assert!(profile.contains("- scribe, a memory store."), "{profile}");
    let block = memory_json(&s, &["show"]);
    assert!(block["profile"]["as_of"].is_string(), "{block}");
    let revision = memory_json(&s, &["show", "--revision", "1"]);
    assert_eq!(revision["text"], profile);

    // Nothing new on a second run: both `memory refresh` and `memory prepare` short-circuit.
    let again = memory_json(&s, &["refresh"]);
    assert_eq!(again["result"], "nothing_new", "{again}");
    let prepared_again = memory_json(&s, &["prepare"]);
    assert_eq!(prepared_again["nothing_new"], true, "{prepared_again}");
    assert_eq!(s.prompts().len(), calls, "no model call");

    // A refused profile writes nothing: allow_retry returns retry+correction; final fails.
    std::fs::write(
        s.fx.root().join("memory/profile.md"),
        profile.replacen("## Pinned\n", "## Pinned\n- Call me Daniel.\n", 1),
    )
    .unwrap();
    let edited = memory_json(&s, &["prepare"]);
    assert_eq!(edited["nothing_new"], false, "{edited}");
    let retry_out = s.run(
        &["memory", "apply", "--json"],
        apply_req(&edited, profile_answer("orbit."), true)
            .to_string()
            .as_bytes(),
    );
    let retry: Value = serde_json::from_slice(&retry_out.stdout).unwrap();
    assert_eq!(retry["status"], "retry", "{retry}");
    assert_eq!(retry["code"], "profile_pinned", "{retry}");
    assert!(
        retry["correction"]
            .as_str()
            .is_some_and(|c| c.contains("Pinned")),
        "{retry}"
    );
    let refused = s.run_any(
        &["memory", "apply", "--json"],
        apply_req(&edited, profile_answer("orbit."), false)
            .to_string()
            .as_bytes(),
    );
    assert_ne!(refused.status.code(), Some(0));
    assert!(
        text(&refused.stdout).contains("profile_pinned")
            || text(&refused.stderr).contains("profile_pinned")
    );
    assert!(
        std::fs::read_to_string(s.fx.root().join("memory/profile.md"))
            .unwrap()
            .contains("- Call me Daniel.")
    );
}

#[test]
fn a_format_24_memory_item_is_not_migrated_and_the_first_memory_write_archives_it() {
    let s = Setup::new();
    // Without a scribe.db the leftover would make the root a refused file store.
    support::seed_records(&s.fx.root(), &[]);
    let dir = s.fx.root().join("memory");
    s.fx.create_private_dir_all(&dir);
    let path = dir.join("m-1.md");
    std::fs::write(
        &path,
        format!(
            "---\nformat: 24\nkind: memory\nid: m-1\nsection: now\nstatus: active\n\
             statement: Sister's wedding on 2026-12-12.\nproject: null\n\
             event_date: 2026-12-12\nsensitive: false\nsensitive_kind: null\nacknowledged: null\n\
             retirement_reason: null\npromoted_from: null\npromotion: null\nmerged_into: null\n\
             superseded_by: null\nunrelated_to: []\npreference_proposal: null\n\
             author:\n  kind: scribe\n  name: session-pass\nupdated_by:\n  kind: scribe\n  name: session-pass\n\
             created: 2026-11-03T06:00:00Z\nupdated: 2026-11-03T06:00:00Z\noccurrences:\n\
             - n: 1\n  date: 2026-11-03T06:00:00Z\n  session: {ON}\n  message: u-w\n  host: host\n  project: scribe\n\
             \x20 quote: {WEDDING_QUOTE}\n  author:\n    kind: scribe\n    name: session-pass\n---\n"
        ),
    )
    .unwrap();
    make_private(&path);
    // A read leaves it and says so.
    let shown = s.run(&["memory", "show"], b"");
    assert!(path.exists());
    assert!(
        text(&shown.stderr).contains("1 format-24 memory item(s) not read"),
        "{}",
        text(&shown.stderr)
    );
    // The first deterministic refresh archives it with no model call.
    s.run(&["memory", "refresh", "--json"], b"");
    assert!(!path.exists());
    let block = memory_json(&s, &["show"]);
    assert_eq!(block["now"], json!([]), "{block}");
    let archived = rusqlite::Connection::open(s.fx.root().join("scribe.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM archive_records", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    assert!(archived >= 1, "the item is archived, not dropped");
}
