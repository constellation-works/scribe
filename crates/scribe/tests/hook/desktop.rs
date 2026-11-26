#[test]
fn claude_desktop_origin_shapes_match_cli_verdicts_in_hooks() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);

    // Human desktop payload on `user-prompt-submit` echoes `#scribe` markers.
    let mut human = claude.payload("user-prompt-submit.json");
    human["prompt"] = json!("always prefer explicit dates #scribe preferences");
    human["origin"] = json!({"kind": "human"});
    human["promptSource"] = json!("sdk");
    human["turnOrigin"] = json!("human");
    let out = hook_ok(&fx, "user-prompt-submit", &human);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: marked — cataloged after the reply"
    );

    // Peer, task-notification, and host-synthetic payloads are skipped silently.
    for (name, extra) in [
        (
            "peer",
            json!({
                "origin": {"kind": "peer", "from": "local_1", "name": "peer", "body": "#scribe preferences: no", "hostInjected": true},
                "promptSource": "system",
                "turnOrigin": "peer",
                "isMeta": true
            }),
        ),
        (
            "task-notification",
            json!({
                "origin": {"kind": "task-notification", "producer": "session-task", "runId": "r-1"},
                "promptSource": "system",
                "turnOrigin": "task_notification"
            }),
        ),
        (
            "host-synthetic",
            json!({
                "promptSource": "sdk",
                "turnOrigin": "host_synthetic"
            }),
        ),
    ] {
        let mut payload = claude.payload("user-prompt-submit.json");
        payload["prompt"] = json!("#scribe preferences: agent wrote this");
        if let (Some(obj), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        let out = hook_ok(&fx, "user-prompt-submit", &payload);
        assert!(
            out.stdout.is_empty(),
            "{name} should be silent: {}",
            text(&out.stdout)
        );
    }

    // `hook stop` marker handover accepts the human desktop transcript line
    // and skips the three non-human desktop transcript lines.
    let typed = "always prefer explicit dates #scribe preferences";
    let at = now_rfc3339();
    let make_line = |uuid: &str, text: &str, extra: Value| {
        let mut line = json!({
            "type": "user",
            "uuid": uuid,
            "sessionId": SESSION,
            "timestamp": at,
            "cwd": claude.cwd,
            "entrypoint": "claude-desktop",
            "message": {"role": "user", "content": text}
        });
        if let (Some(obj), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        format!("{line}\n")
    };
    let progress = format!(
        "{}\n",
        json!({
            "type": "progress",
            "uuid": "u-human",
            "origin": {"kind": "peer"},
            "promptSource": "system",
            "turnOrigin": "peer"
        })
    );
    let transcript = [
        progress,
        make_line(
            "u-human",
            typed,
            json!({"origin": {"kind": "human"}, "promptSource": "sdk", "turnOrigin": "human"}),
        ),
        make_line(
            "u-peer",
            "#scribe preferences: peer",
            json!({"origin": {"kind": "peer"}, "promptSource": "system", "turnOrigin": "peer", "isMeta": true}),
        ),
        make_line(
            "u-task",
            "#scribe preferences: task",
            json!({"origin": {"kind": "task-notification"}, "promptSource": "system", "turnOrigin": "task_notification"}),
        ),
        make_line(
            "u-synth",
            "#scribe preferences: synth",
            json!({"promptSource": "sdk", "turnOrigin": "host_synthetic"}),
        ),
    ]
    .concat();
    std::fs::write(&claude.transcript, transcript).unwrap();
    make_private(&claude.transcript);
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let msg = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert_eq!(
        msg,
        format!("scribe: #scribe in your message is the agent's to record — \"{typed}\""),
        "only u-human is handed over"
    );
    let marks = marks_json(&fx);
    let settled: Vec<&Value> = marks.iter().map(|m| &m["message"]).collect();
    assert_eq!(settled, vec![&json!("u-human")], "{marks:?}");
    assert!(entries(&fx).is_empty());
}
#[test]
fn stop_uses_the_retained_user_row_instead_of_same_uuid_progress_origin() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let typed = "always prefer explicit dates #scribe preferences";
    let at = now_rfc3339();
    let progress = json!({
        "type": "progress",
        "uuid": "u-synth",
        "origin": {"kind": "human"},
        "promptSource": "sdk",
        "turnOrigin": "human"
    });
    let synthetic = json!({
        "type": "user",
        "uuid": "u-synth",
        "sessionId": SESSION,
        "timestamp": at,
        "cwd": claude.cwd,
        "entrypoint": "claude-desktop",
        "message": {"role": "user", "content": typed},
        "promptSource": "sdk",
        "turnOrigin": "host_synthetic"
    });
    write_lines(&claude, &[progress, synthetic]);
    let path = fake_claude(&fx, "exit 1");

    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);

    assert!(
        claude_calls(&fx).is_empty(),
        "synthetic turn is not sent to the model"
    );
    assert!(entries(&fx).is_empty(), "synthetic turn creates no entry");
    assert!(
        marks_json(&fx).is_empty(),
        "synthetic turn writes no mark outcome"
    );
}

