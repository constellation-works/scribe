#[test]
fn a_session_links_to_its_parent_and_its_pass_reads_the_parent_as_context() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    assert_eq!(setup.pass()["passes"][0]["result"], "written");
    setup.write(
        CHILD,
        &[line(
            "user",
            "c-1",
            "Side session: fix the flaky test only.",
        )],
    );

    // Refused, each with its code, and nothing switched or linked.
    let reason = setup.scribe_in(CHILD, &format!("/scribe link {ON}"));
    assert!(reason.contains("refused"), "{reason}");
    let reason = setup.scribe_in(CHILD, &format!("/scribe on --from {UNKNOWN}"));
    assert!(reason.contains("refused"), "{reason}");
    let reason = setup.scribe_in(CHILD, &format!("/scribe on --from {CHILD}"));
    assert!(reason.contains("cannot link to itself"), "{reason}");
    assert_eq!(
        setup.switch_of(CHILD),
        Value::Null,
        "a refusal switches nothing"
    );
    assert_eq!(
        setup.audit_codes()[2..],
        [
            json!("link_session_off"),
            json!("link_unknown_session"),
            json!("link_self")
        ]
    );

    // Switched on and linked in one command; the link waits on the switch
    // until the first pass writes the record.
    let reason = setup.scribe_in(CHILD, &format!("/scribe on --from {ON}"));
    assert!(
        reason.starts_with(&format!(
            "scribe: on — this session is cataloged from now on and linked: spawned_from {ON}"
        )),
        "{reason}"
    );
    assert_eq!(setup.switch_of(CHILD)["on"], true);
    assert_eq!(
        setup.switch_of(CHILD)["pending_links"],
        json!([{"type": "spawned_from", "session": ON}])
    );
    let reason = setup.scribe_in(CHILD, &format!("/scribe link {ON}"));
    assert!(reason.ends_with("nothing changed"), "{reason}");

    // The parent links back to its child: a cycle, refused.
    let audit = setup.audit_lines();
    let reason = setup.scribe_in(ON, &format!("/scribe link {CHILD} --type continues"));
    assert!(reason.contains("cycle"), "{reason}");
    assert_eq!(setup.audit_codes()[audit..], [json!("link_cycle")]);

    // The child's pass carries the parent's latest record as context.
    let before = setup.prompts().len();
    let report = setup.pass();
    assert_eq!(report["passes"][0]["session"], CHILD, "{report}");
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), before + 1);
    let prompt = &prompts[before];
    for part in [
        format!("--- Linked session (data): this session was spawned from session {ON} ---"),
        String::from("title: Defining crew levels"),
        String::from("Daniel defined what level 4 means for crews."),
        String::from("do not retell the linked session"),
    ] {
        assert!(prompt.contains(&part), "{part:?} missing from:\n{prompt}");
    }

    // The link moved onto the child's record; the parent shows the child.
    assert_eq!(setup.switch_of(CHILD)["pending_links"], json!([]));
    let child = setup.record(CHILD).unwrap();
    assert_eq!(
        child["links"],
        json!([{"type": "spawned_from", "session": ON, "title": "Defining crew levels"}])
    );
    assert_eq!(child["related"], json!([]), "related is unchanged");
    let parent = setup.record(ON).unwrap();
    assert_eq!(parent["links"], json!([]));
    assert_eq!(
        parent["children"],
        json!([{"type": "spawned_from", "session": CHILD, "title": "Defining crew levels"}])
    );
    let tree: Value = serde_json::from_slice(
        &setup
            .run(&["session", "list", "--tree", "--json"], b"")
            .stdout,
    )
    .unwrap();
    let order: Vec<&str> = tree["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(order, [ON, CHILD]);
    let plain = setup.run(&["session", "list", "--tree"], b"");
    let rows: Vec<Vec<String>> = text(&plain.stdout)
        .lines()
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect();
    assert_eq!(
        rows[1][..2],
        ["s-2".to_owned(), "s-1".to_owned()],
        "PARENT column"
    );
}

