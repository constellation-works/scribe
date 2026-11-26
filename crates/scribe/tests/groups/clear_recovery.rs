#[test]
fn missing_start_transcript_recovers_at_first_prompt_once() {
    let f = Fixture::new();
    let path = setup_missing_start(&f);
    write_cleared_b_transcript(&f);
    let payload = json!({"session_id":B,"transcript_path":path,"cwd":f.home(),"prompt":"continue from handoff"});
    let result = hook(
        &f,
        &["hook", "user-prompt-submit"],
        payload.clone(),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    let reply: Value = serde_json::from_slice(&result.stdout).unwrap();
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.starts_with("## State\nReady") && context.contains("inherited from"),
        "{reply}"
    );
    assert_eq!(
        show_group(&f, B, "g-1")["seats"][0]["transcripts"],
        json!([A, B])
    );
    let again = hook(
        &f,
        &["hook", "user-prompt-submit"],
        payload,
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    assert!(again.stdout.is_empty(), "{}", text(&again.stdout));
}

#[test]
fn blocked_first_prompt_preserves_deferred_ancestry_for_next_prompt() {
    let f = Fixture::new();
    let path = setup_missing_start(&f);
    write_cleared_b_transcript(&f);

    let status = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id":B,"transcript_path":path,"cwd":f.home(),"prompt":"/scribe status"}),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    let status_reply: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_reply["decision"], "block", "{status_reply}");
    assert!(
        status_reply.get("hookSpecificOutput").is_none(),
        "{status_reply}"
    );
    assert_eq!(
        show_group(&f, B, "g-1")["seats"][0]["transcripts"],
        json!([A])
    );

    // An explicit `/scribe on` (also blocked) leaves the deferred check intact
    // and is preserved once handoff inheritance runs.
    let on = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id":B,"transcript_path":path,"cwd":f.home(),"prompt":"/scribe on"}),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    let on_reply: Value = serde_json::from_slice(&on.stdout).unwrap();
    assert_eq!(on_reply["decision"], "block", "{on_reply}");
    assert_eq!(
        show_group(&f, B, "g-1")["seats"][0]["transcripts"],
        json!([A])
    );

    let payload = json!({"session_id":B,"transcript_path":path,"cwd":f.home(),"prompt":"continue from handoff"});
    let result = hook(
        &f,
        &["hook", "user-prompt-submit"],
        payload.clone(),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    let reply: Value = serde_json::from_slice(&result.stdout).unwrap();
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.starts_with("## State\nReady") && context.contains("inherited from"),
        "{reply}"
    );
    assert_eq!(
        show_group(&f, B, "g-1")["seats"][0]["transcripts"],
        json!([A, B])
    );
    assert_eq!(
        support::control(&f.root())["sessions"][B]["on"],
        json!(true)
    );

    let again = hook(
        &f,
        &["hook", "user-prompt-submit"],
        payload,
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    assert!(again.stdout.is_empty(), "{}", text(&again.stdout));
}

#[test]
fn explicit_off_before_deferred_ancestry_preserves_opt_out() {
    let f = Fixture::new();
    let path = setup_missing_start(&f);
    let store = scribe_core::Store::open(f.root());
    let parent = A.parse().unwrap();
    let child = B.parse().unwrap();
    store
        .update_control(|state| {
            scribe_core::control::switch(
                state,
                &parent,
                &scribe_core::control::Switch::On,
                time::OffsetDateTime::now_utc(),
            )
        })
        .unwrap();
    write_cleared_b_transcript(&f);

    let off = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id": B, "transcript_path": path, "cwd": f.home(), "prompt": "/scribe off"}),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    let reply: Value = serde_json::from_slice(&off.stdout).unwrap();
    assert_eq!(reply["decision"], "block", "{reply}");
    let result = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id": B, "transcript_path": path, "cwd": f.home(), "prompt": "continue from handoff"}),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    let reply: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("Carry on."),
        "{reply}"
    );
    let state = store.control().unwrap();
    let control = state.get(&child).unwrap();
    assert!(!control.on && !control.due && !control.final_pass);
    assert!(
        !scribe_core::control::due(&state)
            .iter()
            .any(|(id, _)| id == &child)
    );
    assert!(
        control
            .pending_links
            .iter()
            .any(|link| link.kind == scribe_core::LinkType::Continues && link.session == parent)
    );
    assert_eq!(
        show_group(&f, B, "g-1")["seats"][0]["transcripts"],
        json!([A, B])
    );
}

#[test]
fn deferred_ancestry_survives_while_transcript_remains_missing() {
    let f = Fixture::new();
    let path = setup_missing_start(&f);
    let payload =
        json!({"session_id":B,"transcript_path":path,"cwd":f.home(),"prompt":"first typed prompt"});

    // Transcript still absent: prompt is not blocked, check is not consumed.
    let early = hook(
        &f,
        &["hook", "user-prompt-submit"],
        payload.clone(),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    assert!(early.stdout.is_empty(), "{}", text(&early.stdout));
    assert_eq!(
        show_group(&f, B, "g-1")["seats"][0]["transcripts"],
        json!([A])
    );

    // Once the transcript exists, the next typed prompt consumes the deferred check once.
    write_cleared_b_transcript(&f);
    let recovered = hook(
        &f,
        &["hook", "user-prompt-submit"],
        payload.clone(),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    let reply: Value = serde_json::from_slice(&recovered.stdout).unwrap();
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.starts_with("## State\nReady") && context.contains("inherited from"),
        "{reply}"
    );
    assert_eq!(
        show_group(&f, B, "g-1")["seats"][0]["transcripts"],
        json!([A, B])
    );

    let again = hook(
        &f,
        &["hook", "user-prompt-submit"],
        payload,
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    assert!(again.stdout.is_empty(), "{}", text(&again.stdout));
}

#[test]
fn session_start_marker_write_failure_keeps_the_brief_and_reports_the_failure() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &["--role", "reviewer"], 0);
    // A closed group with a summary gives the project brief a block to keep.
    f.check(
        with_env(
            &[
                "group",
                "close",
                "g-1",
                "--summary",
                "Retained group summary.",
                "--json",
            ],
            "CLAUDE_CODE_SESSION_ID",
            A,
        ),
        Success,
    );
    support::sql(
        &f.root(),
        "CREATE TRIGGER abort_defer_ancestry BEFORE INSERT ON deferred_ancestry
         BEGIN
             SELECT RAISE(ABORT, 'injected ancestry failure');
         END;",
    );

    let path = transcript_path(&f, B);
    let project = f.elsewhere("scribe");
    f.create_private_dir_all(&project);
    let start = session_start_with_cwd(
        &f,
        B,
        Some(&path),
        "startup",
        "claude-desktop",
        Some(&project),
    );
    let message = start["systemMessage"].as_str().unwrap_or_default();
    assert!(
        message.contains("could not record the deferred ancestry check"),
        "{start}"
    );
    let context = start["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(context.contains("scribe group join"), "{start}");
    assert!(
        !context.contains("ancestry checked at the first prompt"),
        "{start}"
    );
    assert!(context.contains("## Last group"), "{start}");
    assert!(context.contains("Retained group summary."), "{start}");
    assert_eq!(deferred_rows(&f), 0);

    let line = support::audit_lines(&f.root()).pop().unwrap();
    assert_eq!(line["command"], "hook session-start");
    assert_eq!(line["status"], "failure");
    assert_eq!(line["target"], format!("session:{B}"));
    assert_eq!(line["error_code"], "store_database");
}

#[test]
fn first_prompt_ancestry_failure_keeps_the_prompts_own_writes_and_replies() {
    let f = Fixture::new();
    let path = setup_missing_start(&f);
    // B is a member, so this prompt also captures its typed turn for the
    // group, and its settling line retires the handoff A wrote.
    transcript(&f, B, &[row(B, "typed", "user", "Keep groupretireorchid.")]);
    join_as(&f, B, "g-1", "tester", &[], 0);
    support::sql(
        &f.root(),
        "CREATE TRIGGER abort_take_ancestry BEFORE DELETE ON deferred_ancestry
         BEGIN
             SELECT RAISE(ABORT, 'injected ancestry failure');
         END;",
    );
    assert_eq!(deferred_rows(&f), 1);

    let prompt = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({
            "session_id": B,
            "transcript_path": path,
            "cwd": f.home(),
            "prompt": "#scribe retire handoff-1\nKeep groupretireorchid."
        }),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    let reply: Value = serde_json::from_slice(&prompt.stdout).unwrap();
    let message = reply["systemMessage"].as_str().unwrap_or_default();
    assert!(message.contains("retired handoff-1"), "{reply}");
    assert!(
        message.contains("could not check this session's ancestry"),
        "{reply}"
    );
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(context.contains("retired handoff-1"), "{reply}");
    assert!(context.contains("relay this to the user"), "{reply}");

    // The settling write and the group capture of this prompt are kept.
    assert_eq!(
        invoke(&f, A, &["entry", "show", "handoff-1"], 0)["status"],
        "retired"
    );
    assert_eq!(
        invoke(
            &f,
            A,
            &[
                "search",
                "groupretireorchid",
                "--kind",
                "message",
                "--group",
                "g-1"
            ],
            0
        )["total"],
        1
    );
    // The check was not consumed, and the failure is audited with its code.
    assert_eq!(deferred_rows(&f), 1);
    let line = support::audit_lines(&f.root()).pop().unwrap();
    assert_eq!(line["command"], "hook user-prompt-submit");
    assert_eq!(line["status"], "failure");
    assert_eq!(line["target"], format!("session:{B}"));
    assert_eq!(line["error_code"], "store_database");

    // Once the store writes again, the next typed prompt consumes the check.
    support::sql(&f.root(), "DROP TRIGGER abort_take_ancestry;");
    let next = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({
            "session_id": B,
            "transcript_path": path,
            "cwd": f.home(),
            "prompt": "continue"
        }),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    assert!(next.stdout.is_empty(), "{}", text(&next.stdout));
    assert_eq!(deferred_rows(&f), 0);
}

#[test]
fn missing_or_non_member_predecessor_joins_nothing_and_tells_session_to_rerun_join() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    let before = support::audit_lines(&f.root());

    // Missing derived-from line (and missing transcript file).
    let no_ancestry = transcript(&f, B, &[row(B, "u", "user", "fresh")]);
    for (source, path) in [
        ("startup", Some(&no_ancestry)),
        ("clear", Some(&no_ancestry)),
        ("startup", None),
    ] {
        let reply = session_start(&f, B, path, source, "claude-desktop");
        let context = reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or_default();
        assert!(context.contains("scribe group join"), "{reply}");
    }

    // Predecessor line naming a session in no group.
    let non_member = cleared_transcript(&f, C, A, &[D]);
    let reply = session_start(&f, C, Some(&non_member), "startup", "claude-desktop");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(context.contains("scribe group join"), "{reply}");

    // Predecessor line naming an earlier holder already replaced on the seat.
    join(&f, D, "g-1", &["--replace"], 0);
    let replaced_before = support::audit_lines(&f.root());
    let stale_child = cleared_transcript(&f, "55555555-5555-4555-8555-555555555555", C, &[A]);
    let stale_reply = session_start(
        &f,
        "55555555-5555-4555-8555-555555555555",
        Some(&stale_child),
        "startup",
        "claude-desktop",
    );
    assert!(
        stale_reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or_default()
            .contains("scribe group join"),
        "{stale_reply}"
    );
    assert_eq!(support::audit_lines(&f.root()), replaced_before);
    assert_eq!(
        show_group(&f, A, "g-1")["seats"][0]["transcripts"],
        json!([A, D])
    );
    assert!(before.len() < replaced_before.len());
}

#[test]
fn group_join_accepts_cleared_predecessor_without_replace_and_refuses_others() {
    let f = Fixture::new();
    // Recent timestamp (year 2090): A has NOT ended under the 24-hour rule.
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &["--role", "reviewer"], 0);

    // B's transcript names A as its cleared predecessor -> join succeeds without --replace.
    cleared_transcript(&f, B, C, &[A]);
    let joined = join(&f, B, "g-1", &[], 0);
    assert_eq!(joined["seats"][0]["role"], "reviewer");
    assert_eq!(joined["seats"][0]["transcripts"], json!([A, B]));

    // C has no predecessor line -> refused with seat_occupied without --replace.
    transcript(&f, C, &[row(C, "u", "user", "unrelated")]);
    assert_eq!(join(&f, C, "g-1", &[], 1)["code"], "seat_occupied");

    // D's predecessor is A (no longer the seat's newest transcript B) -> refused with seat_occupied.
    cleared_transcript(&f, D, C, &[A]);
    assert_eq!(join(&f, D, "g-1", &[], 1)["code"], "seat_occupied");
}

