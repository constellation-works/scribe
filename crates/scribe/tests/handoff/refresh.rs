#[test]
fn self_refresh_records_mode_and_requires_current_interactive_session() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    tr.write(&[("u-1", "user", "Continue working on the widget.")]);
    let body = valid_handoff("Widget ready.", "Continue widget work.");
    let related = format!("session:{OFF_SESSION}");
    let args = [
        "entry",
        "add",
        "--tag",
        "handoff",
        "--author",
        "opus",
        "--related",
        &related,
        "--mode",
        "continue",
        "--json",
        &body,
    ];
    let out = fx.check(
        with_env(&args, "CLAUDE_CODE_SESSION_ID", OFF_SESSION),
        Success,
    );
    assert_eq!(json_of(&out)["handoff_mode"], "continue");
    let shown = json_of(&fx.check(Cli(&["session", "show", OFF_SESSION, "--json"]), Success));
    assert_eq!(shown["handoffs"][0]["handoff_mode"], "continue");
    let brief = json_of(&fx.check(Cli(&["brief", "--json"]), Success));
    assert!(brief.to_string().contains("continue"));
    let search = json_of(&fx.check(Cli(&["search", "widget", "--json"]), Success));
    assert!(search.to_string().contains("handoff mode: continue"));
    let other = fx.check(
        with_env(&args, "CLAUDE_CODE_SESSION_ID", ON_SESSION),
        Code(1),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&other.stderr).unwrap()["code"],
        "quote_required"
    );
    tr.write(&[("a-1", "assistant", "No user turn.")]);
    let empty = fx.check(
        with_env(&args, "CLAUDE_CODE_SESSION_ID", OFF_SESSION),
        Code(1),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&empty.stderr).unwrap()["code"],
        "quote_not_found"
    );
}

#[test]
fn legacy_database_reads_without_migrating_and_upgrades_on_write() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Please write this session handoff.";
    tr.write(&[("u-1", "user", request)]);
    let rel = format!("session:{OFF_SESSION}");
    let body = valid_handoff("Legacy state.", "Next task.");
    fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--related",
            &rel,
            "--quote",
            request,
            &body,
        ],
        Success,
    );
    support::remove_group_capture_schema(&fx.root());
    support::sql(
        &fx.root(),
        "DROP TABLE group_members; DROP TABLE group_seats; DROP TABLE groups;
             ALTER TABLE entries DROP COLUMN handoff_mode; PRAGMA user_version = 6;",
    );
    let shown = json_of(&fx.check(Cli(&["session", "show", OFF_SESSION, "--json"]), Success));
    assert_eq!(shown["handoffs"][0]["handoff_mode"], Value::Null);
    let out = fx.check(
        with_env(
            &[
                "entry",
                "add",
                "--tag",
                "handoff",
                "--author",
                "opus",
                "--related",
                &rel,
                "--mode",
                "new",
                "--json",
                &valid_handoff("New state.", "Next task."),
            ],
            "CLAUDE_CODE_SESSION_ID",
            OFF_SESSION,
        ),
        Success,
    );
    assert_eq!(json_of(&out)["handoff_mode"], "new");
}

