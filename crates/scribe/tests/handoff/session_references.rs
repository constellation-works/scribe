#[test]
fn session_pass_yields_no_memory_or_proposed_claim_from_handoff_text() {
    let fx = Fixture::new();
    // Configure an assistant-reading catcher too so we prove an assistant turn echoing a handoff
    // cannot be admitted as a candidate either.
    fx.config(
        r#"
[tags.notes.capture]
from = "both"
route = "propose"
triggers = ["always", "from now on"]
criteria = "Any standing rule mentioned by user or assistant."
not = "One-off remarks."
"#,
    );

    let tr = Transcript::new(&fx, ON_SESSION);
    let request = "Please write a handoff summarizing where we stopped.";
    let handoff_body = valid_handoff(
        "Always keep snapshots private; from now on we migrated to the new pipeline on 2026-11-22.",
        "From now on run the full suite before every release.",
    );
    let echoed_reply = format!("Recorded handoff:\n\n{handoff_body}");
    tr.write(&[
        ("u-1", "user", request),
        ("a-1", "assistant", &echoed_reply),
    ]);

    // Write the handoff entry on the session.
    let rel = format!("session:{ON_SESSION}");
    fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            request,
            &handoff_body,
            "--json",
        ],
        Success,
    );

    // Switch the session on and run a pass whose fake model tries to return both a candidate
    // claim and a memory item quoting the handoff's text.
    tr.prompt(&fx, "/scribe on");
    let claim_quote =
        "Always keep snapshots private; from now on we migrated to the new pipeline on 2026-11-22.";
    let pass_answer = json!({
        "title": "Pipeline work",
        "summary": "Summarized pipeline state.",
        "goals": "Finish migration.",
        "open": null,
        "new": [{
            "starts_at": "u-1",
            "heading": "Handoff turn",
            "text": "User asked for a handoff.",
            "topics": [],
            "related": []
        }],
        "next": ["Run tests."],
        "topics": ["pipeline"],
        "related": [],
        "terms": [],
        "candidates": [{
            "quote": claim_quote,
            "message": "a-1",
            "tag": "notes",
            "subject": "snapshots",
            "context": "Always keep snapshots private. Mentioned in the handoff."
        }],
        "memory": [{
            "kind": "now",
            "now_kind": "plan",
            "title": "Pipeline migration",
            "text": "Migrated to the new pipeline.",
            "quote": claim_quote,
            "message": "a-1",
            "date": "2026-11-22"
        }]
    });
    let mut prompt = Value::Null;
    let pass = support::pass::run(
        |step, input| support::pass::cli_step(&fx, &[], step, input),
        json!({"sessions": [ON_SESSION]}),
        |request, _| {
            prompt = request.clone();
            Ok(pass_answer.clone())
        },
    )
    .unwrap();
    assert_eq!(pass["passes"][0]["result"], "written");

    // Zero proposed claims and zero memory items were created from the handoff text.
    let proposals = json_of(&fx.check(Cli(&["proposal", "list", "--json"]), Success));
    assert!(
        proposals["proposals"].as_array().unwrap().is_empty(),
        "pass must open no claim from handoff text: {proposals}"
    );
    let memory = json_of(&fx.check(Cli(&["memory", "show", "--json"]), Success));
    assert!(
        memory["now"].as_array().unwrap().is_empty()
            && memory["milestones"].as_array().unwrap().is_empty(),
        "pass must write no memory item from handoff text: {memory}"
    );

    // The handoff entry body was not fed as a preferences/notes exclusion either.
    let prompt = prompt.to_string();
    assert!(
        !prompt.contains("handoff-1"),
        "pass prompt should not include stored handoff entries:\n{prompt}"
    );
}

#[test]
fn marker_capture_cannot_create_or_attribute_a_handoff_to_the_user() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let unhinted = "Please save a session summary before we stop #scribe";
    let hinted = "#scribe handoff: capture where we paused in this branch";
    let folded = "#scribe add this summary to handoff";
    let pref = "Always keep status updates concise #scribe preferences";
    tr.write(&[
        ("u-1", "user", unhinted),
        ("u-2", "user", hinted),
        ("u-3", "user", folded),
        ("u-4", "user", pref),
    ]);

    // One Stop hands every marker to the agent and writes no entry.
    let stop_payload = json!({
        "session_id": OFF_SESSION,
        "transcript_path": tr.path,
        "cwd": tr.cwd,
        "hook_event_name": "Stop"
    });
    let out = fx.check(
        Cli(&["hook", "stop"]),
        Success.stdin(stop_payload.to_string().as_bytes()),
    );
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let lines: Vec<&str> = context.lines().collect();
    assert_eq!(lines.len(), 4, "{context}");
    // The hinted handoff line is the agent's to write as its own handoff:
    // no `--spoken`, which would record the user's statement.
    assert!(lines[1].contains("--tag handoff "), "{context}");
    assert!(!lines[1].contains("--spoken"), "{context}");

    // Zero entries were written: nothing is attributed to the user.
    let listed = json_of(&fx.check(&["list", "--json"], Success));
    let entries = listed["entries"].as_array().unwrap();
    assert!(entries.is_empty(), "{entries:?}");

    // Every marker was recorded as handed to the agent in `marks.json`.
    let marks_doc: Value = serde_json::from_str(
        &support::exported(&fx.root(), "marks.json").expect("marks.json written"),
    )
    .unwrap();
    let marks = marks_doc["marks"].as_array().unwrap();
    assert_eq!(marks.len(), 4, "{marks:?}");
    for uuid in ["u-1", "u-2", "u-3", "u-4"] {
        let m = marks.iter().find(|m| m["message"] == uuid).unwrap();
        assert_eq!(m["outcome"], "refused", "{m}");
        assert_eq!(m["code"], "agent_handoff", "{m}");
    }

    // Agent handoff path still works normally on the same session.
    let rel = format!("session:{OFF_SESSION}");
    let added = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            unhinted,
            &valid_handoff("State recorded.", "Continue next."),
            "--json",
        ],
        Success,
    ));
    assert_eq!(added["id"], "handoff-1");
    assert_eq!(added["author"]["kind"], "agent");
}