#[test]
fn restarting_a_replaced_session_names_no_seat_and_writes_nothing() {
    let f = Fixture::new();
    for id in [A, B, C] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    open(&f);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    join(&f, B, "g-1", &["--replace"], 0);
    join(&f, C, "g-2", &[], 0);
    let before = support::audit_lines(&f.root());
    assert_eq!(member_rows(&f), 3);

    // A restarts on its own transcript with no predecessor line: B holds
    // the seat now, so A must not be reported as holding it.
    let a_path = transcript_path(&f, A);
    let reply = session_start(&f, A, Some(&a_path), "startup", "claude-desktop");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(!context.contains("holds seat"), "{reply}");
    assert!(context.contains("was replaced"), "{reply}");
    assert!(!context.contains("scribe group join"), "{reply}");

    // A's cleared predecessor C holds an open seat in g-2. A has a historical
    // row, so it must not be inserted again (the session is unique).
    let cleared = cleared_transcript(&f, A, D, &[C]);
    let reply = session_start(&f, A, Some(&cleared), "startup", "claude-desktop");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(!context.contains("holds seat"), "{reply}");
    assert!(context.contains("was replaced"), "{reply}");
    assert!(!context.contains("scribe group join"), "{reply}");
    assert_eq!(member_rows(&f), 3);
    assert_eq!(support::audit_lines(&f.root()), before);
    assert_eq!(
        show_group(&f, A, "g-1")["seats"][0]["transcripts"],
        json!([A, B])
    );
    assert_eq!(
        show_group(&f, A, "g-2")["seats"][0]["transcripts"],
        json!([C])
    );

    // B restarting still identifies itself as the holder, with no write.
    let b_path = transcript_path(&f, B);
    let reply = session_start(&f, B, Some(&b_path), "startup", "claude-desktop");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(context.contains("holds seat editor"), "{reply}");
    assert_eq!(member_rows(&f), 3);
    assert_eq!(support::audit_lines(&f.root()), before);
}

