#[test]
fn a_seeded_switch_is_read_without_writes_and_a_pass_writes_the_retry_state() {
    let setup = Setup::new();
    setup.config("");
    // The retired file-store decoders are gone: the switch is seeded as a
    // current-format control document, which is strict about every field.
    let doc = json!({"format": 31, "codex_cursors": {}, "sessions": {ON: {
        "provider": "claude-code", "on": true, "since": "2026-11-03T05:00:00Z",
        "due": true, "final": false, "consecutive_failures": 0,
        "retry_after": null, "pending_links": [], "cataloged_by": null}}});
    support::seed_records(&setup.fx.root(), &[&doc.to_string()]);
    setup.run(&["session", "list", "--json"], b"");
    let before = setup.control_bytes();
    setup.run(&["session", "list", "--json"], b"");
    assert_eq!(setup.control_bytes(), before, "a read writes nothing");
    setup.pass();
    let current = support::control(&setup.fx.root());
    assert_eq!(current["format"], 31);
    assert_eq!(current["sessions"][ON]["consecutive_failures"], 0);
    assert_eq!(current["sessions"][ON]["retry_after"], Value::Null);
    assert_eq!(current["sessions"][ON]["pending_links"], json!([]));
}

#[test]
fn scribe_on_off_and_status_switch_the_session_and_block_the_prompt() {
    let setup = Setup::new();
    let reason = setup.scribe("/scribe status");
    assert!(reason.contains("never switched on"), "{reason}");
    assert!(!support::has_control(&setup.fx.root()));

    let reason = setup.scribe("/scribe on");
    assert!(reason.starts_with("scribe: on"), "{reason}");
    let switch = setup.control();
    assert_eq!(switch["on"], true);
    assert_eq!(switch["due"], true);
    assert_eq!(setup.audit_lines(), 1);

    // On twice: a no-op that says so.
    let before = setup.control_bytes();
    let reason = setup.scribe("/scribe ON");
    assert!(reason.starts_with("scribe: already on since "), "{reason}");
    assert!(reason.ends_with("nothing changed"), "{reason}");
    assert_eq!(setup.control_bytes(), before);
    assert_eq!(setup.audit_lines(), 1);

    let reason = setup.scribe("/scribe");
    assert!(reason.starts_with("scribe: on since "), "{reason}");
    assert!(reason.contains("a pass is due; no record yet"), "{reason}");

    let reason = setup.scribe("/scribe off");
    assert!(reason.starts_with("scribe: off"), "{reason}");
    assert_eq!(setup.control()["on"], false);
    assert_eq!(setup.control()["due"], false);
    assert_eq!(setup.audit_lines(), 2);

    let reason = setup.scribe("/scribe maybe");
    assert!(reason.contains("unknown command"), "{reason}");
    assert_eq!(setup.control()["on"], false);

    // No pass runs for a session switched off.
    assert_eq!(setup.pass()["passes"], json!([]));
    assert!(setup.prompts().is_empty());
}

