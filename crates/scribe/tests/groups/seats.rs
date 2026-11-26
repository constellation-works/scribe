#[test]
fn repeated_handoff_keeps_group_seat_newest() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    for text in ["First version.", "Second version."] {
        invoke(
            &f,
            A,
            &[
                "entry",
                "add",
                &format!("{HANDOFF}\n{text}"),
                "--tag",
                "handoff",
                "--author",
                "aide",
                "--related",
                &format!("session:{A}"),
                "--quote",
                QUOTE,
            ],
            0,
        );
    }
    let group = show_group(&f, A, "g-1");
    assert_eq!(group["seats"][0]["newest_handoff"], "handoff-2");
    assert_eq!(
        invoke(&f, A, &["entry", "show", "handoff-1"], 0)["superseded_by"],
        "handoff-2"
    );
}

#[test]
fn seat_chains_replacement_handoffs_and_mistaken_joins() {
    let f = Fixture::new();
    for id in [A, B] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    open(&f);
    open(&f);
    join(&f, A, "g-1", &["--role", "copy editor"], 0);
    join(&f, A, "g-1", &[], 0);
    assert_eq!(
        join(&f, A, "g-2", &["--replace"], 1)["code"],
        "group_membership"
    );
    assert_eq!(join(&f, B, "g-1", &[], 1)["code"], "seat_occupied");
    let handoff = |session| {
        invoke(
            &f,
            session,
            &[
                "entry",
                "add",
                &format!("{HANDOFF}\nSession {session}"),
                "--tag",
                "handoff",
                "--author",
                "aide",
                "--related",
                &format!("session:{session}"),
                "--quote",
                QUOTE,
            ],
            0,
        )["id"]
            .clone()
    };
    let first = handoff(A);
    let group = join(&f, B, "g-1", &["--replace"], 0);
    assert_eq!(
        group["seats"],
        json!([{"name":"editor","role":"copy editor","transcripts":[A,B],"newest_handoff":first}])
    );
    let second = handoff(B);
    let before = support::audit_lines(&f.root());
    assert!(before.iter().any(|r| r["command"] == "group join"
        && r["author"] == "aide"
        && r["error_code"] == "seat_occupied"));
    let shown = show_group(&f, A, "g-1");
    let list = invoke(&f, A, &["group", "list"], 0);
    assert_eq!(list["items"][1], shown);
    assert_eq!(shown["seats"][0]["newest_handoff"], second);
    assert_eq!(support::audit_lines(&f.root()), before);
    // Historical entries may have subsecond timestamps. Pick by the same
    // instant and numeric id ordering as session show, including across seats' holders.
    support::sql(&f.root(), "UPDATE entries SET created = CASE n
        WHEN 1 THEN '2026-11-22T19:00:00.000002Z' ELSE '2026-11-22T19:00:00.000001Z' END WHERE tag='handoff';");
    assert_eq!(
        show_group(&f, A, "g-1")["seats"][0]["newest_handoff"],
        first
    );
    let after = invoke(&f, B, &["group", "leave"], 0);
    assert_eq!(after["seats"][0]["transcripts"], json!([A]));
    assert_eq!(after["seats"][0]["newest_handoff"], first);
    join(&f, B, "g-2", &[], 0);
    assert_eq!(invoke(&f, A, &["group", "leave"], 0)["seats"], json!([]));
    assert_eq!(
        invoke(&f, C, &["group", "leave"], 1)["code"],
        "group_membership"
    );
}

#[test]
fn group_show_renders_cataloged_session_aliases() {
    // Seat sessions with no catalog entry still print as their shortest unique
    // A neighboring handoff and both seat members exercise cataloged aliases.
    const CATALOGED: &str = "11111111-9999-4999-8999-999999999999";
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    transcript(&f, B, &[row(B, "u", "user", QUOTE)]);
    transcript(&f, CATALOGED, &[row(CATALOGED, "u", "user", QUOTE)]);
    // A handoff naming the sibling makes the store know it without a seat.
    invoke(
        &f,
        CATALOGED,
        &[
            "entry",
            "add",
            &format!("{HANDOFF}\nSession {CATALOGED}"),
            "--tag",
            "handoff",
            "--author",
            "aide",
            "--related",
            &format!("session:{CATALOGED}"),
            "--quote",
            QUOTE,
        ],
        0,
    );
    open(&f);
    join(&f, A, "g-1", &["--role", "reviewer"], 0);
    join_as(&f, B, "g-1", "tester", &["--role", "tester"], 0);

    let table = f.check(
        with_env(
            &["group", "show", "g-1", "--format", "table"],
            "CLAUDE_CODE_SESSION_ID",
            A,
        ),
        Success,
    );
    let shown = text(&table.stdout);
    let seats = shown.lines().find(|l| l.starts_with("seats")).unwrap();
    assert!(seats.contains("s-1 [handoff"), "{shown}");
    assert!(seats.contains("s-3 [handoff"), "{shown}");
    assert!(!seats.contains(A) && !seats.contains(B), "{shown}");

    let plain = f.check(&["show", "g-1"], Text);
    assert!(plain.contains("s-1") && plain.contains("s-3"), "{plain}");
    assert_eq!(
        show_group(&f, A, "g-1")["seats"],
        json!([
            {"name":"editor","role":"reviewer","transcripts":[A],"newest_handoff":null},
            {"name":"tester","role":"tester","transcripts":[B],"newest_handoff":null}
        ])
    );
}

#[test]
fn close_evidence_is_bound_to_the_prepared_group_when_a_slug_is_reused() {
    let f = Fixture::new();
    let prompt = "/scribe group close-reuse";
    let path = transcript(&f, A, &[row(A, "typed", "user", prompt)]);
    let open_project = || {
        hook(
            &f,
            &["hook", "user-prompt-submit"],
            json!({"session_id": A, "transcript_path": path, "cwd": f.home(), "prompt": prompt}),
            &[],
        );
        invoke(&f, A, &["group", "leave"], 0);
    };
    open_project();
    let input = json!({"group":"close-reuse", "session":A, "provider":"claude-code"});
    let prepared = close_plugin(&f, "group-close-prepare", input.clone());
    assert_eq!(prepared["output"]["id"], "g-1", "{prepared}");
    invoke(&f, A, &["group", "close", "g-1", "--no-summary"], 0);
    open_project();
    let replacement = close_plugin(&f, "group-close-prepare", input.clone());
    assert_eq!(replacement["output"]["id"], "g-2", "{replacement}");
    assert_eq!(
        prepared["output"]["evidence"],
        replacement["output"]["evidence"]
    );
    assert_eq!(
        prepared["output"]["related"],
        replacement["output"]["related"]
    );
    assert_ne!(
        prepared["output"]["evidence_sha256"],
        replacement["output"]["evidence_sha256"]
    );

    let mut apply = input;
    apply["summary"] = json!("Summary for the original group");
    apply["evidence_sha256"] = prepared["output"]["evidence_sha256"].clone();
    let rejected = close_plugin(&f, "group-close-apply", apply.clone());
    assert_eq!(
        rejected["error"]["code"], "group_evidence_changed",
        "{rejected}"
    );
    apply["group"] = prepared["output"]["id"].clone();
    let original = close_plugin(&f, "group-close-apply", apply);
    assert_eq!(original["output"]["group"]["id"], "g-1", "{original}");
    assert_eq!(original["output"]["changed"], false, "{original}");
    assert!(show_group(&f, A, "g-2")["closed"].is_null());

    // Both shipped jobs must pass the prepared id to apply after drafting.
    for job in [
        include_str!("../../../../plugin/.orbit-plugin/jobs/group_close.yaml"),
        include_str!("../../../../plugin/orbit-codex/.orbit-plugin/jobs/group_close.yaml"),
    ] {
        let apply_step = job.split("    - id: apply\n").nth(1).unwrap();
        assert!(apply_step.contains("group: \"{{ steps.prepare.output.id }}\""));
    }
}

#[test]
fn project_group_join_preserves_existing_role_when_role_is_omitted() {
    let f = Fixture::new();
    let prompt = "/scribe group role-preservation";
    let transcript = transcript(&f, A, &[row(A, "typed", "user", prompt)]);
    hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({
            "session_id": A,
            "transcript_path": transcript,
            "cwd": f.home(),
            "prompt": prompt
        }),
        &[],
    );
    let opened = show_group(&f, A, "role-preservation");
    assert_eq!(opened["seats"][0]["role"], "member");

    let custom = join_as(
        &f,
        B,
        "role-preservation",
        "editor",
        &["--role", "copy editor"],
        0,
    );
    let custom_editor = custom["seats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|seat| seat["name"] == "editor")
        .unwrap();
    assert_eq!(custom_editor["role"], "copy editor");

    // Retrying the same session with no --role keeps the custom role.
    let retry = join(&f, B, "role-preservation", &[], 0);
    let editor = retry["seats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|seat| seat["name"] == "editor")
        .unwrap();
    assert_eq!(editor["role"], "copy editor");
    assert_eq!(editor["transcripts"], json!([B]));

    // A new project-group seat without --role still defaults to member.
    let new_seat = join_as(&f, D, "role-preservation", "observer", &[], 0);
    let observer = new_seat["seats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|seat| seat["name"] == "observer")
        .unwrap();
    assert_eq!(observer["role"], "member");

    // An explicit incompatible role is still refused, while an authorized
    // replacement that omits --role inherits the existing role.
    assert_eq!(
        join(
            &f,
            C,
            "role-preservation",
            &["--replace", "--role", "member"],
            1
        )["code"],
        "group_membership"
    );
    let replacement = join(&f, C, "role-preservation", &["--replace"], 0);
    let editor = replacement["seats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|seat| seat["name"] == "editor")
        .unwrap();
    assert_eq!(editor["role"], "copy editor");
    assert_eq!(editor["transcripts"], json!([B, C]));
}

#[test]
fn joins_after_an_ended_transcript_and_keeps_unverifiable_seats_occupied() {
    let f = Fixture::new();
    let mut message = row(A, "u", "user", QUOTE);
    message["timestamp"] = json!("2000-01-01T00:00:00Z");
    transcript(&f, A, &[message]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    let group = join(&f, B, "g-1", &[], 0);
    assert_eq!(group["seats"][0]["transcripts"], json!([A, B]));
    assert_eq!(join(&f, C, "g-1", &[], 1)["code"], "seat_occupied");
    let export = f.elsewhere("dump");
    f.check(
        Cli(&["export", export.to_str().unwrap(), "--as", "jsonl"]),
        Success,
    );
    let members = std::fs::read_to_string(export.join("group_members.jsonl")).unwrap();
    assert_eq!(members.lines().count(), 2);
    assert!(export.join("groups.jsonl").is_file() && export.join("group_seats.jsonl").is_file());
}

#[test]
fn old_schema_reads_empty_without_migration_then_upgrades_on_open() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f); // creates scribe.db
    support::remove_group_capture_schema(&f.root());
    support::sql(
        &f.root(),
        "DROP TABLE group_members; DROP TABLE group_seats; DROP TABLE groups; PRAGMA user_version=8;",
    );
    assert_eq!(invoke(&f, A, &["group", "list"], 0)["total"], 0);
    let conn = rusqlite::Connection::open(f.root().join("scribe.db")).unwrap();
    let version = || {
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(version(), 8);
    open(&f);
    assert_eq!(version(), 20);
}

#[test]
fn existing_members_start_group_capture_at_the_schema_upgrade() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    support::remove_group_capture_schema(&f.root());
    support::sql(&f.root(), "PRAGMA user_version = 10");
    let old = show_group(&f, A, "g-1");
    assert!(old["summary"].is_null());
    assert_eq!(old["seats"][0]["transcripts"], json!([B]));
    let report = invoke(&f, A, &["brief", "--project", "scribe"], 0);
    assert_eq!(report["last_group"]["id"], "g-1");

    open(&f);
    let mut old_turn = row(B, "old", "user", "beforeupgradejuniper");
    old_turn["timestamp"] = json!("2000-01-01T00:00:00Z");
    let path = transcript(
        &f,
        B,
        &[old_turn, row(B, "new", "user", "afterupgradejuniper")],
    );
    stop(&f, B, &path, &[]);
    let messages = |word| {
        invoke(
            &f,
            A,
            &["search", word, "--kind", "message", "--group", "g-1"],
            0,
        )
    };
    assert_eq!(messages("beforeupgradejuniper")["total"], 0);
    assert_eq!(messages("afterupgradejuniper")["total"], 1);
}

#[test]
fn desktop_clear_inherits_group_and_seat_without_typing_or_rerunning() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &["--role", "reviewer"], 0);

    // Non-interactive, resume, compact, and mismatched transcripts do not inherit.
    let child_b = cleared_transcript(&f, B, C, &[A]);
    for (source, entrypoint) in [
        ("startup", "sdk-cli"),
        ("resume", "claude-desktop"),
        ("compact", "claude-desktop"),
    ] {
        session_start(&f, B, Some(&child_b), source, entrypoint);
    }
    let mismatched = transcript_path(&f, C);
    std::fs::copy(&child_b, &mismatched).unwrap();
    session_start(&f, B, Some(&mismatched), "startup", "claude-desktop");
    assert_eq!(
        show_group(&f, A, "g-1")["seats"][0]["transcripts"],
        json!([A])
    );

    // Desktop clear (`source: startup`) appends B to A's seat with nothing typed.
    let before = support::audit_lines(&f.root()).len();
    let reply = session_start(&f, B, Some(&child_b), "startup", "claude-desktop");
    assert!(reply.get("systemMessage").is_none(), "{reply}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("editor") && context.contains("reviewer") && context.contains("g-1"),
        "{context}"
    );
    assert!(!context.contains("scribe group join"), "{context}");
    assert_eq!(
        show_group(&f, B, "g-1")["seats"][0]["transcripts"],
        json!([A, B])
    );
    let audit = support::audit_lines(&f.root());
    assert_eq!(audit.len(), before + 1);
    assert_eq!(audit[before]["command"], "hook session-start");
    assert_eq!(audit[before]["target"], "g-1");
    assert!(audit[before]["error_code"].is_null());
    assert_eq!(invoke(&f, B, &["session", "list"], 0)["total"], 0);

    // Repeating SessionStart for B is idempotent and writes no second audit line.
    let repeat = session_start(&f, B, Some(&child_b), "startup", "claude-desktop");
    assert_eq!(support::audit_lines(&f.root()).len(), before + 1);
    assert!(
        !repeat["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("scribe group join")
    );

    // A second clear (C cleared from B, still carrying A as launch ancestor) appends C too.
    let child_c = cleared_transcript(&f, C, A, &[B]);
    session_start(&f, C, Some(&child_c), "startup", "claude-desktop");
    assert_eq!(
        show_group(&f, C, "g-1")["seats"][0]["transcripts"],
        json!([A, B, C])
    );
}

#[test]
fn desktop_clear_with_continue_handoff_prepends_handoff_and_inherits_seat() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &["--role", "reviewer"], 0);
    invoke(
        &f,
        A,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "aide",
            "--related",
            &format!("session:{A}"),
            "--mode",
            "continue",
            "--quote",
            QUOTE,
            &format!("{HANDOFF}\nCarry on."),
        ],
        0,
    );
    let child_b = cleared_transcript(&f, B, C, &[A]);
    let reply = session_start(&f, B, Some(&child_b), "startup", "claude-desktop");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.starts_with("## State\nReady"), "{context}");
    assert!(
        context.contains("editor") && context.contains("g-1"),
        "{context}"
    );
    assert_eq!(
        show_group(&f, B, "g-1")["seats"][0]["transcripts"],
        json!([A, B])
    );

    let store = scribe_core::Store::open(f.root());
    let parent = A.parse().unwrap();
    let successor = B.parse().unwrap();
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
    let now = time::OffsetDateTime::now_utc();
    store
        .link_session(
            &successor,
            &scribe_core::SessionLink {
                kind: scribe_core::LinkType::Continues,
                session: parent,
            },
            true,
            now,
        )
        .unwrap();
    let archive = f.root().join("pre-sqlite-session-forget-test");
    f.check(Cli(&["export", archive.to_str().unwrap()]), Success);
    let archived_handoff = archive.join("entries/handoff/handoff-1.md");
    assert!(archived_handoff.is_file());
    let before = std::fs::read(&child_b).unwrap();
    let reply = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id": B, "transcript_path": child_b, "cwd": f.home(),
            "prompt": format!("#scribe forget {A}")}),
        &[],
    );
    assert!(text(&reply.stdout).contains("provider transcript was left untouched"));
    assert_eq!(std::fs::read(&child_b).unwrap(), before);
    assert!(!archived_handoff.exists());
    assert_eq!(
        invoke(&f, B, &["entry", "show", "handoff-1"], 1)["code"],
        "entry_not_found"
    );
    let group = show_group(&f, B, "g-1");
    let seat = &group["seats"][0];
    assert_eq!(seat["transcripts"], json!([B]), "{group}");
    assert_eq!(seat["role"], "reviewer");
    assert!(seat["handoff"].is_null(), "{group}");
    assert!(
        store
            .control()
            .unwrap()
            .get(&successor)
            .unwrap()
            .pending_links
            .is_empty()
    );
    let brief = invoke(&f, B, &["brief", "--project", "scribe"], 0);
    assert!(!brief.to_string().contains("Carry on."), "{brief}");
}