#[test]
fn handoff_add_refuses_multiple_or_invalid_session_refs_so_unverified_sessions_never_display_it() {
    let fx = Fixture::new();
    let tr_a = Transcript::new(&fx, OFF_SESSION);
    let tr_b = Transcript::new(&fx, ON_SESSION);
    let third = "8a4f3e01-5c7d-4f62-9b94-3d2e1f0a9b82";
    let tr_c = Transcript::new(&fx, third);

    let request = "Please write a handoff for this session before stopping.";
    tr_a.write(&[("u-1", "user", request)]);
    tr_b.write(&[("u-1", "user", "Unrelated discussion in another session.")]);
    tr_c.write(&[("u-1", "user", request)]);

    let body = valid_handoff(
        "Work paused cleanly.",
        "Resume from the next checklist item.",
    );
    let rel_a = format!("session:{OFF_SESSION}");
    let rel_b = format!("session:{ON_SESSION}");
    let rel_c = format!("session:{third}");

    // Refused: verified session A paired with unverified session B.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel_a,
            "--related",
            &rel_b,
            "--quote",
            request,
            &body,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Refused: even when both transcripts contain the same generic phrase, a handoff belongs to one session.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel_a,
            "--related",
            &rel_c,
            "--quote",
            request,
            &body,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Refused: invalid session id syntax alongside a valid session (or alone).
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel_a,
            "--related",
            "session:../bad",
            "--quote",
            request,
            &body,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Neither unverified session displays a handoff, and no entry was written.
    let (code, _) = refused(&fx, &["session", "show", ON_SESSION]);
    assert_eq!(code, "session_not_found");
    let listed = json_of(&fx.check(&["list", "--json"], Success));
    assert!(listed["entries"].as_array().unwrap().is_empty());

    // Accepted: single verified session plus non-session related refs (and duplicate identical session ref).
    let added = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel_a,
            "--related",
            &rel_a,
            "--related",
            "task:ORB-1",
            "--related",
            "repo:scribe",
            "--quote",
            request,
            &body,
            "--json",
        ],
        Success,
    ));
    assert_eq!(added["id"], "handoff-1");

    // Only OFF_SESSION displays the handoff in session show and brief.
    let shown_a = json_of(&fx.check(Cli(&["session", "show", OFF_SESSION, "--json"]), Success));
    assert_eq!(shown_a["handoffs"].as_array().unwrap().len(), 1);
    assert_eq!(shown_a["handoffs"][0]["id"], "handoff-1");
    let (code, _) = refused(&fx, &["session", "show", ON_SESSION]);
    assert_eq!(code, "session_not_found");
    let brief = json_of(&fx.check(Cli(&["brief", "--project", "scribe", "--json"]), Success));
    let sessions = brief["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], OFF_SESSION);
    let plain = fx.check(Cli(&["brief", "--project", "scribe"]), Text);
    let prefix = &OFF_SESSION[..8];
    assert!(plain.contains(&format!("- {prefix} ·")), "{plain}");
    assert_eq!(
        json_of(&fx.check(Cli(&["session", "show", prefix, "--json"]), Success))["id"],
        OFF_SESSION
    );
}