#[test]
fn replaced_member_stores_no_new_turns_and_its_history_stays_readable() {
    let f = Fixture::new();
    let a_before = [
        row(A, "u", "user", QUOTE),
        row(A, "before", "user", "beforeorchid typed by A"),
    ];
    let a_path = transcript(&f, A, &a_before);
    let b_path = transcript(&f, B, &[row(B, "u", "user", "joining now")]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    let fake = f.fake_program("claude", "#!/bin/sh\nexit 1\n");
    let env = [("PATH", fake.to_str().unwrap())];
    stop(&f, A, &a_path, &env);

    join(&f, B, "g-1", &["--replace"], 0);
    // A keeps typing on its transcript after B holds the seat.
    let mut a_after = a_before.to_vec();
    a_after.push(row(A, "after", "user", "afterreplaceorchid typed by A"));
    transcript(&f, A, &a_after);
    let before_replaced_stop = support::audit_lines(&f.root());
    stop(&f, A, &a_path, &env);
    assert_eq!(support::audit_lines(&f.root()), before_replaced_stop);
    transcript(
        &f,
        B,
        &[
            row(B, "u", "user", "joining now"),
            row(B, "b", "user", "bcaptureorchid typed by B"),
        ],
    );
    stop(&f, B, &b_path, &env);

    let search = |session: &str, term: &str| {
        invoke(
            &f,
            session,
            &["search", term, "--kind", "message", "--group", "g-1"],
            0,
        )
    };
    assert_eq!(search(B, "afterreplaceorchid")["total"], 0);
    let history = search(B, "beforeorchid");
    assert_eq!(history["total"], 1);
    assert_eq!(history["hits"][0]["cite"], format!("session:{A}"));
    let captured = search(B, "bcaptureorchid");
    assert_eq!(captured["total"], 1);
    assert_eq!(captured["hits"][0]["cite"], format!("session:{B}"));
}

#[test]
fn a_restored_seat_keeps_turns_typed_during_displacement_out_of_the_group() {
    let f = Fixture::new();
    let a_before = [
        row(A, "u", "user", QUOTE),
        row(A, "before", "user", "beforeorchid typed by A"),
    ];
    let a_path = transcript(&f, A, &a_before);
    transcript(&f, B, &[row(B, "u", "user", "joining now")]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    let fake = f.fake_program("claude", "#!/bin/sh\nexit 1\n");
    let env = [("PATH", fake.to_str().unwrap())];
    stop(&f, A, &a_path, &env);

    join(&f, B, "g-1", &["--replace"], 0);
    let mut a_turns = a_before.to_vec();
    a_turns.push(row(A, "during", "user", "duringdisplaceorchid typed by A"));
    transcript(&f, A, &a_turns);
    stop(&f, A, &a_path, &env);

    // B leaves: A holds the seat again and its next hooks must not backfill.
    invoke(&f, B, &["group", "leave"], 0);
    stop(&f, A, &a_path, &env);
    a_turns.push(row(A, "after", "user", "afterrestoreorchid typed by A"));
    transcript(&f, A, &a_turns);
    stop(&f, A, &a_path, &env);

    let search = |term: &str| {
        invoke(
            &f,
            A,
            &["search", term, "--kind", "message", "--group", "g-1"],
            0,
        )
    };
    assert_eq!(search("duringdisplaceorchid")["total"], 0);
    assert_eq!(search("beforeorchid")["total"], 1);
    assert_eq!(search("afterrestoreorchid")["total"], 1);
}