#[test]
fn a_failed_session_catalog_writes_one_failure_audit_line() {
    let setup = Setup::new();
    setup.answers(&[None]);
    let out = setup.run_catalog(&["session", "catalog", NEVER, "--author", "opus"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    // The switch was written and stays due; its window fails once.
    assert!(support::has_control(&setup.fx.root()));
    let audit = audit_log(&setup);
    assert_eq!(audit.len(), 2, "{audit:?}");
    assert_eq!(audit[0]["command"], "session catalog");
    assert_eq!(audit[0]["status"], "success", "the switch");
    assert_eq!(audit[1]["command"], "session catalog");
    assert_eq!(audit[1]["status"], "failure");
    assert_eq!(audit[1]["error_code"], "model_failed");
    assert_eq!(audit[1]["target"], format!("session:{NEVER}"));
    assert_eq!(audit[1]["author"], "opus");

    // A catalog that succeeds resumes the switch, then logs its window.
    setup.answer(&answer());
    let out = setup.run_catalog(&["session", "catalog", NEVER, "--author", "opus"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let audit = audit_log(&setup);
    assert_eq!(audit.len(), 3, "a resume wrote no switch: {audit:?}");
    assert_eq!(audit[2]["status"], "success");
}

#[test]
fn scribe_catalog_in_a_typed_prompt_catalogs_a_past_session_as_daniel() {
    let setup = Setup::new();
    // An id that names no transcript is refused, audited by code, and
    // writes no switch.
    let reason = setup.scribe("/scribe catalog 0a1b2c3d-0000-4000-8000-00000000dead");
    assert!(
        reason.starts_with("scribe: /scribe catalog 0a1b2c3d-0000-4000-8000-00000000dead refused"),
        "{reason}"
    );
    assert!(!support::has_control(&setup.fx.root()));

    // Typed by Daniel in another session: the switch is written as his,
    // and no window runs in the hook.
    let reason = setup.scribe(&format!("/scribe catalog {NEVER}"));
    assert!(
        reason.starts_with(&format!("scribe: cataloging {NEVER} as yours")),
        "{reason}"
    );
    let switch = support::control(&setup.fx.root())["sessions"][NEVER].clone();
    assert_eq!(switch["on"], true);
    assert_eq!(switch["due"], true);
    assert_eq!(switch["cataloged_by"]["by"]["kind"], "user");
    assert!(setup.prompts().is_empty());
    assert!(setup.record(NEVER).is_none());
    let audited = setup.audit_lines();

    // Again: nothing changes and nothing is audited.
    let before = setup.control_bytes();
    let reason = setup.scribe(&format!("/scribe catalog {NEVER}"));
    assert!(reason.contains("already being cataloged"), "{reason}");
    assert_eq!(setup.control_bytes(), before);
    assert_eq!(setup.audit_lines(), audited);

    // The next session pass runs its windows as a catalog and keeps
    // Daniel's attribution; the transcript is days old, so it ends final
    // and off.
    let pass = setup.pass();
    assert_eq!(pass["passes"][0]["result"], "written", "{pass}");
    let record = setup.record(NEVER).unwrap();
    assert_eq!(record["status"], "final", "{record}");
    assert_eq!(record["cataloged_by"]["by"]["kind"], "user", "{record}");
    let switch = support::control(&setup.fx.root())["sessions"][NEVER].clone();
    assert_eq!(switch["on"], false);
}

#[test]
fn stop_marks_due_by_turns_and_minutes_and_session_end_marks_final_without_a_model() {
    let setup = Setup::new();
    setup.config("[catalog]\nevery_turns = 2\nevery_minutes = 600\n");
    setup.scribe("/scribe on");
    assert_eq!(setup.pass()["passes"][0]["result"], "written");
    assert_eq!(setup.control()["due"], false);
    let calls = setup.prompts().len();

    setup.append(ON, &[line("user", "u-2", "One more turn.")]);
    setup.hook("stop", &setup.payload("Stop"));
    assert_eq!(setup.control()["due"], false, "one turn of two");
    setup.append(ON, &[line("user", "u-3", "And another.")]);
    let out = setup.hook("stop", &setup.payload("Stop"));
    assert!(out.stdout.is_empty());
    assert_eq!(setup.control()["due"], true, "two turns since the pass");

    setup.hook("session-end", &setup.payload("SessionEnd"));
    assert_eq!(setup.control()["final"], true);
    assert_eq!(setup.prompts().len(), calls, "the hooks call no model");

    // The final pass writes a final record and settles the switch.
    let report = setup.pass();
    assert_eq!(report["passes"][0]["result"], "written");
    assert_eq!(setup.record(ON).unwrap()["status"], "final");
    assert_eq!(setup.control()["due"], false);
    assert_eq!(setup.control()["final"], false);

    // By minutes: on since long ago, never passed, no new turns needed.
    let minutes = Setup::new();
    minutes.config("[catalog]\nevery_turns = 50\nevery_minutes = 5\n");
    let control = json!({"format": 31, "codex_cursors": {}, "sessions": {ON: {
        "provider": "claude-code", "on": true, "since": "2026-01-01T00:00:00Z",
        "due": false, "final": false, "consecutive_failures": 0,
        "retry_after": null, "pending_links": [], "cataloged_by": null}}});
    support::seed_records(&minutes.fx.root(), &[&control.to_string()]);
    minutes.hook("stop", &minutes.payload("Stop"));
    assert_eq!(minutes.control()["due"], true);
    assert!(minutes.prompts().is_empty());
}

#[test]
fn a_pass_refuses_when_a_configured_redact_name_is_unset() {
    // STD-05 R13: [redact] env names a value scribe must mask, but the
    // routine's environment (the plugin's env_pass, here the cleared test
    // environment) does not carry it. The pass refuses before any model
    // call or write, rather than storing text unmasked without warning.
    let setup = Setup::new();
    setup.config("[redact]\nenv = [\"SCRIBE_UNSET_REDACT_TOKEN\"]\n");
    setup.scribe("/scribe on");
    let out = setup.fx.check(
        with_env(&["session", "pass", "--json"], "PATH", &setup.path),
        Capture,
    );
    assert_ne!(out.status.code(), Some(0), "{}", text(&out.stdout));
    assert!(
        text(&out.stderr).contains("SCRIBE_UNSET_REDACT_TOKEN"),
        "{}",
        text(&out.stderr)
    );
    assert!(
        text(&out.stderr).contains("redact_env_unset")
            || text(&out.stderr).contains("[redact] env"),
        "{}",
        text(&out.stderr)
    );
    assert!(setup.record(ON).is_none(), "nothing was written");
    assert_eq!(setup.control()["due"], true, "the pass stays due");
    assert!(setup.prompts().is_empty(), "no model call before refusal");

    // The same selection can prepare and commit with the masking value set.
    let out = setup.run_pass(
        &["session", "pass", "--json"],
        &[("SCRIBE_UNSET_REDACT_TOKEN", "a-value-that-must-be-masked")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "written");
    assert!(setup.record(ON).is_some());
}

#[test]
fn a_pass_writes_one_record_of_daniels_terms_and_never_reads_a_session_never_switched_on() {
    let setup = Setup::new();
    let never_before = std::fs::read(setup.transcript(NEVER)).unwrap();
    // A Stop and a SessionEnd for the never-on session touch nothing.
    let mut never = setup.payload("Stop");
    never["session_id"] = json!(NEVER);
    never["transcript_path"] = json!(setup.transcript(NEVER));
    setup.hook("stop", &never);
    never["hook_event_name"] = json!("SessionEnd");
    setup.hook("session-end", &never);
    assert!(!support::has_control(&setup.fx.root()));

    setup.scribe("/scribe on");
    let report = setup.pass();
    let passes = report["passes"].as_array().unwrap();
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0]["session"], ON);
    assert_eq!(passes[0]["result"], "written");
    assert_eq!(passes[0]["dropped_terms"], 1, "the assistant's term");

    let record = setup.record(ON).unwrap();
    assert_eq!(record["scribed_through"], "a-1");
    assert_eq!(record["passes"], 1);
    assert_eq!(
        record["terms"],
        json!([{"term": "graduate to level 4", "said": DEFINED, "message": "u-1"}])
    );
    assert!(
        support::exported(&setup.fx.root(), "entries/preferences/preferences-1.md").is_none()
            && setup.entry_count() == 0,
        "no entries"
    );
    // The record is written at format 23: a header and one open segment
    // holding this pass's segment text.
    let file = setup.record_text();
    assert!(file.starts_with("---\nformat: 31\n"), "{file}");
    assert!(file.contains("\n## Log\n### 1. "), "{file}");
    assert!(!file.contains("## Outcome"), "{file}");
    assert_eq!(record["active"], true);
    assert_eq!(record["outcome"], "");
    assert_eq!(record["log"][0]["state"], "open");
    assert_eq!(record["log"][0]["origin"], "pass");
    assert_eq!(record["log"][0]["last_message"], "a-1");

    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains(DEFINED));
    assert!(!prompts[0].contains(NEVER_SAID));
    assert!(setup.record(NEVER).is_none());
    assert_eq!(
        std::fs::read(setup.transcript(NEVER)).unwrap(),
        never_before
    );

    // An invalid answer keeps the previous record and leaves the pass due.
    let before = setup.record_text().into_bytes();
    setup.append(ON, &[line("user", "u-2", "More on levels.")]);
    setup.scribe("/scribe off");
    setup.scribe("/scribe on");
    let mut over = answer();
    over["summary"] = json!(format!("{}\n## Injected", "s".repeat(1201)));
    setup.answer(&over);
    let out = setup.run_pass(&["session", "pass", "--json"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "failed");
    assert_eq!(report["passes"][0]["error_code"], "session_answer_invalid");
    assert_eq!(setup.record_text().into_bytes(), before);
    assert_eq!(setup.control()["due"], true);
}

/// A `claude` that cannot answer: exits 3 without output (`model_failed`).

#[test]
fn a_failed_pass_stays_due_exits_1_naming_it_and_the_next_run_retries_it() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.responses.replace(Responses::Failure(
        "model_failed",
        "the model failed".into(),
    ));
    // More failures in a row than the old give-up bound of three: each one
    // leaves the session due, whatever its code.
    for run in 1..=4 {
        let out = setup.run_pass(&["session", "pass", "--session", ON], &[]);
        assert_eq!(out.status.code(), Some(1), "run {run}");
        let stderr = text(&out.stderr);
        assert!(
            stderr.contains(&format!(
                "error: 1 session pass(es) failed: {ON} model_failed"
            )),
            "{stderr}"
        );
        assert!(
            text(&out.stdout).contains("failed"),
            "the report is printed"
        );
        assert_eq!(setup.control()["due"], true, "run {run} keeps it due");
        assert!(setup.record(ON).is_none());
    }
    // --json still prints the report, and the error object on stderr.
    let out = setup.run_pass(&["session", "pass", "--session", ON, "--json"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["error_code"], "model_failed");
    assert_eq!(report["passes"][0]["gave_up"], false);
    let reason = report["passes"][0]["error"].as_str().unwrap();
    assert!(reason.contains("model failed"), "{reason}");
    assert!(reason.chars().count() <= 300, "{reason}");
    // The model's failure is logged first; the error object is the last line.
    let stderr = text(&out.stderr);
    let error: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(error["code"], "session_pass_failed", "{stderr}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains(&format!("{ON} model_failed: {reason}")),
        "{}",
        error["error"]
    );
    let audit = support::audit_text(&setup.fx.root());
    assert!(
        !audit.contains(reason),
        "audit exposed the failure reason: {audit}"
    );

    // No claude at all (model_unavailable) is no different.
    setup.responses.replace(Responses::Failure(
        "model_unavailable",
        "the model could not be started".into(),
    ));
    let out = setup.run_pass(&["session", "pass", "--session", ON], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("model_unavailable"));
    assert_eq!(setup.control()["due"], true);

    // Once the model answers, the next run writes the record.
    setup.answer(&answer());
    let out = setup.pass_session(ON);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "written");
    assert!(report["passes"][0].get("error").is_none());
    assert!(setup.record(ON).is_some());
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn a_failed_cli_pass_with_closed_stdout_exits_quietly_with_zero() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    let setup = Setup::new();
    setup.scribe("/scribe on");
    std::fs::set_permissions(setup.transcript(ON), std::fs::Permissions::from_mode(0o620))
        .expect("make transcript fail its provenance check");

    // Close stdout's reader before the child starts so EPIPE is deterministic
    // when the failed pass report reaches stdout.
    let (reader, writer) = std::os::unix::net::UnixStream::pair().expect("create stdout stream");
    drop(reader);
    let mut command = setup.fx.cmd(&["session", "pass", "--session", ON]);
    command
        .env("PATH", &setup.path)
        .stdout(Stdio::from(std::os::fd::OwnedFd::from(writer)))
        .stderr(Stdio::piped());
    let child = command.spawn().expect("start failed session pass");
    let out = support::ChildGuard::new(child).finish();

    assert_eq!(setup.control()["due"], true, "the failed pass stays due");
    assert_eq!(
        setup.control()["consecutive_failures"],
        1,
        "the pass failed on the transcript provenance check"
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        !text(&out.stderr).contains("error:"),
        "stderr: {}",
        text(&out.stderr)
    );
}

#[test]
fn a_failed_cli_pass_with_writable_stdout_still_reports_exit_one() {
    use std::os::unix::fs::PermissionsExt;

    let setup = Setup::new();
    setup.scribe("/scribe on");
    std::fs::set_permissions(setup.transcript(ON), std::fs::Permissions::from_mode(0o620))
        .expect("make transcript fail its provenance check");

    let out = setup.run_any(&["session", "pass", "--session", ON], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("failed"),
        "stdout: {}",
        text(&out.stdout)
    );
    assert!(text(&out.stderr).contains("error: 1 session pass(es) failed"));
}

#[test]
fn a_failed_pass_redacts_the_model_reason_in_json_and_the_aggregate_error() {
    const SECRET: &str = "pass-error-secret-value-123";
    const SECRET_NAME: &str = "SCRIBE_PASS_ERROR_SECRET";
    let setup = Setup::new();
    setup.config(&format!("[redact]\nenv = [\"{SECRET_NAME}\"]\n"));
    setup.scribe("/scribe on");
    setup.responses.replace(Responses::Failure(
        "model_failed",
        format!("model error: {SECRET}"),
    ));
    let out = setup.run_pass(
        &["session", "pass", "--session", ON, "--json"],
        &[(SECRET_NAME, SECRET)],
    );

    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let reason = report["passes"][0]["error"].as_str().unwrap();
    assert!(
        reason.contains("[redacted:SCRIBE_PASS_ERROR_SECRET]"),
        "{reason}"
    );
    assert!(!reason.contains(SECRET), "{reason}");
    assert!(reason.chars().count() <= 300, "{reason}");
    let stderr = text(&out.stderr);
    let error: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(error["code"], "session_pass_failed", "{stderr}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains(&format!("{ON} model_failed: {reason}")),
        "{}",
        error["error"]
    );
}

#[test]
fn session_names_a_switched_on_session_whose_due_was_cleared_and_passes_it() {
    let setup = Setup::new();
    // A session stranded as an earlier give-up rule left it: on, not due,
    // no record.
    let control = json!({"format": 31, "codex_cursors": {}, "sessions": {ON: {
        "provider": "claude-code", "on": true, "since": "2026-11-03T05:00:00Z",
        "due": false, "final": false, "consecutive_failures": 0,
        "retry_after": null, "pending_links": [], "cataloged_by": null}}});
    setup.config("");
    support::seed_records(&setup.fx.root(), &[&control.to_string()]);
    assert_eq!(setup.pass()["passes"], json!([]), "nothing is due");
    assert!(setup.prompts().is_empty());

    let queued = setup.run_any(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(queued.status.code(), Some(0));
    let settlement: Value = serde_json::from_slice(&queued.stdout).unwrap();
    assert_eq!(settlement, json!({"passes": [], "remaining": 1}));
    assert!(text(&queued.stderr).contains("orbit job run scribe-session-pass"));
    assert!(setup.record(ON).is_none());

    let out = setup.run(
        &[
            "session",
            "pass",
            "--session",
            ON,
            "--session",
            ON,
            "--json",
        ],
        b"",
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let passes = report["passes"].as_array().unwrap();
    assert_eq!(passes.len(), 1, "named twice, passed once");
    assert_eq!(passes[0]["session"], ON);
    assert_eq!(passes[0]["result"], "written");
    assert_eq!(setup.record(ON).unwrap()["passes"], 1);
    assert_eq!(setup.control()["due"], false);

    // A session never switched on, or switched off, is refused and not read.
    let calls = setup.prompts().len();
    let out = setup.run_any(&["session", "pass", "--session", NEVER], b"");
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(&format!("session {NEVER} is not switched on")),
        "{stderr}"
    );
    setup.scribe("/scribe off");
    let out = setup.run_pass(&["session", "pass", "--session", ON, "--json"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "session_not_on");
    assert_eq!(setup.prompts().len(), calls, "no model call");
    assert!(setup.record(NEVER).is_none());
}

#[test]
fn the_plugin_failure_tool_records_backoff_and_keeps_the_session_due() {
    let setup = Setup::new();
    setup.config("[instructions]\nsession = 'Keep decisions for {user}.'\nsession_header = 'Shorten this for {user}.'\n");
    setup.scribe("/scribe on");
    let prepared = plugin_pass(&setup, "prepare", json!({}));
    assert_eq!(prepared["needs_model"], true);
    assert_eq!(prepared["session_distill_crew"], "haiku");
    assert_eq!(prepared["session_header_crew"], "haiku");
    assert_eq!(
        prepared["window_request"]["instructions"],
        "Keep decisions for the user."
    );
    let failed = plugin_pass(&setup, "fail", json!({"basis": prepared["basis"]}));
    assert_eq!(failed["pass"]["result"], "failed");
    assert_eq!(failed["pass"]["error_code"], "model_failed");
    assert_eq!(setup.control()["due"], true);

    let waiting = plugin_pass(&setup, "prepare", json!({}));
    assert_eq!(waiting["done"], true);
    assert_eq!(waiting["cursor"]["remaining"], 1);
    setup.expire_retry();
    let prepared = plugin_pass(&setup, "prepare", json!({}));
    let answer = setup
        .fixed_response(&prepared["window_request"], false)
        .unwrap();
    let mut over = answer.clone();
    over["summary"] = json!("s".repeat(1201));
    let retry = plugin_pass(
        &setup,
        "apply",
        json!({
            "basis": prepared["basis"], "cursor": prepared["cursor"], "answer": over,
        }),
    );
    assert_eq!(retry["status"], "retry_header");
    assert_eq!(
        retry["header_request"]["instructions"],
        "Shorten this for the user."
    );
    assert!(setup.record(ON).is_none());

    let reply = plugin_pass(
        &setup,
        "apply",
        json!({
            "basis": prepared["basis"], "cursor": prepared["cursor"], "answer": over,
            "repair": true, "previous_status": "retry_header", "header": pass::header(&answer),
        }),
    );
    assert_eq!(reply["status"], "written", "{reply}");
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn a_continuation_prepare_refuses_a_catalog_injected_onto_a_normal_cursor() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    let prepared = plugin_pass(&setup, "prepare", json!({"sessions": [ON]}));
    assert_eq!(prepared["needs_model"], true, "{prepared}");
    assert!(prepared["basis"]["catalog"].is_null(), "{prepared}");
    let control = setup.control_bytes();
    // A reserved author and a plausible one are both refused: the cursor
    // never carried a catalog, so the caller cannot add one.
    for catalog in [
        json!({"author": "daniel", "project": "elsewhere"}),
        json!({"author": "opus", "project": "elsewhere"}),
    ] {
        let (code, message) = plugin_pass_refusal(
            &setup,
            "prepare",
            json!({
                "sessions": [ON], "catalog": catalog,
                "progress": [{"cursor": prepared["cursor"]}],
            }),
        );
        assert_eq!(code, "invalid_input", "{message}");
        assert!(message.contains("catalog selection"), "{message}");
    }
    assert_eq!(setup.control_bytes(), control, "a refusal writes nothing");
    // The same continuation without a catalog still prepares.
    let again = plugin_pass(
        &setup,
        "prepare",
        json!({"sessions": [ON], "progress": [{"cursor": prepared["cursor"]}]}),
    );
    assert_eq!(again["needs_model"], true, "{again}");
}

#[test]
fn a_continuation_prepare_refuses_a_catalog_changed_mid_run() {
    let setup = Setup::new();
    let config = pass::claude_config(&setup.fx);
    let begun = pass::plugin_tool(
        &setup.fx,
        &[],
        "session-catalog-begin",
        json!({"session": NEVER, "author": "opus", "project": "first"}),
        &config,
    )
    .unwrap();
    let selection =
        json!({"sessions": begun["sessions"], "resolve": false, "catalog": begun["catalog"]});
    let prepared = plugin_pass(&setup, "prepare", selection.clone());
    assert_eq!(prepared["basis"]["catalog"]["author"], "opus", "{prepared}");
    let cursor = json!([{"cursor": prepared["cursor"]}]);
    let with = |catalog: Value| {
        let mut input = selection.clone();
        input["catalog"] = catalog;
        input["progress"] = cursor.clone();
        input
    };
    for catalog in [
        json!({"author": "daniel", "project": "first"}),
        json!({"author": "sonnet", "project": "first"}),
        json!({"author": "opus", "project": "second"}),
        json!({"author": "opus"}),
        Value::Null,
    ] {
        let (code, message) = plugin_pass_refusal(&setup, "prepare", with(catalog.clone()));
        assert_eq!(code, "invalid_input", "{catalog}: {message}");
        assert!(message.contains("catalog selection"), "{message}");
    }
    // The unchanged selection continues the run.
    let again = plugin_pass(&setup, "prepare", with(begun["catalog"].clone()));
    assert_eq!(again["basis"]["catalog"], begun["catalog"], "{again}");
    // A cursor a catalog run returned cannot start a normal pass either.
    let (code, _) = plugin_pass_refusal(
        &setup,
        "prepare",
        json!({"sessions": [NEVER], "progress": cursor}),
    );
    assert_eq!(code, "invalid_input");
}

#[test]
fn a_continuation_prepare_refuses_a_catalog_run_whose_switch_is_not_a_catalog_switch() {
    let setup = Setup::new();
    let config = pass::claude_config(&setup.fx);
    let begun = pass::plugin_tool(
        &setup.fx,
        &[],
        "session-catalog-begin",
        json!({"session": NEVER, "author": "opus"}),
        &config,
    )
    .unwrap();
    let selection =
        json!({"sessions": begun["sessions"], "resolve": false, "catalog": begun["catalog"]});
    let prepared = plugin_pass(&setup, "prepare", selection.clone());
    let mut next = selection;
    next["progress"] = json!([{"cursor": prepared["cursor"]}]);
    // The switch loses its catalog marker between iterations.
    support::set_switch(&setup.fx.root(), NEVER, "cataloged_by", "NULL");
    let (code, message) = plugin_pass_refusal(&setup, "prepare", next.clone());
    assert_eq!(code, "invalid_input", "{message}");
    assert!(message.contains("catalog switch"), "{message}");
    // So does a run that names no single session.
    next["sessions"] = json!([]);
    let (code, message) = plugin_pass_refusal(&setup, "prepare", next);
    assert_eq!(code, "invalid_input", "{message}");
}