#[test]
fn refresh_continue_after_desktop_clear_inherits_switch_links_and_prepends_one_handoff() {
    for on in [true, false] {
        let fx = Fixture::new();
        let parent = Transcript::new(&fx, OFF_SESSION);
        let body = valid_handoff("The chosen handoff.", "Resume the widget.");
        refresh_handoff(&fx, &parent, "continue", &body);
        let neighbor = Transcript::new(&fx, ON_SESSION);
        refresh_handoff(
            &fx,
            &neighbor,
            "continue",
            &valid_handoff("Other captain.", "Stay here."),
        );
        if on {
            parent.prompt(&fx, "/scribe on");
        }
        let child = after_clear(&fx, &[OFF_SESSION]);
        let before = support::audit_lines(&fx.root()).len();
        for _ in 0..2 {
            let reply = child.session_start(&fx);
            let context = reply["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();
            assert!(context.starts_with(body.trim_end()), "{context}");
            assert_eq!(context.matches("## State").count(), 1);
            assert!(!context.contains("Other captain."));
            let status = child.prompt(&fx, "/scribe status")["reason"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(
                status.starts_with(if on { "scribe: on" } else { "scribe: off" }),
                "{status}"
            );
            assert!(
                status.contains(&format!("continues {OFF_SESSION}")),
                "{status}"
            );
        }
        let audit = support::audit_lines(&fx.root());
        assert_eq!(audit.len(), before + 1);
        assert_eq!(audit[before]["command"], "hook session-start");
        if on {
            child.prompt(&fx, "/scribe off");
            child.session_start(&fx);
            let status = child.prompt(&fx, "/scribe status");
            assert!(
                status["reason"]
                    .as_str()
                    .unwrap()
                    .starts_with("scribe: off")
            );
        }
    }
}

#[test]
fn handoffs_in_other_projects_do_not_supersede_each_other() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Please save this handoff.";
    tr.write(&[("u", "user", request)]);
    for (project, state) in [("one", "First state."), ("two", "Second state.")] {
        fx.check(
            &[
                "add",
                "--tag",
                "handoff",
                "--author",
                "opus",
                "--project",
                project,
                "--related",
                &format!("session:{OFF_SESSION}"),
                "--quote",
                request,
                &valid_handoff(state, "Next."),
                "--json",
            ],
            Success,
        );
    }
    assert_eq!(
        json_of(&fx.check(&["show", "handoff-1", "--json"], Success))["status"],
        "active"
    );
}

#[test]
fn continuation_supersedes_only_its_chain() {
    let fx = Fixture::new();
    let parent = Transcript::new(&fx, OFF_SESSION);
    refresh_handoff(
        &fx,
        &parent,
        "continue",
        &valid_handoff("Parent state.", "Next."),
    );
    let neighbor = Transcript::new(&fx, ON_SESSION);
    refresh_handoff(
        &fx,
        &neighbor,
        "continue",
        &valid_handoff("Neighbor state.", "Next."),
    );
    let child = after_clear(&fx, &[OFF_SESSION]);
    child.session_start(&fx);
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let old = json_of(&fx.check(&["show", "handoff-1", "--json"], Success));
    assert_eq!(old["status"], "superseded");
    assert_eq!(old["superseded_by"], "handoff-3");
    let other = json_of(&fx.check(&["show", "handoff-2", "--json"], Success));
    assert_eq!(other["status"], "active");
}

#[test]
fn refresh_new_or_absent_handoff_leaves_the_ordinary_brief_and_switch_alone() {
    let fx = Fixture::new();
    let parent = Transcript::new(&fx, OFF_SESSION);
    let body = valid_handoff("Leave this context behind.", "Start afresh.");
    refresh_handoff(
        &fx,
        &parent,
        "continue",
        &valid_handoff("Original plan.", "Keep working."),
    );
    refresh_handoff(&fx, &parent, "new", &body);
    parent.prompt(&fx, "/scribe on");
    for predecessor in [OFF_SESSION, ON_SESSION] {
        let child = after_clear(&fx, &[predecessor]);
        let before = support::audit_lines(&fx.root());
        let reply = child.session_start(&fx);
        let context = reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            context.contains("[handoff: handoff-2, mode: new]"),
            "{context}"
        );
        assert!(!context.contains("## State"));
        assert!(reply.get("systemMessage").is_none(), "{reply}");
        assert_eq!(support::audit_lines(&fx.root()), before);
        let status = child.prompt(&fx, "/scribe status");
        assert!(
            status["reason"]
                .as_str()
                .unwrap()
                .contains("never switched on")
        );
    }
}

