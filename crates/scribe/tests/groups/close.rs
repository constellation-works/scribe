#[test]
fn group_close_prepare_uses_haiku_for_unset_or_blank_crew_and_preserves_explicit_crew() {
    for (crew, expected) in [
        (None, "haiku"),
        (Some(""), "haiku"),
        (Some(" \t "), "haiku"),
        (Some("sonnet"), "sonnet"),
    ] {
        let f = Fixture::new();
        transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
        open(&f);
        let prepared = close_plugin_with_crew(
            &f,
            "group-close-prepare",
            json!({"group":"g-1", "session":A, "provider":"claude-code"}),
            crew,
        );
        assert_eq!(prepared["ok"], true, "{prepared}");
        assert_eq!(prepared["output"]["crew"], expected, "{crew:?}: {prepared}");
    }
}

#[test]
fn opener_closes_with_one_bounded_summary_and_the_project_brief_shows_it() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    f.config("[instructions]\ngroup_summary = \"Emphasize the release outcome.\"\n");
    let private = "ghp_abcdefghijklmnopqrstuvwxyzABCDEFGHIJ";
    let member = transcript(
        &f,
        B,
        &[row(
            B,
            "u",
            "user",
            &format!(
                "{QUOTE} sprucechecklist {private} {}",
                "longword ".repeat(100)
            ),
        )],
    );
    stop(&f, B, &member, &[]);
    let handoff = invoke(
        &f,
        B,
        &[
            "entry",
            "add",
            HANDOFF,
            "--tag",
            "handoff",
            "--author",
            "aide",
            "--related",
            &format!("session:{B}"),
            "--quote",
            QUOTE,
        ],
        0,
    );
    let before_prepare = support::audit_lines(&f.root());
    let prepared_cli = invoke(&f, A, &["group", "evidence", "g-1"], 0);
    let prepared_reply = close_plugin(
        &f,
        "group-close-prepare",
        json!({"group":"g-1", "session":A, "provider":"claude-code"}),
    );
    assert_eq!(prepared_reply["ok"], true, "{prepared_reply}");
    let prepared = &prepared_reply["output"];
    assert_eq!(prepared["evidence"], prepared_cli["evidence"]);
    assert_eq!(prepared["evidence_sha256"], prepared_cli["evidence_sha256"]);
    assert_eq!(prepared["crew"], "haiku");
    assert_eq!(prepared["instructions"], "Emphasize the release outcome.");
    assert_eq!(support::audit_lines(&f.root()), before_prepare);
    let missing = f.check(
        with_env(&["group", "close", "g-1"], "CLAUDE_CODE_SESSION_ID", A),
        Code(2),
    );
    assert!(text(&missing.stderr).contains("scribe group evidence"));
    assert!(text(&missing.stderr).contains("scribe-group-close"));
    let evidence = &prepared["evidence"];
    let digest = prepared["evidence_sha256"].as_str().unwrap();
    let summary = format!("Review complete. {private}");
    let close = |session| {
        f.check(
            with_env(
                &[
                    "group",
                    "close",
                    "g-1",
                    "--summary",
                    &summary,
                    "--if-evidence",
                    digest,
                    "--json",
                ],
                "CLAUDE_CODE_SESSION_ID",
                session,
            ),
            Capture,
        )
    };
    let denied = close(B);
    assert_eq!(denied.status.code(), Some(1));
    let refused: Value = serde_json::from_slice(&denied.stderr).unwrap();
    assert_eq!(refused["code"], "group_membership", "{refused}");
    assert_eq!(
        refused["error"],
        "only the opening session or the current holder of its seat can close g-1; any other \
         session can close it with --quote '<span>', where <span> is a verbatim span of a user \
         turn typed in that session that contains the whole canonical group id g-1 and is at \
         least eight characters long after normalization",
        "{refused}"
    );
    assert!(show_group(&f, A, "g-1")["closed"].is_null());
    let after_denied: Vec<_> = support::audit_lines(&f.root())
        .into_iter()
        .filter(|line| line["command"] == "group close")
        .collect();
    assert_eq!(after_denied.len(), 1, "{after_denied:?}");
    assert_eq!(after_denied[0]["status"], "failure");
    assert_eq!(after_denied[0]["target"], "g-1");
    assert_eq!(after_denied[0]["error_code"], "group_membership");
    assert_eq!(after_denied[0]["instruction_session"], B);

    let applied = close_plugin(
        &f,
        "group-close-apply",
        json!({"group":"g-1", "session":A, "provider":"claude-code", "summary":summary, "evidence_sha256":digest}),
    );
    assert_eq!(applied["ok"], true, "{applied}");
    assert_eq!(applied["output"]["changed"], true);
    let out = close(A);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let close_successes: Vec<_> = support::audit_lines(&f.root())
        .into_iter()
        .filter(|line| line["command"] == "group close" && line["status"] == "success")
        .collect();
    assert_eq!(close_successes.len(), 1, "{close_successes:?}");
    assert_eq!(close_successes[0]["target"], "g-1");
    assert!(close_successes[0]["error_code"].is_null());
    assert!(close_successes[0]["author"].is_null());
    assert_eq!(close_successes[0]["instruction_session"], A);
    assert!(close_successes[0]["instruction_message"].is_null());
    let audit_text = support::audit_text(&f.root());
    assert!(!audit_text.contains("Review complete."));
    assert!(!audit_text.contains("sprucechecklist"));
    assert!(!audit_text.contains(private));
    assert!(evidence["title"].is_string(), "{evidence}");
    assert!(evidence["handoffs"].is_array(), "{evidence}");
    assert!(evidence["turns"].is_array(), "{evidence}");
    assert!(
        evidence["handoffs"]
            .to_string()
            .contains(handoff["id"].as_str().unwrap()),
        "{evidence}"
    );
    assert!(
        evidence["turns"].to_string().contains("sprucechecklist"),
        "{evidence}"
    );
    let turn = evidence["turns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|turn| turn[2].as_str().unwrap().contains("sprucechecklist"))
        .unwrap();
    assert_eq!(turn.as_array().unwrap().len(), 3, "{turn}");
    assert_eq!(turn[0], B, "{turn}");
    assert_eq!(turn[1], "u", "{turn}");
    assert!(!evidence.to_string().contains(private));
    assert!(evidence.to_string().len() < 2500);
    let shown = show_group(&f, A, "g-1");
    assert!(!shown["closed"].is_null());
    let summary = shown["summary"].as_str().unwrap();
    assert!(summary.contains("Review complete."));
    assert!(!summary.contains(private));
    assert!(summary.contains("task:example"));
    let report = invoke(&f, A, &["brief", "--project", "scribe"], 0);
    assert_eq!(report["last_group"]["id"], "g-1");
    assert_eq!(report["last_group"]["summary"], summary);
    assert!(
        f.check(Cli(&["brief", "--project", "scribe"]), Text)
            .contains(summary)
    );
    let member = transcript(&f, B, &[row(B, "after", "user", "aftercloseorchid")]);
    stop(&f, B, &member, &[]);
    assert_eq!(
        invoke(
            &f,
            A,
            &[
                "search",
                "aftercloseorchid",
                "--kind",
                "message",
                "--group",
                "g-1"
            ],
            0
        )["total"],
        0
    );
    let left = invoke(&f, B, &["group", "leave"], 0);
    assert!(left["summary"].is_null());
    assert!(!left["closed"].is_null());
}