#[test]
fn s9_answers_write_bounded_segments_instead_of_one_outcome() {
    let setup = Setup::new();
    setup.append(
        ON,
        &[
            line("user", "u-2", "A second thread."),
            line("assistant", "a-2", "A third thread."),
        ],
    );
    setup.scribe("/scribe on");
    let mut body = answer();
    body.as_object_mut().unwrap().remove("outcome");
    body["open"] = Value::Null;
    body["new"] = json!(["u-1", "u-2", "a-2"].map(|id| json!({
        "starts_at": id, "heading": format!("Thread {id}"), "text": "é".repeat(900),
        "topics": [], "related": []
    })));
    setup.answer(&body);
    let out = setup.run_pass(&["session", "pass", "--json"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let record = setup.record(ON).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 3);
    assert_eq!(record["log"][0]["state"], "sealed");
    assert_eq!(record["log"][0]["last_message"], "a-1");
    assert_eq!(record["log"][1]["last_message"], "u-2");
    assert_eq!(record["log"][2]["state"], "open");
}

#[test]
fn a_pass_refuses_when_the_record_changed_since_it_was_loaded() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.append(
        ON,
        &[
            line("user", "u-2", "More work."),
            line("assistant", "a-2", "Done."),
        ],
    );
    let root = setup.fx.root();
    setup.before_answer.replace(Some(Box::new(move || {
        support::sql(
            &root,
            &format!(
                "UPDATE sessions SET updated = '2030-01-01T00:00:00Z',
                      summary = 'Rebuilt summary.' WHERE provider_session = '{ON}'"
            ),
        );
    })));
    let out = setup.run_pass(&["session", "pass", "--session", ON, "--json"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "failed");
    assert_eq!(report["passes"][0]["error_code"], "record_changed");
    assert!(
        report["passes"][0]["error"]
            .as_str()
            .unwrap()
            .contains("changed since it was loaded"),
        "{}",
        report["passes"][0]["error"]
    );
    let record = setup.record(ON).unwrap();
    assert_eq!(
        record["updated"], "2030-01-01T00:00:00Z",
        "the rebuilt record is kept"
    );
    assert_eq!(record["summary"], "Rebuilt summary.");
    assert_eq!(record["scribed_through"], "a-1", "{record}");
    assert_eq!(setup.control()["due"], true, "the session stays due");
}

#[test]
fn rebuild_refusals_are_audited_and_leave_the_record_unchanged() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    let before = setup.record_text().into_bytes();
    let out = setup.run_any(&rebuild_args(false), b"");
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "confirm_required");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("1 segment(s) and 1 pass(es)")
    );
    setup.scribe("/scribe off");
    setup.scribe("/scribe on");
    let out = setup.run_any(&rebuild_args(true), b"");
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stderr).unwrap()["code"],
        "session_busy"
    );
    setup.scribe("/scribe off");
    std::fs::remove_file(setup.transcript(ON)).unwrap();
    let out = setup.run_any(&rebuild_args(true), b"");
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stderr).unwrap()["code"],
        "transcript_missing"
    );
    assert_eq!(setup.record_text().into_bytes(), before);
    assert_eq!(setup.prompts().len(), 1, "no model calls for refusals");
    let rebuilds: Vec<Value> = support::audit_lines(&setup.fx.root())
        .into_iter()
        .filter(|l| l["command"] == "session rebuild")
        .collect();
    assert_eq!(rebuilds.len(), 3);
    for line in rebuilds {
        assert_eq!(line["target"], format!("session:{ON}"));
        assert_eq!(line["status"], "failure");
    }
}

#[test]
fn a_rebuild_of_a_forgotten_session_is_refused_before_any_model_call_or_checkpoint() {
    let s = Setup::new();
    s.scribe("/scribe on");
    s.pass();
    let forgot = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(forgot.contains("scribe: forgot s-1"), "{forgot}");
    // The transcript is still on disk, so only the refusal stands between
    // the rebuild and the model.
    assert!(s.transcript(ON).exists());
    let calls = s.prompts().len();
    let out = s.run_any(&rebuild_args(true), b"");
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "session_forgotten", "{error}");
    assert_eq!(
        s.prompts().len(),
        calls,
        "a forgotten session takes no model call"
    );
    assert_eq!(s.checkpoint_count(), 0, "and keeps no rebuild checkpoint");
}

// The rebuild lock is gone (2_design.md §19.6): a session forgotten after
// prepare admitted the rebuild is refused when apply admits it again, and no
// checkpoint is written.
#[test]
fn rebuild_apply_refuses_a_session_forgotten_after_prepare() {
    let s = Setup::new();
    s.scribe("/scribe on");
    s.pass();
    let config = pass::claude_config(&s.fx);
    let prepared = pass::plugin_tool(
        &s.fx,
        &[],
        "session-rebuild-prepare",
        json!({"session": ON, "author": "test-agent", "confirm": true}),
        &config,
    )
    .unwrap();
    assert_eq!(prepared["needs_model"], true, "{prepared}");
    let answer = s
        .fixed_response(&prepared["window_request"], false)
        .unwrap();
    let forgot = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(forgot.contains("scribe: forgot s-1"), "{forgot}");
    let reply = pass::plugin_tool(
        &s.fx,
        &[],
        "session-rebuild-apply",
        json!({"basis": prepared["basis"], "cursor": prepared["cursor"], "answer": [answer, null]}),
        &config,
    )
    .unwrap_err();
    let reply: Value = serde_json::from_slice(&reply.stdout).unwrap();
    assert_eq!(reply["error"]["code"], "session_forgotten", "{reply}");
    assert_eq!(s.checkpoint_count(), 0);
}