#[test]
fn handoff_update_cannot_move_attach_or_orphan_session_reference_but_allows_other_metadata() {
    let fx = Fixture::new();
    let tr_a = Transcript::new(&fx, OFF_SESSION);
    let tr_b = Transcript::new(&fx, ON_SESSION);
    let request = "Please save a handoff before ending this session.";
    tr_a.write(&[("u-1", "user", request)]);
    tr_b.write(&[("u-1", "user", "Different session without that request.")]);

    let rel_a = format!("session:{OFF_SESSION}");
    let rel_b = format!("session:{ON_SESSION}");
    let added = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel_a,
            "--related",
            "task:ORB-1",
            "--quote",
            request,
            &valid_handoff("Initial state.", "Initial next."),
            "--json",
        ],
        Success,
    ));
    assert_eq!(added["id"], "handoff-1");

    // Refused: removing the verified session would orphan the handoff.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--remove-related",
            &rel_a,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Refused: adding another session would display the handoff on an unverified session.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--add-related",
            &rel_b,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Refused: replacing the verified session with another session.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--remove-related",
            &rel_a,
            "--add-related",
            &rel_b,
        ],
    );
    assert_eq!(code, "handoff_session");

    // Allowed: editing content, topics, and non-session related references preserves the session ref.
    let updated_body = valid_handoff("Updated state.", "Updated next.");
    let updated = json_of(&fx.check(
        &[
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--content",
            &updated_body,
            "--add-topic",
            "release",
            "--remove-related",
            "task:ORB-1",
            "--add-related",
            "pr:constellation/scribe#42",
            "--json",
        ],
        Success,
    ));
    assert_eq!(
        updated["related"],
        json!([rel_a, "pr:constellation/scribe#42"])
    );
    assert_eq!(updated["topics"], json!(["release"]));

    // Allowed no-ops: re-adding the existing session or removing an absent session changes nothing.
    let noop_add = fx.check(
        &[
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--add-related",
            &rel_a,
            "--json",
        ],
        Success,
    );
    assert_eq!(text(&noop_add.stderr).trim(), "nothing changed");
    let noop_remove = fx.check(
        &[
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--remove-related",
            &rel_b,
            "--json",
        ],
        Success,
    );
    assert_eq!(text(&noop_remove.stderr).trim(), "nothing changed");

    // Only OFF_SESSION displays handoff-1; ON_SESSION still does not.
    let shown_a = json_of(&fx.check(Cli(&["session", "show", OFF_SESSION, "--json"]), Success));
    assert_eq!(shown_a["handoffs"].as_array().unwrap().len(), 1);
    assert_eq!(shown_a["handoffs"][0]["id"], "handoff-1");
    let (code, _) = refused(&fx, &["session", "show", ON_SESSION]);
    assert_eq!(code, "session_not_found");
    let brief = json_of(&fx.check(Cli(&["brief", "--project", "scribe", "--json"]), Success));
    let sessions = brief["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], OFF_SESSION);
}

#[test]
fn session_show_reports_actual_switch_state_for_switched_on_handoff_only_session_before_first_pass()
{
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, ON_SESSION);
    let request = "Please save a handoff before our first pass runs.";
    tr.write(&[("u-1", "user", request)]);

    // Switch the session on, without running a session pass (no catalog record yet).
    tr.prompt(&fx, "/scribe on");
    assert!(
        support::record_text(&fx.root(), ON_SESSION).is_none(),
        "no catalog record exists before the first pass"
    );

    // Add an active handoff for the switched-on, unrecorded session.
    let rel = format!("session:{ON_SESSION}");
    let added = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            request,
            &valid_handoff("Switched on, no pass yet.", "Run first pass or resume."),
            "--json",
        ],
        Success,
    ));
    assert_eq!(added["id"], "handoff-1");

    // `session show` must report the actual switch state while keeping `has_record: false` and the handoff.
    let shown = json_of(&fx.check(Cli(&["session", "show", ON_SESSION, "--json"]), Success));
    assert_eq!(shown["id"], ON_SESSION);
    assert_eq!(shown["has_record"], false);
    assert_eq!(shown["on"], true);
    assert_eq!(shown["due"], true);
    assert_eq!(shown["handoffs"].as_array().unwrap().len(), 1);
    assert_eq!(shown["handoffs"][0]["id"], "handoff-1");

    // Prefix and short id (`s-1`) resolve the same way.
    let shown_short = json_of(&fx.check(Cli(&["session", "show", "s-1", "--json"]), Success));
    assert_eq!(shown_short, shown);

    // Table and plain rendering reflect `on: yes` / `due: yes` and avoid saying it was not switched on.
    let table = fx.check(
        Cli(&["session", "show", ON_SESSION, "--format", "table"]),
        Text,
    );
    assert!(table.contains("handoff-1"), "{table}");
    assert!(
        !table.contains("not switched on"),
        "switched-on session must not claim it was not switched on:\n{table}"
    );
    assert!(
        table
            .lines()
            .any(|l| l.starts_with("on ") && l.contains("yes")),
        "table must report on=yes:\n{table}"
    );
    assert!(
        table
            .lines()
            .any(|l| l.starts_with("due ") && l.contains("yes")),
        "table must report due=yes:\n{table}"
    );

    let plain = fx.check(Cli(&["session", "show", ON_SESSION]), Text);
    let first_line = plain.lines().next().unwrap();
    assert_eq!(
        first_line.split('\t').nth(2),
        Some("yes"),
        "plain row due column must be yes:\n{plain}"
    );

    // Switching off before any pass still has no record, and now reports `on: false`, `due: false`.
    tr.prompt(&fx, "/scribe off");
    let shown_off = json_of(&fx.check(Cli(&["session", "show", ON_SESSION, "--json"]), Success));
    assert_eq!(shown_off["has_record"], false);
    assert_eq!(shown_off["on"], false);
    assert_eq!(shown_off["due"], false);
    assert_eq!(shown_off["handoffs"][0]["id"], "handoff-1");
}