#[test]
fn the_newest_transcript_of_the_openers_seat_closes_after_a_replace() {
    let f = seated_group();
    join_as(&f, C, "g-1", "editor", &[], 0);
    join_as(&f, B, "g-1", "chief", &["--replace"], 0);
    // The replaced opener and the newest transcript of its seat both pass;
    // other sessions do not.
    assert_close_refused(&f, D);
    assert_close_refused(&f, C);
    for _ in 0..2 {
        let out = close_as(&f, B);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let successes: Vec<_> = group_close_audits(&f)
        .into_iter()
        .filter(|line| line["status"] == "success")
        .collect();
    assert_eq!(successes.len(), 1, "{successes:?}");
    assert_eq!(successes[0]["instruction_session"], B);
    assert!(!show_group(&f, B, "g-1")["closed"].is_null());
    // An unauthorized caller is refused on a closed group, not told it is closed.
    assert_close_refused(&f, D);
    assert_close_refused(&f, C);
}

#[test]
fn the_newest_transcript_of_the_openers_seat_closes_after_a_clear() {
    let f = seated_group();
    cleared_transcript(&f, B, D, &[A]);
    join_as(&f, B, "g-1", "chief", &[], 0);
    let out = close_as(&f, B);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let again = close_as(&f, B);
    assert_eq!(again.status.code(), Some(0), "{}", text(&again.stderr));
}

#[test]
fn a_replaced_transcript_that_is_not_the_opener_cannot_close() {
    let f = seated_group();
    join_as(&f, B, "g-1", "chief", &["--replace"], 0);
    join_as(&f, C, "g-1", "chief", &["--replace"], 0);
    assert_close_refused(&f, B);
    // The opener itself still closes, as before.
    let out = close_as(&f, A);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn a_session_that_left_the_openers_seat_cannot_close() {
    let f = seated_group();
    join_as(&f, B, "g-1", "chief", &["--replace"], 0);
    invoke(&f, B, &["group", "leave"], 0);
    assert_close_refused(&f, B);
    assert_close_refused(&f, D);
}

/// A closer whose own transcript cannot be verified is refused, and the refusal
/// names that session's transcript rather than the opener's, for the opener and
/// for the seat holder that replaced it, through `group close`, `group
/// evidence` and the job's prepare and apply tools alike.
#[test]
fn a_closer_whose_own_transcript_cannot_be_verified_is_named_as_itself() {
    let f = seated_group();
    join_as(&f, B, "g-1", "chief", &["--replace"], 0);
    let digest = invoke(&f, B, &["group", "evidence", "g-1"], 0)["evidence_sha256"].clone();
    std::fs::remove_file(transcript_path(&f, A)).unwrap();
    std::fs::remove_file(transcript_path(&f, B)).unwrap();
    for (closer, other) in [(B, A), (A, B)] {
        let error = assert_close_refused(&f, closer);
        let message = error["error"].as_str().unwrap();
        assert!(
            message.contains(&format!("transcript of session {closer} ")),
            "{message}"
        );
        assert!(!message.contains(other), "{message}");
        let read = invoke(&f, closer, &["group", "evidence", "g-1"], 1);
        assert_eq!(read["code"], "group_membership", "{read}");
        assert_eq!(read["error"], message, "{read}");
        let input = json!({"group": "g-1", "session": closer, "provider": "claude-code"});
        let prepared = close_plugin(&f, "group-close-prepare", input.clone());
        assert_eq!(prepared["ok"], false, "{prepared}");
        assert_eq!(prepared["error"]["code"], "group_membership", "{prepared}");
        let mut input = input;
        input["summary"] = json!("Review complete.");
        input["evidence_sha256"] = digest.clone();
        let applied = close_plugin(&f, "group-close-apply", input);
        assert_eq!(applied["ok"], false, "{applied}");
        assert_eq!(applied["error"]["code"], "group_membership", "{applied}");
    }
    assert!(show_group(&f, A, "g-1")["closed"].is_null());
    // A session that may not close is still refused as one, not for its
    // missing transcript.
    let error = assert_close_refused(&f, C);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .starts_with("only the opening session"),
        "{error}"
    );
}

/// Once closed, a group's close, evidence and prepare all end without error,
/// even for an authorized closer whose transcript is gone; an unauthorized
/// caller is still refused.
#[test]
fn an_already_closed_group_ends_clean_for_its_closer() {
    let f = seated_group();
    let out = close_as(&f, A);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let shown = show_group(&f, A, "g-1");
    std::fs::remove_file(transcript_path(&f, A)).unwrap();
    let again = close_as(&f, A);
    assert_eq!(again.status.code(), Some(0), "{}", text(&again.stderr));
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(again, shown);
    let read = invoke(&f, A, &["group", "evidence", "g-1"], 0);
    assert_eq!(read, json!({"id": "g-1", "closed": true}));
    let prepared = close_plugin(
        &f,
        "group-close-prepare",
        json!({"group": "g-1", "session": A, "provider": "claude-code"}),
    );
    assert_eq!(prepared["ok"], true, "{prepared}");
    assert_eq!(prepared["output"]["closed"], true, "{prepared}");
    assert!(prepared["output"]["evidence"].is_null(), "{prepared}");
    assert_eq!(show_group(&f, A, "g-1"), shown);
    let refused = invoke(&f, C, &["group", "evidence", "g-1"], 1);
    assert_eq!(refused["code"], "group_membership", "{refused}");
}

#[test]
fn an_opener_without_a_seat_is_the_only_closer() {
    let f = Fixture::new();
    for id in [A, B, C] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    open(&f);
    join_as(&f, B, "g-1", "chief", &[], 0);
    assert_close_refused(&f, B);
    assert_close_refused(&f, C);
    f.check(
        with_env(
            &["group", "close", "g-1", "--no-summary", "--json"],
            "CLAUDE_CODE_SESSION_ID",
            A,
        ),
        Success,
    );
    let group = show_group(&f, A, "g-1");
    assert!(
        group["summary"]
            .as_str()
            .unwrap()
            .starts_with("For work missing from the handoffs")
    );
}

#[test]
fn a_quoted_request_closes_the_group_for_a_session_with_no_seat() {
    let f = Fixture::new();
    for id in [A, B] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    transcript(
        &f,
        D,
        &[
            row(
                D,
                "u-close",
                "user",
                "please close g-1 once the release ships",
            ),
            row(
                D,
                "u-clipped",
                "user",
                "please close g-12 once the release ships",
            ),
        ],
    );
    open(&f);
    join_as(&f, B, "g-1", "chief", &[], 0);
    // This eligible span is shorter than the typed turn that contains it.
    let quote = "please close g-1";
    // Without a quote D holds no seat, so the opener's rule still refuses it.
    assert_close_refused(&f, D);

    let out = close_with(&f, D, &["--quote", quote]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let successes: Vec<_> = group_close_audits(&f)
        .into_iter()
        .filter(|line| line["status"] == "success")
        .collect();
    assert_eq!(successes.len(), 1, "{successes:?}");
    assert_eq!(successes[0]["instruction_session"], D);
    assert_eq!(successes[0]["instruction_message"], "u-close");
    let group = show_group(&f, A, "g-1");
    // The supplied summary leads; the group's related references follow it.
    assert!(
        group["summary"]
            .as_str()
            .is_some_and(|summary| summary.starts_with("Review complete.")),
        "{group}"
    );
    assert!(!group["seats"].to_string().contains(D), "{group}");

    // A retry returns the closed record with no second success audit.
    let audits = group_close_audits(&f).len();
    let again = close_with(&f, D, &["--quote", quote]);
    assert_eq!(again.status.code(), Some(0), "{}", text(&again.stderr));
    assert_eq!(group_close_audits(&f).len(), audits);
    assert_eq!(show_group(&f, A, "g-1"), group);
}

#[test]
fn a_quoted_close_is_refused_unless_a_typed_span_of_this_session_names_the_group() {
    let f = Fixture::new();
    for id in [A, B] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    open(&f);
    join_as(&f, B, "g-1", "chief", &[], 0);
    let mut harness = row(D, "u-harness", "user", "close g-1 from the harness");
    harness["isMeta"] = json!(true);
    transcript(
        &f,
        D,
        &[
            row(D, "u-typed", "user", "close g-1 once the release ships"),
            row(D, "u-other", "user", "finish g-12 after the audit"),
            row(D, "u-prefix", "user", "close xg-1 after review"),
            row(D, "u-unicode", "user", "close g-1é after review"),
            row(B, "u-copied", "user", "close g-1 copied from the parent"),
            harness,
        ],
    );
    for (quote, code) in [
        // Not in any turn typed in this session.
        ("close g-1 right now", "quote_not_found"),
        // Typed in another session's transcript.
        ("close g-1 copied from the parent", "quote_not_found"),
        // Only in a harness line, which is not a typed turn.
        ("close g-1 from the harness", "quote_not_found"),
    ] {
        assert_close_refused_with(&f, D, &["--quote", quote], code);
    }
    // Verbatim in a typed turn, but the quote does not contain g-1 at all.
    let absent =
        assert_close_refused_with(&f, D, &["--quote", "the release ships"], "group_membership");
    assert_eq!(
        absent["error"],
        "the quote must contain g-1, the group it closes"
    );
    // The quote holds g-1, but where it matches the typed turn extends it
    // into a longer id (g-12, or a word such as xg-1 or g-1é, including past
    // the quote's edge as in "finish g-1" clipped out of "finish g-12 after
    // the audit"), so the refusal says what names it.
    let clipped_message = "the quote must name g-1 as a whole id, the group it closes; g-1 is not standalone in the matched span, where the typed turn extends it into a longer id, so quote a span where g-1 stands alone";
    for quote in [
        "finish g-12 after the audit",
        "finish g-1",
        "close g-1é",
        "g-1 after review",
    ] {
        let clipped = assert_close_refused_with(&f, D, &["--quote", quote], "group_membership");
        assert_eq!(clipped["error"], clipped_message, "{quote}");
    }
    assert!(show_group(&f, A, "g-1")["closed"].is_null());
}

#[test]
fn a_quoted_close_whose_span_clips_the_id_is_refused_even_when_its_turn_names_it_whole() {
    let f = Fixture::new();
    for id in [A, B] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    // One typed turn: the first span clips g-1 out of g-12, and a later span
    // of the same turn closes g-1 standalone.
    transcript(
        &f,
        D,
        &[row(
            D,
            "u-both",
            "user",
            "finish g-12 after the audit, then close g-1",
        )],
    );
    open(&f);
    join_as(&f, B, "g-1", "chief", &[], 0);
    // The submitted span clips the id, so the refusal names that span, not the turn.
    let clipped = assert_close_refused_with(&f, D, &["--quote", "finish g-1"], "group_membership");
    assert_eq!(
        clipped["error"],
        "the quote must name g-1 as a whole id, the group it closes; g-1 is not standalone in the matched span, where the typed turn extends it into a longer id, so quote a span where g-1 stands alone"
    );
    // The standalone span of the same turn is what the close is authorized by.
    let out = close_with(&f, D, &["--quote", "close g-1"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn a_quoted_close_under_eight_characters_is_refused_even_when_it_names_the_group() {
    let f = Fixture::new();
    for id in [A, B] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    transcript(
        &f,
        D,
        &[row(D, "u-close", "user", "end g-1 after the release")],
    );
    open(&f);
    join_as(&f, B, "g-1", "chief", &[], 0);
    // Seven characters name g-1 but fall below the shared eight-character minimum.
    assert_close_refused_with(&f, D, &["--quote", "end g-1"], "quote_not_found");
    // Thirteen characters from the same typed turn clear the minimum and name g-1.
    let out = close_with(&f, D, &["--quote", "end g-1 after"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[cfg(unix)]
#[test]
fn a_quoted_close_keeps_a_transcript_io_failure_apart_from_missing_evidence() {
    use std::os::unix::fs::PermissionsExt;

    let f = Fixture::new();
    for id in [A, B] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    let quoted = transcript(
        &f,
        D,
        &[row(
            D,
            "u-close",
            "user",
            "please close g-1 once the release ships",
        )],
    );
    open(&f);
    join_as(&f, B, "g-1", "chief", &[], 0);
    let args = ["--quote", "please close g-1"];
    let unreadable = std::fs::read(&quoted).unwrap();
    let projects = quoted.parent().unwrap().to_path_buf();

    // Missing evidence and a provenance refusal fail closed as not found.
    std::fs::remove_file(&quoted).unwrap();
    assert_close_refused_with(&f, D, &args, "quote_not_found");
    std::fs::write(&quoted, &unreadable).unwrap();
    std::fs::set_permissions(&quoted, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert_close_refused_with(&f, D, &args, "quote_not_found");
    support::make_private(&quoted);

    // A read that fails on an existing transcript keeps its I/O diagnostic and
    // audit code; the request itself was valid.
    {
        let _restore = RestoreMode::lock(&quoted, 0o600);
        assert_close_refused_with(&f, D, &args, "io_error");
        let out = close_with(&f, D, &args);
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert!(
            error["error"]
                .as_str()
                .is_some_and(|m| m.contains("cannot read")
                    && m.contains(&format!("{D}.jsonl"))
                    && m.contains("Permission denied")),
            "{error}"
        );
    }
    // So does a discovery failure, before any transcript is opened.
    {
        let _restore = RestoreMode::lock(&projects, 0o700);
        assert_close_refused_with(&f, D, &args, "io_error");
    }

    let out = close_with(&f, D, &args);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));

    // A retry on the closed group verifies the quote first: unreadable
    // evidence is `io_error`, absent evidence is `quote_not_found`, and
    // neither adds a model call or changes the closed record.
    let group = show_group(&f, A, "g-1");
    {
        let _restore = RestoreMode::lock(&quoted, 0o600);
        assert_close_refused_with(&f, D, &args, "io_error");
    }
    {
        let _restore = RestoreMode::lock(&projects, 0o700);
        assert_close_refused_with(&f, D, &args, "io_error");
    }
    std::fs::remove_file(&quoted).unwrap();
    assert_close_refused_with(&f, D, &args, "quote_not_found");
    assert_eq!(show_group(&f, A, "g-1"), group);
}