#[test]
fn a_catalog_of_a_forgotten_session_with_a_record_is_refused_before_any_model_call() {
    let s = Setup::new();
    s.scribe("/scribe on");
    s.pass();
    let forgot = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(forgot.contains("scribe: forgot s-1"), "{forgot}");
    let calls = s.prompts().len();
    let out = s.run_any(
        &["session", "catalog", ON, "--author", "opus", "--json"],
        b"",
    );
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "session_forgotten", "{error}");
    assert_eq!(
        s.prompts().len(),
        calls,
        "a forgotten session takes no model call"
    );
    assert_eq!(s.checkpoint_count(), 0);
}

#[test]
fn a_catalog_of_a_forgotten_session_never_passed_is_refused_before_any_model_call() {
    let s = Setup::new();
    // Switched on, never passed: a switch row with no record.
    let mut payload = s.payload("UserPromptSubmit");
    payload["session_id"] = json!(NEVER);
    payload["transcript_path"] = json!(s.transcript(NEVER));
    payload["prompt"] = json!("/scribe on");
    s.hook("user-prompt-submit", &payload);
    let forgot = text(&type_settlement(&s, &format!("#scribe forget {NEVER}")).stdout);
    assert!(forgot.contains("scribe: forgot s-"), "{forgot}");
    let calls = s.prompts().len();
    let out = s.run_any(
        &["session", "catalog", NEVER, "--author", "opus", "--json"],
        b"",
    );
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "session_forgotten", "{error}");
    assert_eq!(
        s.prompts().len(),
        calls,
        "a forgotten session takes no model call"
    );
    assert_eq!(s.checkpoint_count(), 0);
}

#[test]
fn a_pass_window_that_fails_rolls_its_closures_back_and_audits_none() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    let proposal = json!({
        "id": "p-1",
        "kind": "candidate",
        "older": "decisions-1",
        "newer": null,
        "candidate": {
            "tag": "decisions",
            "role": "user",
            "quote": "I always keep decisions item 1 documented.",
            "evidence": null,
            "cite": {
                "session": ON,
                "message": "u-1",
                "at": "2026-11-03T06:00:00Z",
                "prompt_sha256": "0".repeat(64),
                "line": 1,
                "cwd": "/nonexistent/work/scribe"
            },
            "project": "scribe",
            "provider": "claude-code",
            "host": "host",
            "admitted": null
        },
        "reason": "Unmarked decisions statement.",
        "quotes": [],
        "state": "open",
        "opened_at": "2026-11-03T06:00:00Z",
        "settled_at": null,
        "settled_by": null,
        "author": {"kind": "scribe", "name": "session-pass"},
        "updated_by": null,
        "settled_author": null,
        "closed_reason": null
    });
    seed_claims(&setup.fx.root(), &[proposal]);

    // A trigger refuses the record's row, so the window's transaction fails
    // after it closed the claim (phase 7: the closure and the record commit
    // together, so nothing persisted is left to audit).
    support::sql(
        &setup.fx.root(),
        "CREATE TRIGGER staged_failure BEFORE UPDATE ON sessions WHEN NEW.has_record = 1
         BEGIN SELECT RAISE(ABORT, 'staged failure'); END;",
    );
    let out = setup.run_pass(&["session", "pass", "--session", ON, "--json"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "failed");
    assert_eq!(
        report["passes"][0]["error_code"], "store_database",
        "{report}"
    );
    // The failed pass backs the session off, so the next run moves on.
    assert_eq!(setup.control()["due"], true);
    assert_eq!(setup.control()["consecutive_failures"], 1);
    assert!(setup.control()["retry_after"].is_string());

    let stored = support::proposals_doc(&setup.fx.root());
    assert_eq!(stored["proposals"][0]["state"], "open");
    assert_eq!(stored["proposals"][0]["closed_reason"], Value::Null);

    let pass_lines: Vec<Value> = support::audit_lines(&setup.fx.root())
        .into_iter()
        .filter(|line| line["command"] == "session pass")
        .collect();
    let closures: Vec<&Value> = pass_lines
        .iter()
        .filter(|line| line["status"] == "success" && line["target"] == "p-1")
        .collect();
    assert!(closures.is_empty(), "{pass_lines:?}");
    let failures: Vec<&Value> = pass_lines
        .iter()
        .filter(|line| line["status"] == "failure" && line["error_code"] == "store_database")
        .collect();
    assert_eq!(failures.len(), 1, "{pass_lines:?}");
}