#[test]
fn markers_in_a_slash_command_turn_and_a_queued_line_of_an_off_session_are_handed_over_at_stop() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let slash = "draft the plan; #scribe preference: dates are always absolute";
    let queued = "#scribe preference: keep status updates under five lines";
    typed_forms_transcript(&claude, slash, queued);
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let context = reply(&out)["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(context.lines().count(), 2, "{context}");
    let marks = marks_json(&fx);
    let settled: Vec<&Value> = marks.iter().map(|m| &m["message"]).collect();
    assert_eq!(
        settled,
        vec![&json!("u-slash"), &json!("u-queued")],
        "{marks:?}"
    );
    assert!(
        marks.iter().all(|m| m["code"] == "agent_handoff"),
        "{marks:?}"
    );
}

#[test]
fn a_handed_over_marker_stays_listed() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    // As Claude Code writes them: each line carries the session's cwd.
    write_lines(
        &claude,
        &[
            json!({"type": "session-owner", "sessionId": SESSION}),
            json!({"type": "user", "uuid": "u-1", "sessionId": SESSION, "cwd": claude.cwd,
                   "timestamp": now_rfc3339(),
                   "message": {"role": "user", "content": MARKED}}),
            json!({"type": "assistant", "uuid": "a-u-1", "sessionId": SESSION, "cwd": claude.cwd,
                   "timestamp": now_rfc3339(),
                   "message": {"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}}),
        ],
    );
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert!(!out.stdout.is_empty());
    assert_eq!(marks_json(&fx)[0]["code"], "agent_handoff");

    let listed = fx.check(Cli(&["mark", "list", "--json"]), Success);
    let doc: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(doc["total"], 1, "{doc}");
    assert_eq!(doc["marks"][0]["message"], "u-1");
    assert_eq!(doc["marks"][0]["outcome"], "refused");
    assert_eq!(doc["marks"][0]["code"], "agent_handoff");
}

#[cfg(unix)]
#[test]
fn an_error_after_marking_rolls_back_the_marker_and_audits_the_failure() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_switch_on(&fx, SESSION);
    marked_turns(&claude, &[("u-1", MARKED)]);
    // Fail when mark_due persists `sw_due = 1` after annotation::run writes `marks`.
    support::sql(
        &fx.root(),
        "CREATE TRIGGER fail_mark_due BEFORE UPDATE OF sw_due ON sessions \
         WHEN NEW.sw_due = 1 BEGIN SELECT RAISE(ABORT, 'injected mark_due failure'); END;",
    );

    let failed = hook(&fx, "stop", &claude.payload("stop.json"));
    assert_eq!(failed.status.code(), Some(1), "{}", text(&failed.stderr));
    assert!(failed.stdout.is_empty(), "{}", text(&failed.stdout));
    support::sql(&fx.root(), "DROP TRIGGER fail_mark_due;");

    assert!(
        marks_json(&fx).is_empty(),
        "marking rolled back with the failed Stop: {:?}",
        marks_json(&fx)
    );
    assert_eq!(
        support::control(&fx.root())["sessions"][SESSION]["due"],
        false
    );
    let failed_audit = audit(&fx);
    assert_eq!(failed_audit.len(), 1, "{failed_audit:?}");
    assert_eq!(failed_audit[0]["command"], "hook stop");
    assert_eq!(failed_audit[0]["status"], "failure");
    assert_eq!(failed_audit[0]["error_code"], "store_database");

    // Once the fault clears, the next Stop still hands the marker over.
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let reply_json = reply(&out);
    assert!(
        reply_json["systemMessage"]
            .as_str()
            .unwrap()
            .contains(MARKED),
        "{reply_json}"
    );
    assert!(
        reply_json["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("scribe entry add --tag preferences"),
        "{reply_json}"
    );
    let marks = marks_json(&fx);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0]["code"], "agent_handoff");
    assert_eq!(
        support::control(&fx.root())["sessions"][SESSION]["due"],
        true
    );
    let all_audit = audit(&fx);
    assert_eq!(all_audit.len(), 3, "{all_audit:?}");
    assert_eq!(all_audit[1]["command"], "hook stop");
    assert_eq!(all_audit[1]["status"], "failure");
    assert_eq!(all_audit[1]["error_code"], "agent_handoff");
    assert_eq!(all_audit[1]["target"], format!("session:{SESSION}"));
    assert_eq!(all_audit[2]["command"], "hook stop");
    assert_eq!(all_audit[2]["status"], "success");
    assert_eq!(all_audit[2]["target"], format!("session:{SESSION}"));
    assert!(all_audit[2]["error_code"].is_null());
}