#[test]
fn refresh_ambiguous_or_missing_ancestry_waits_for_a_typed_handoff_id() {
    let fx = Fixture::new();
    let parent = Transcript::new(&fx, OFF_SESSION);
    let body = valid_handoff("Resume exactly here.", "Run the fixture.");
    let id = refresh_handoff(&fx, &parent, "continue", &body);
    let child = after_clear(&fx, &[OFF_SESSION, ON_SESSION]);
    let before = support::audit_lines(&fx.root());
    let ambiguous = child.session_start(&fx);
    let message = ambiguous["systemMessage"].as_str().unwrap();
    assert!(
        message.contains(OFF_SESSION) && message.contains(ON_SESSION),
        "{message}"
    );
    assert!(
        !ambiguous["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("## State")
    );
    child.write(&[]);
    let ordinary = child.session_start(&fx);
    assert!(
        !ordinary["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("## State")
    );
    assert_eq!(support::audit_lines(&fx.root()), before);
    let reply = child.prompt(&fx, &format!("/scribe:scribe continue {id}"));
    assert!(reply.get("decision").is_none(), "{reply}");
    assert_eq!(
        reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .trim_end(),
        body.trim_end()
    );
    let status = child.prompt(&fx, "/scribe status");
    assert!(
        status["reason"]
            .as_str()
            .unwrap()
            .contains(&format!("continues {OFF_SESSION}"))
    );
}

#[test]
fn refresh_requires_interactive_start_and_provenance_and_refuses_a_self_link() {
    let fx = Fixture::new();
    let parent = Transcript::new(&fx, OFF_SESSION);
    let id = refresh_handoff(&fx, &parent, "continue", &valid_handoff("Parent.", "Next."));
    let child = after_clear(&fx, &[OFF_SESSION]);
    let before = support::audit_lines(&fx.root());
    for (source, entrypoint) in [
        ("startup", "sdk-cli"),
        ("resume", "cli"),
        ("compact", "claude-desktop"),
    ] {
        let reply = child.start_from(&fx, source, entrypoint);
        assert!(
            !reply["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap_or_default()
                .contains("## State")
        );
    }
    let mut mismatched = Transcript::new(&fx, ON_SESSION);
    mismatched.path = child.path.clone();
    let reply = mismatched.session_start(&fx);
    assert!(
        !reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("## State")
    );
    assert_eq!(support::audit_lines(&fx.root()), before);
    let refused = parent.prompt(&fx, &format!("/scribe continue {id}"));
    assert_eq!(refused["decision"], "block");
    assert!(refused.get("hookSpecificOutput").is_none());
    assert_eq!(
        support::audit_lines(&fx.root()).last().unwrap()["error_code"],
        "link_self"
    );
    let status = parent.prompt(&fx, "/scribe status");
    assert!(
        status["reason"]
            .as_str()
            .unwrap()
            .contains("never switched on")
    );
}

#[test]
fn peer_restarted_session_can_write_second_refresh_through_clear_ancestry() {
    let fx = Fixture::new();
    let parent = Transcript::new(&fx, OFF_SESSION);
    refresh_handoff(
        &fx,
        &parent,
        "continue",
        &valid_handoff("First.", "Continue."),
    );
    clear_transcript(&fx, CHILD, &[OFF_SESSION]);
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["handoff_mode"], "continue");
    assert_eq!(json_of(&out)["id"], "handoff-2");
    let grandchild = "8f3e2d90-4b6c-4e51-8a83-2c1d0e9f8a72";
    clear_transcript(&fx, grandchild, &[CHILD]);
    let out = try_refresh(&fx, grandchild);
    assert!(out.status.success(), "{}", text(&out.stderr));
}

#[test]
fn refresh_rejects_invalid_clear_ancestry_and_reports_failed_link() {
    let fx = Fixture::new();
    let parent = clear_transcript(&fx, OFF_SESSION, &[]);
    clear_transcript(&fx, CHILD, &[OFF_SESSION]);
    let assert_refused = |id: &str, failed: &str| {
        let out = try_refresh(&fx, id);
        assert_eq!(out.status.code(), Some(1));
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], "quote_not_found");
        assert!(error.to_string().contains(failed), "{error}");
    };
    assert_refused(CHILD, OFF_SESSION);
    clear_transcript(&fx, OFF_SESSION, &[CHILD]);
    assert_refused(CHILD, CHILD);
    clear_transcript(&fx, OFF_SESSION, &[CHILD, ON_SESSION]);
    assert_refused(CHILD, OFF_SESSION);
    // A symlink is never acceptable transcript provenance, even to a typed transcript.
    let typed = Transcript::new(&fx, ON_SESSION);
    typed.write(&[("u", "user", "A typed turn")]);
    std::fs::remove_file(&parent.path).unwrap();
    std::os::unix::fs::symlink(&typed.path, &parent.path).unwrap();
    assert_refused(CHILD, OFF_SESSION);
}