#[test]
fn rebuild_resumes_hidden_checkpoints_and_preserves_identity_links_and_activity() {
    use scribe_core::activity::CatalogedBy;
    use scribe_core::{Author, AuthorKind, LinkType, Redactor, SessionLink, Store};
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    let store = Store::open(setup.fx.root());
    let id = ON.parse().unwrap();
    let mut original = store.session(&id).unwrap().record;
    original.active = false;
    original.keep_active = true;
    original.active_change = Some(scribe_core::activity::ActiveChange {
        at: original.created,
        by: Author {
            kind: AuthorKind::Agent,
            name: "curator".parse().unwrap(),
        },
        route: scribe_core::activity::ActiveRoute::Manual,
        reason: String::from("No longer current"),
        cite: Some(String::from("session:source#1")),
    });
    original.links = vec![SessionLink {
        kind: LinkType::Continues,
        session: NEVER.parse().unwrap(),
    }];
    original.cataloged_by = Some(CatalogedBy {
        at: original.created,
        by: Author {
            kind: AuthorKind::Agent,
            name: "cataloger".parse().unwrap(),
        },
    });
    original.status = scribe_core::SessionStatus::Final;
    original.log[0].state = scribe_core::SegmentState::Sealed;
    original.log[0].origin = scribe_core::SegmentOrigin::Upgrade;
    let expected = original.updated;
    store
        .settle_pass(
            &id,
            Some(original.clone()),
            false,
            Some(expected),
            &Redactor::new([]),
        )
        .unwrap();
    let before = setup.record_text().into_bytes();
    setup.config("[catalog]\nwindow_messages = 2\nsegment_windows = 2\n");
    setup.append(
        ON,
        &[
            line("user", "u-2", "New work."),
            line("assistant", "a-2", "Built it."),
            line("user", "u-3", "Finish."),
        ],
    );
    setup.answers(&[None, Some(answer()), None, Some(answer()), Some(answer())]);
    let out = setup.run_rebuild(&rebuild_args(true));
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 1);
    assert_eq!(report["passes"][0]["error_code"], "model_failed");
    assert!(support::has_checkpoint(&setup.fx.root(), ON));
    assert_eq!(setup.record_text().into_bytes(), before);
    assert_eq!(
        store
            .list_sessions(&scribe_core::SessionFilter {
                include_inactive: true,
                ..Default::default()
            })
            .unwrap()
            .listing
            .items
            .len(),
        1
    );
    let out = setup.run(&rebuild_args(true), b"");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 2);
    assert_eq!(
        report["passes"][0]["author"],
        json!({"kind": "agent", "name": "test-agent"})
    );
    let shown: Value =
        serde_json::from_slice(&setup.run(&["session", "show", ON, "--json"], b"").stdout).unwrap();
    assert_eq!(
        shown["rebuilt_by"],
        json!({"kind": "agent", "name": "test-agent"})
    );
    let rebuild_line = support::audit_lines(&setup.fx.root())
        .into_iter()
        .rfind(|l| l["command"] == "session rebuild")
        .unwrap();
    assert_eq!(rebuild_line["status"], "success");
    assert_eq!(rebuild_line["author"], "test-agent");
    let rebuilt = store.session(&id).unwrap().record;
    assert_eq!(rebuilt.id, original.id);
    assert_eq!(rebuilt.created, original.created);
    assert_eq!(rebuilt.links, original.links);
    assert_eq!(rebuilt.active, original.active);
    assert_eq!(rebuilt.keep_active, original.keep_active);
    assert_eq!(rebuilt.active_change, original.active_change);
    assert_eq!(rebuilt.cataloged_by, original.cataloged_by);
    assert_eq!(rebuilt.scribed_through.as_deref(), Some("u-3"));
    assert_eq!(rebuilt.passes, 3);
    assert_eq!(rebuilt.status, scribe_core::SessionStatus::Final);
    assert!(
        rebuilt
            .log
            .iter()
            .all(|s| s.origin == scribe_core::SegmentOrigin::Rebuild
                && s.state == scribe_core::SegmentState::Sealed)
    );
    assert!(!support::has_checkpoint(&setup.fx.root(), ON));
    let prompts = setup.prompts();
    assert!(
        !prompts[3].contains("--- user · u-1 ---"),
        "resume starts at failed window"
    );
    assert!(prompts[3].contains("--- user · u-2 ---"));
    assert!(
        prompts[1].contains("There is no record yet"),
        "first rebuild window has no previous record"
    );
}

#[test]
fn rebuild_folds_newer_transcript_messages_after_a_visible_revision_changes() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    let extra = line("user", "u-later", "Work arriving during the rebuild.");
    // The competing writer publishes before the first rebuild answer is
    // returned. No timing race, sleeps or concurrent test load is involved.
    let root = setup.fx.root();
    let transcript = setup.transcript(ON);
    setup.before_answer.replace(Some(Box::new(move || {
        support::sql(
            &root,
            &format!(
                "UPDATE sessions SET updated = '2030-01-01T00:00:00Z'
                 WHERE provider_session = '{ON}'"
            ),
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(file, "{extra}").unwrap();
    })));
    let control_before = setup.control_bytes();
    let out = setup.run(&rebuild_args(true), b"");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 2);
    let record = setup.record(ON).unwrap();
    assert_eq!(record["scribed_through"], "u-later");
    assert_eq!(record["log"][0]["origin"], "rebuild");
    assert_eq!(setup.control_bytes(), control_before);
    let prompts = setup.prompts();
    assert!(prompts[2].contains("--- user · u-later ---"));
    assert!(!prompts[2].contains("--- user · u-1 ---"));
}

#[test]
fn rebuild_repairs_a_repeated_third_window_budget_miss_and_reports_its_window() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.config("[catalog]\nwindow_messages = 2\n");
    setup.append(
        ON,
        &[
            line("user", "u-2", "Second window."),
            line("assistant", "a-2", "Continue."),
            line("user", "u-3", "Third window."),
        ],
    );
    let mut over = answer();
    over["open"] = Value::Null;
    over["new"] = json!([]);
    over["next"] = json!(["Keep this bullet", "é".repeat(1000)]);
    setup.answers(&[
        None,
        Some(answer()),
        Some(answer()),
        Some(over.clone()),
        Some(over),
    ]);

    let out = setup.run(&rebuild_args(true), b"");

    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 3);
    assert_eq!(
        report["passes"][0]["warnings"],
        json!(["window 3: repaired next: cut at bullet boundary (budget 1000 characters)"])
    );
    assert!(text(&out.stderr).contains("window 3: repaired next"));
    assert_eq!(setup.prompts().len(), 5);
    let record = setup.record(ON).unwrap();
    assert_eq!(record["next"], "- Keep this bullet");
    assert_eq!(record["scribed_through"], "u-3");
    assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
    assert!(!support::has_checkpoint(&setup.fx.root(), ON));
}

#[test]
fn rebuild_compresses_an_overlong_third_window_header_even_when_open_is_omitted() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.config("[catalog]\nwindow_messages = 2\n");
    setup.append(
        ON,
        &[
            line("user", "u-2", "Second window."),
            line("assistant", "a-2", "Continue."),
            line("user", "u-3", "Third window."),
        ],
    );
    let mut over = answer();
    over.as_object_mut().unwrap().remove("open");
    over["new"] = json!([]);
    over["summary"] = json!("s".repeat(1201));
    let compressed = json!({"title": "Rebuilt session", "summary": "All three windows.",
                            "goals": "Record the session.", "next": []});
    setup.answers(&[
        None,
        Some(answer()),
        Some(answer()),
        Some(over),
        Some(compressed),
    ]);

    let out = setup.run(&rebuild_args(true), b"");

    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 3);
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 5);
    assert!(prompts[3].contains("Third window."));
    assert!(!prompts[4].contains("Third window."), "window not resent");
    assert!(
        !prompts[4].contains("Level 4 is defined."),
        "segment not resent"
    );
    let record = setup.record(ON).unwrap();
    assert_eq!(record["summary"], "All three windows.");
    assert_eq!(record["scribed_through"], "u-3");
    assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
    assert_eq!(record["log"][0]["windows"], 3);
    assert_eq!(record["log"][0]["state"], "sealed");
    assert_eq!(record["log"][0]["origin"], "rebuild");
    assert!(!support::has_checkpoint(&setup.fx.root(), ON));
}