#[test]
fn refresh_clear_ancestry_is_limited_to_sixteen_hops() {
    let fx = Fixture::new();
    let ids: Vec<String> = (0..18)
        .map(|n| format!("00000000-0000-4000-8000-{n:012x}"))
        .collect();
    for n in 0..17 {
        clear_transcript(&fx, &ids[n], &[&ids[n + 1]]);
    }
    Transcript::new(&fx, &ids[17]).write(&[("u", "user", "A typed turn")]);
    let accepted = try_refresh(&fx, &ids[2]);
    assert!(accepted.status.success(), "{}", text(&accepted.stderr));
    let refused = try_refresh(&fx, &ids[0]);
    assert_eq!(refused.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&refused.stderr).unwrap();
    assert_eq!(error["code"], "quote_not_found");
    assert!(error.to_string().contains(&ids[15]));
    assert!(error.to_string().contains("16-transcript"));
}

#[test]
fn refresh_follows_a_launch_edge_from_a_typed_session_to_a_cleared_rank() {
    let fx = Fixture::new();
    Transcript::new(&fx, ON_SESSION).write(&[("u", "user", "A typed turn")]);
    // The seat's root: a rank a peer launched from the typed session, never typed in.
    ancestry_transcript(&fx, OFF_SESSION, &[("launch", ON_SESSION)]);
    // The session a peer restarted after a clear from that root.
    clear_transcript(&fx, CHILD, &[OFF_SESSION]);
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["handoff_mode"], "continue");
}

#[test]
fn refresh_launch_ancestry_refuses_untyped_ambiguous_cyclic_and_unreadable_links() {
    let fx = Fixture::new();
    let assert_refused = |id: &str, stopped_at: &str, reason: &str| {
        let out = try_refresh(&fx, id);
        assert_eq!(out.status.code(), Some(1));
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], "quote_not_found");
        assert!(error.to_string().contains(stopped_at), "{error}");
        assert!(error.to_string().contains(reason), "{error}");
    };
    // A launched root with no typed turn and no predecessor of its own.
    ancestry_transcript(&fx, OFF_SESSION, &[("launch", ON_SESSION)]);
    ancestry_transcript(&fx, ON_SESSION, &[]);
    clear_transcript(&fx, CHILD, &[OFF_SESSION]);
    assert_refused(CHILD, ON_SESSION, "typed user turn");
    // An unreadable first launcher is a dead end; the second can be typed.
    Transcript::new(&fx, ON_SESSION).write(&[("u", "user", "A typed turn")]);
    ancestry_transcript(
        &fx,
        OFF_SESSION,
        &[("launch", UNREADABLE_SESSION), ("launch", ON_SESSION)],
    );
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
    // Metadata order matters: a cycle before a typed launcher is refused,
    // even though the typed session sorts first by id.
    ancestry_transcript(
        &fx,
        OFF_SESSION,
        &[("launch", CHILD), ("launch", ON_SESSION)],
    );
    assert_refused(CHILD, CHILD, "cycle");
    // A launch edge back to a session already on the walk.
    ancestry_transcript(&fx, OFF_SESSION, &[("launch", CHILD)]);
    assert_refused(CHILD, CHILD, "cycle");
    // A launch edge to a session this host cannot read.
    ancestry_transcript(&fx, CHILD, &[("launch", UNREADABLE_SESSION)]);
    assert_refused(CHILD, UNREADABLE_SESSION, "not readable");
}

#[test]
fn refresh_launch_branches_converging_on_a_shared_ancestor_reach_a_later_typed_launcher() {
    let fx = Fixture::new();
    const FIRST: &str = "8a4f3ea1-5c7d-4f62-9b94-3d2e1f0a9b82";
    const SECOND: &str = "9b5a4fb2-6d8e-4073-8ca5-4e3f2a1b0c93";
    const SHARED: &str = "ac6b50c3-7e9f-4184-9db6-5f4a3b2c1da4";
    const TYPED: &str = "bd7c61d4-8fa0-4295-8ec7-6a5b4c3d2eb5";
    // Two untyped launchers lead to the same untyped ancestor, which has no
    // predecessor of its own; a third launcher carries a typed turn.
    ancestry_transcript(&fx, SHARED, &[]);
    ancestry_transcript(&fx, FIRST, &[("launch", SHARED)]);
    ancestry_transcript(&fx, SECOND, &[("launch", SHARED)]);
    Transcript::new(&fx, TYPED).write(&[("u", "user", "A typed turn")]);
    ancestry_transcript(
        &fx,
        CHILD,
        &[("launch", FIRST), ("launch", SECOND), ("launch", TYPED)],
    );
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["handoff_mode"], "continue");
    // Without the typed launcher the walk still ends untyped, not as a cycle.
    ancestry_transcript(&fx, CHILD, &[("launch", FIRST), ("launch", SECOND)]);
    let out = try_refresh(&fx, CHILD);
    assert_eq!(out.status.code(), Some(1));
    let error = text(&out.stderr);
    assert!(error.contains("quote_not_found"), "{error}");
    assert!(!error.contains("cycle"), "{error}");
}

#[test]
fn refresh_follows_a_refreshed_ranks_clear_edge_before_its_launch_edge() {
    let fx = Fixture::new();
    Transcript::new(&fx, ON_SESSION).write(&[("u", "user", "A typed turn")]);
    ancestry_transcript(&fx, OFF_SESSION, &[("launch", ON_SESSION)]);
    // A refreshed rank: its launch edge names a session this host cannot read.
    // The walk takes the clear edge to the seat's root first, so it is not read.
    ancestry_transcript(
        &fx,
        CHILD,
        &[
            ("launch", UNREADABLE_SESSION),
            ("launch", ON_SESSION),
            ("clear", OFF_SESSION),
        ],
    );
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["handoff_mode"], "continue");
}

#[test]
fn refresh_launch_branches_share_the_transcript_budget() {
    let fx = Fixture::new();
    let ids: Vec<String> = (0..16)
        .map(|n| format!("10000000-0000-4000-8000-{n:012x}"))
        .collect();
    for n in 0..14 {
        clear_transcript(&fx, &ids[n], &[&ids[n + 1]]);
    }
    clear_transcript(&fx, &ids[14], &[]);
    Transcript::new(&fx, &ids[15]).write(&[("u", "user", "A typed turn")]);
    ancestry_transcript(&fx, CHILD, &[("launch", &ids[0]), ("launch", &ids[15])]);
    let out = try_refresh(&fx, CHILD);
    assert_eq!(out.status.code(), Some(1));
    let error = text(&out.stderr);
    assert!(error.contains(&ids[14]), "{error}");
    assert!(error.contains("16-transcript"), "{error}");
    // Shortening the first branch leaves room for the typed launcher.
    clear_transcript(&fx, &ids[13], &[]);
    let out = try_refresh(&fx, CHILD);
    assert!(out.status.success(), "{}", text(&out.stderr));
}

#[test]
fn refresh_clear_and_launch_edges_share_the_sixteen_hop_limit() {
    let fx = Fixture::new();
    let ids: Vec<String> = (0..18)
        .map(|n| format!("00000000-0000-4000-8000-{n:012x}"))
        .collect();
    for n in 0..17 {
        let via = if n % 2 == 0 { "launch" } else { "clear" };
        ancestry_transcript(&fx, &ids[n], &[(via, ids[n + 1].as_str())]);
    }
    Transcript::new(&fx, &ids[17]).write(&[("u", "user", "A typed turn")]);
    let accepted = try_refresh(&fx, &ids[2]);
    assert!(accepted.status.success(), "{}", text(&accepted.stderr));
    let refused = try_refresh(&fx, &ids[0]);
    assert_eq!(refused.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&refused.stderr).unwrap();
    assert_eq!(error["code"], "quote_not_found");
    assert!(error.to_string().contains(&ids[15]));
    assert!(error.to_string().contains("16-transcript"));
}
