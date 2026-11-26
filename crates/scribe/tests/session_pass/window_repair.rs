#[test]
fn header_overruns_retry_with_only_the_header_and_preserve_the_segments() {
    let setup = Setup::new();
    let mut over = answer();
    over["summary"] = json!("é".repeat(1201));
    let compressed = json!({
        "title": "Defining crew levels",
        "summary": "A compact summary.",
        "goals": "Name the crew levels.",
        "next": ["write it into the routing doc"]
    });
    setup.answers(&[Some(over), Some(compressed)]);
    setup.scribe("/scribe on");

    let report = setup.pass();

    assert_eq!(report["passes"][0]["windows"], 1);
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0].contains(DEFINED));
    assert!(
        prompts[0].contains("summary: at most 900 characters"),
        "ORB-23950: leave summary headroom"
    );
    assert!(
        prompts[1].contains("Compress only"),
        "ORB-23950: compress rather than resend the window"
    );
    assert!(
        prompts[1].contains("Current summary length: 1201 characters"),
        "ORB-23950: count Unicode characters for the model"
    );
    assert!(prompts[1].contains(&"é".repeat(1201)));
    for excluded in [
        DEFINED,
        "Level 4 is defined.",
        "--- user",
        "--- assistant",
        "terms:",
    ] {
        assert!(!prompts[1].contains(excluded), "{excluded} resent");
    }
    let record = setup.record(ON).unwrap();
    assert_eq!(record["summary"], "A compact summary.");
    assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
    assert_eq!(record["terms"].as_array().unwrap().len(), 1);
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn a_replayed_transcript_is_read_once_and_a_segment_after_the_replay_is_accepted() {
    // ORB-24918: Claude Code wrote earlier lines into the transcript again on
    // resume. The pass resumed after the first copy of `scribed_through`,
    // re-read the replay, and refused every start in it as not following the
    // open segment.
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    assert_eq!(setup.record(ON).unwrap()["scribed_through"], "a-1");
    setup.append(
        ON,
        &[
            replayed("user", "u-1", &format!("Crews. {DEFINED}.")),
            replayed(
                "assistant",
                "a-1",
                "Level 4 crews merge their own pull requests after green CI.",
            ),
            line("user", "u-2", "Fresh work after the resume."),
        ],
    );
    let mut body = answer();
    body["terms"] = json!([]);
    body["new"][0]["heading"] = json!("After the resume");
    body["new"][0]["text"] = json!("Fresh work.");
    answer_at_window_first(&setup, &body);

    let out = setup.pass_session(ON);

    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "written", "{report}");
    assert_eq!(report["passes"][0]["windows"], 1);
    assert_eq!(report["passes"][0]["warnings"], json!([]), "{report}");
    assert!(
        !text(&out.stderr).contains("refused"),
        "{}",
        text(&out.stderr)
    );
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 2, "one call per pass, no retry");
    assert!(prompts[1].contains("· u-2 ---"));
    for replay in ["· u-1 ---", "· a-1 ---"] {
        assert!(!prompts[1].contains(replay), "{replay} read again");
    }
    let record = setup.record(ON).unwrap();
    let log = record["log"].as_array().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0]["state"], "sealed");
    assert_eq!(log[0]["last_message"], "a-1");
    assert_eq!(log[1]["first_message"], "u-2");
    assert_eq!(log[1]["heading"], "After the resume");
    assert_eq!(record["scribed_through"], "u-2");

    // A second replay with nothing new needs no model call.
    setup.append(
        ON,
        &[
            replayed("user", "u-1", &format!("Crews. {DEFINED}.")),
            replayed("user", "u-2", "Fresh work after the resume."),
        ],
    );
    setup.pass_session(ON);
    assert_eq!(setup.prompts().len(), 2);
    assert_eq!(setup.record(ON).unwrap()["passes"], record["passes"]);
}

#[test]
fn next_and_segment_text_get_a_target_and_their_current_length() {
    // ORB-24918: `next` and segment text had only their hard limits, so a
    // record carrying ~900 characters of `next` forward overshot 1000 when
    // the model added an item.
    let setup = Setup::new();
    let mut first = answer();
    let items: Vec<String> = (0..9)
        .map(|n| format!("item {n} {}", "x".repeat(100)))
        .collect();
    first["next"] = json!(items);
    first["new"][0]["text"] = json!("é".repeat(650));
    setup.answer(&first);
    setup.scribe("/scribe on");
    setup.pass();
    let record = setup.record(ON).unwrap();
    let next_chars = record["next"].as_str().unwrap().chars().count();
    assert!(next_chars > 900, "{next_chars}");

    setup.append(ON, &[line("user", "u-2", "One more step.")]);
    let mut over = answer();
    over["new"] = json!([]);
    over["open"] = json!(null);
    let mut more = items.clone();
    more.push(format!("item 9 {}", "x".repeat(100)));
    over["next"] = json!(more);
    let compressed = json!({
        "title": "Defining crew levels",
        "summary": "Daniel defined what level 4 means for crews.",
        "goals": "Name the crew levels.",
        "next": ["write it into the routing doc"]
    });
    // The fake numbers calls across runs: the first pass was call 0.
    setup.answers(&[None, Some(over), Some(compressed)]);

    let out = setup.run(&["session", "pass", "--session", ON], b"");

    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 3);
    let window = &prompts[1];
    assert!(
        window.contains(
            "next: the open next steps, one short item each, at most 750 characters in all \
             (hard limit 1000), counting a `- ` prefix per item and a newline between items"
        ),
        "{window}"
    );
    assert!(window.contains(&format!("Current next length: {next_chars} characters.")));
    assert!(window.contains("Segment text is at most 700 characters (hard limit 900)"));
    assert!(window.contains("text length: 650 characters\n"));
    let compress = &prompts[2];
    assert!(compress.starts_with("Compress only"));
    assert!(compress.contains("next: at most 750 characters total (hard limit 1000)"));
    assert!(compress.contains("Current next length: 1099 characters."));
    // The retry resolved the refusal: nothing at warn level.
    assert!(
        !text(&out.stderr).contains("refused"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        setup.record(ON).unwrap()["next"],
        "- write it into the routing doc"
    );
}

#[test]
fn omitted_or_null_open_keeps_its_content_and_still_advances_and_seals() {
    for omitted in [true, false] {
        let setup = Setup::new();
        setup.scribe("/scribe on");
        setup.pass();
        let before = setup.record(ON).unwrap()["log"][0].clone();
        setup.append(ON, &[line("user", "u-2", "Continue the thread.")]);
        setup.hook("session-end", &setup.payload("SessionEnd"));
        let mut body = answer();
        body["new"] = json!([]);
        if omitted {
            body.as_object_mut().unwrap().remove("open");
        }
        setup.answer(&body);

        setup.pass();

        let record = setup.record(ON).unwrap();
        let segment = &record["log"][0];
        for field in [
            "heading",
            "text",
            "topics",
            "related",
            "first_message",
            "from",
        ] {
            assert_eq!(segment[field], before[field], "{field}");
        }
        assert_eq!(segment["last_message"], "u-2");
        assert_eq!(segment["windows"], 2);
        assert_eq!(segment["state"], "sealed");
        assert_eq!(record["scribed_through"], "u-2");
        assert_eq!(setup.prompts().len(), 2, "no retry needed");
        assert!(
            setup.prompts()[1].contains(&format!(
                "Current summary length: {} characters",
                answer()["summary"].as_str().unwrap().chars().count()
            )),
            "ORB-23950: tell the model the previous summary length"
        );
    }
}

#[test]
fn unexpected_open_becomes_the_first_new_segment() {
    let setup = Setup::new();
    let mut body = answer();
    body["open"] = json!({"heading": "Opening thread", "text": "Opening details.", "topics": [], "related": []});
    body["new"][0]["starts_at"] = json!("a-1");
    setup.answer(&body);
    setup.scribe("/scribe on");

    setup.pass();

    let record = setup.record(ON).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 2);
    assert_eq!(record["log"][0]["text"], "Opening details.");
    assert_eq!(record["log"][0]["first_message"], "u-1");
    assert_eq!(record["log"][0]["last_message"], "u-1");
    assert_eq!(record["log"][0]["state"], "sealed");
    assert_eq!(record["log"][1]["first_message"], "a-1");
    assert_eq!(setup.prompts().len(), 1);
}

#[test]
fn repeated_header_budget_misses_are_repaired_after_retry_and_reported() {
    for (field, value, expected, boundary) in [
        (
            "summary",
            json!(format!("Keep this sentence. {}", "é".repeat(1200))),
            "Keep this sentence.".to_owned(),
            "sentence",
        ),
        (
            "goals",
            json!(format!("Keep these words {}", "é".repeat(600))),
            "Keep these words…".to_owned(),
            "word",
        ),
        (
            "summary",
            json!(format!("Keep this sentence.\" {}", "é".repeat(1200))),
            "Keep this sentence.\"".to_owned(),
            "sentence",
        ),
        (
            "title",
            json!("é".repeat(121)),
            format!("{}…", "é".repeat(119)),
            "word",
        ),
        (
            "next",
            json!(["Keep this bullet", "é".repeat(1000)]),
            "- Keep this bullet".to_owned(),
            "bullet",
        ),
        (
            "next",
            json!([format!("Keep these words {}", "é".repeat(1000))]),
            "- Keep these words…".to_owned(),
            "word",
        ),
    ] {
        let setup = Setup::new();
        let mut over = answer();
        over[field] = value;
        setup.answers(&[Some(over.clone()), Some(over)]);
        setup.scribe("/scribe on");

        let out = setup.run_pass(&["session", "pass", "--json"], &[]);

        assert_eq!(out.status.code(), Some(0), "{field}: {}", text(&out.stderr));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["passes"][0]["windows"], 1);
        let warning = report["passes"][0]["warnings"][0].as_str().unwrap();
        assert!(
            warning.contains(field) && warning.contains(boundary),
            "{warning}"
        );
        assert!(text(&out.stderr).contains(warning));
        assert_eq!(setup.prompts().len(), 2, "repair follows the retry");
        assert!(!setup.prompts()[1].contains(DEFINED), "header-only retry");
        let record = setup.record(ON).unwrap();
        assert_eq!(record[field], expected, "{field}");
        assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
        assert_eq!(record["scribed_through"], "a-1");
        assert_eq!(setup.control()["due"], false);
    }
}

#[test]
fn segment_text_repairs_keep_new_segments_and_redact_before_writing() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.append(
        ON,
        &[
            line("user", "u-2", "Second."),
            line("assistant", "a-2", "Third."),
        ],
    );
    let mut over = answer();
    over["open"] = json!({"heading": "First", "text": format!("Keep first. {}", "é".repeat(900)), "topics": [], "related": []});
    over["new"] = json!([
        {"starts_at": "u-2", "heading": "Second", "text": format!("Keep sensitive-value-123. {}", "é".repeat(900)), "topics": [], "related": []},
        {"starts_at": "a-2", "heading": "Third", "text": format!("Keep third. {}", "é".repeat(900)), "topics": [], "related": []}
    ]);
    setup.answer(&over);
    setup.config("[redact]\nenv = [\"TEST_SECRET\"]\n");
    let out = setup.run_pass(
        &["session", "pass", "--session", ON, "--json"],
        &[("TEST_SECRET", "sensitive-value-123")],
    );

    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["warnings"].as_array().unwrap().len(), 3);
    assert_eq!(report["passes"][0]["segments_sealed"], 2);
    assert_eq!(setup.prompts().len(), 3, "initial call, miss and one retry");
    // Reading the stored record does not require loading the model config.
    let record = setup.record(ON).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 3);
    assert_eq!(record["log"][0]["text"], "Keep first.");
    assert_eq!(record["log"][0]["last_message"], "a-1");
    assert_eq!(record["log"][1]["text"], "Keep [redacted:TEST_SECRET].");
    assert_eq!(record["log"][1]["first_message"], "u-2");
    assert_eq!(record["log"][2]["text"], "Keep third.");
    assert_eq!(record["log"][2]["state"], "open");
    assert!(!setup.record_text().contains("sensitive-value-123"));
}

#[test]
fn starts_at_repairs_warn_in_plain_output_and_preserve_sealed_segments() {
    for overflow in [false, true] {
        let setup = Setup::new();
        setup.scribe("/scribe on");
        let mut initial = answer();
        initial["new"] = json!([
            {"starts_at": "u-1", "heading": "Sealed", "text": "Sealed history.", "topics": [], "related": []},
            {"starts_at": "a-1", "heading": "Open", "text": "Open history.", "topics": [], "related": []}
        ]);
        setup.answer(&initial);
        setup.pass();
        let before = setup.record(ON).unwrap();
        setup.append(ON, &[line("user", "u-2", "More.")]);
        let mut bad = answer();
        bad["new"][0]["starts_at"] = json!("a-1");
        bad["new"][0]["text"] = json!(if overflow {
            "é".repeat(900)
        } else {
            "Continued history.".to_owned()
        });
        setup.answer(&bad);

        let out = setup.run(&["session", "pass", "--session", ON], b"");

        assert!(text(&out.stderr).contains("window 1: repaired new segment 1 starts_at"));
        assert!(text(&out.stdout).contains("written"));
        assert_eq!(setup.prompts().len(), 3);
        let record = setup.record(ON).unwrap();
        assert_eq!(record["log"][0], before["log"][0]);
        assert_eq!(record["scribed_through"], "u-2");
        if overflow {
            assert_eq!(record["log"].as_array().unwrap().len(), 3);
            assert_eq!(record["log"][2]["first_message"], "u-2");
            assert_eq!(record["log"][2]["text"], "é".repeat(900));
        } else {
            assert_eq!(record["log"].as_array().unwrap().len(), 2);
            assert_eq!(
                record["log"][1]["text"],
                "Open history.\n\nContinued history."
            );
        }
    }
}

#[test]
fn header_length_retries_cover_every_prose_field_and_recheck_the_result() {
    for (field, value) in [
        ("title", json!("t".repeat(121))),
        ("goals", json!("g".repeat(601))),
        ("next", json!(["n".repeat(999)])),
    ] {
        let setup = Setup::new();
        let mut over = answer();
        over[field] = value;
        over["summary"] = json!("é".repeat(1201));
        setup.answers(&[Some(over), Some(answer())]);
        setup.scribe("/scribe on");

        assert_eq!(setup.pass()["passes"][0]["result"], "written", "{field}");
        assert_eq!(setup.prompts().len(), 2);
        assert!(
            !setup.prompts()[1].contains(DEFINED),
            "window resent for {field}"
        );
        assert_eq!(
            setup.record(ON).unwrap()[field],
            match field {
                "next" => json!("- write it into the routing doc"),
                _ => answer()[field].clone(),
            }
        );
    }

    // A shorter header can still be invalid; it receives no third attempt.
    for (field, value, reason) in [
        ("summary", json!(""), "summary is empty"),
        (
            "goals",
            json!("## Injected"),
            "Goals holds a section heading",
        ),
        (
            "title",
            json!("two\nlines"),
            "title must be one non-empty line",
        ),
        ("model_failure", Value::Null, "the model failed"),
    ] {
        let setup = Setup::new();
        setup.scribe("/scribe on");
        setup.pass();
        let before = setup.record_text().into_bytes();
        setup.append(ON, &[line("user", "u-2", "A new turn.")]);
        setup.hook("session-end", &setup.payload("SessionEnd"));
        let mut over = answer();
        over["summary"] = json!("s".repeat(1201));
        let mut invalid = answer();
        invalid[field] = value;
        // Script indices include the already successful initial pass.
        setup.answers(&[
            Some(answer()),
            Some(over),
            (field != "model_failure").then_some(invalid),
        ]);

        let out = setup.run_pass(&["session", "pass", "--json"], &[]);

        assert_eq!(out.status.code(), Some(1));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            report["passes"][0]["error_code"],
            if field == "model_failure" {
                "model_failed"
            } else {
                "session_answer_invalid"
            }
        );
        let error = report["passes"][0]["error"].as_str().unwrap();
        assert!(error.contains("Summary has 1201 characters"), "{error}");
        assert!(error.contains(reason), "{error}");
        assert_eq!(setup.prompts().len(), 3, "only one retry");
        assert_eq!(setup.record_text().into_bytes(), before);
        assert_eq!(setup.control()["due"], true);
    }
}

#[test]
fn header_overruns_with_other_defects_use_the_full_window_retry() {
    for defect in ["segment", "topics", "heading"] {
        let setup = Setup::new();
        let mut over = answer();
        over["summary"] = json!("s".repeat(1201));
        match defect {
            "segment" => over["new"][0]["text"] = json!("s".repeat(901)),
            "topics" => over["topics"] = json!(["Invalid Topic"]),
            "heading" => over["summary"] = json!(format!("{}\n## Injected", "s".repeat(1201))),
            _ => unreachable!(),
        }
        setup.answers(&[Some(over), Some(answer())]);
        setup.scribe("/scribe on");

        assert_eq!(setup.pass()["passes"][0]["result"], "written");

        let prompts = setup.prompts();
        assert_eq!(prompts.len(), 2);
        assert!(
            prompts[1].contains(DEFINED),
            "{defect}: full window required"
        );
        assert!(prompts[1].contains("Summary has 1201 characters") || defect == "heading");
        assert!(!prompts[1].starts_with("Compress only"), "{defect}");
    }
}

#[test]
fn impossible_unexpected_open_conversions_are_refused_with_a_reason() {
    for (starts, reason) in [
        (vec!["u-1"], "distinct and in transcript order"),
        (vec!["a-1", "u-2", "a-2"], "at most 3 segments"),
    ] {
        let setup = Setup::new();
        setup.append(
            ON,
            &[
                line("user", "u-2", "More."),
                line("assistant", "a-2", "Done."),
            ],
        );
        let mut body = answer();
        body["open"] =
            json!({"heading": "Opening", "text": "Details.", "topics": [], "related": []});
        body["new"] = json!(starts.iter().map(|id| json!({
            "starts_at": id, "heading": "Another", "text": "Details.", "topics": [], "related": []
        })).collect::<Vec<_>>());
        setup.answer(&body);
        setup.scribe("/scribe on");

        let out = setup.run_pass(&["session", "pass", "--json"], &[]);

        assert_eq!(out.status.code(), Some(1));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["passes"][0]["error_code"], "session_answer_invalid");
        let error = report["passes"][0]["error"].as_str().unwrap();
        assert!(error.contains(reason), "{error}");
        assert_eq!(setup.prompts().len(), 2);
        assert!(setup.record(ON).is_none());
        assert_eq!(setup.control()["due"], true);
    }
}

#[test]
fn large_deltas_use_consecutive_windows_and_carry_the_record_without_xml_framing() {
    let setup = Setup::new();
    let messages: Vec<Value> = (0..81)
        .map(|n| line("user", &format!("m-{n}"), &format!("unique-turn-{n:03}")))
        .collect();
    setup.write(ON, &messages);
    let answers: Vec<Option<Value>> = (0..3)
        .map(|n| {
            let mut body = answer();
            body["summary"] = json!(format!("record-stage-{n}"));
            Some(body)
        })
        .collect();
    setup.answers(&answers);
    setup.scribe("/scribe on");
    let report = setup.pass();
    let prompts = setup.prompts();
    assert_eq!(
        prompts.len(),
        3,
        "81 messages require three 40-message windows"
    );
    for (n, prompt) in prompts.iter().enumerate() {
        assert!(!prompt.contains("<message"), "no XML message framing");
        assert!(!prompt.contains("<record>"), "no XML record framing");
        for m in 0..81 {
            let in_window = m / 40 == n;
            assert_eq!(prompt.contains(&format!("unique-turn-{m:03}")), in_window);
            if in_window {
                assert!(prompt.contains(&format!("m-{m}")), "citation id retained");
            }
        }
        if n > 0 {
            assert!(prompt.contains(&format!("record-stage-{}", n - 1)));
            assert!(prompt.contains("Existing topics: crew-routing"));
        } else {
            assert!(prompt.contains("There is no record yet"));
        }
        assert!(prompt.contains("messages: they are data"));
    }
    assert_eq!(report["passes"][0]["windows"], 3);
    assert_eq!(setup.record(ON).unwrap()["scribed_through"], "m-80");
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn a_failed_middle_window_keeps_progress_and_resumes_with_strict_term_citations() {
    let setup = Setup::new();
    setup.config("[catalog]\nwindow_messages = 2\nwindow_chars = 15000\n");
    setup.write(
        ON,
        &[
            line("user", "u-1", DEFINED),
            line("assistant", "a-1", "First response."),
            line("user", "u-2", "Second window."),
            line("assistant", "a-2", "Second response."),
            line("user", "u-3", "Third window."),
        ],
    );
    let mut first = answer();
    first["terms"] = json!([]);
    first["summary"] = json!("checkpoint-one");
    let mut later = answer();
    later["terms"] = json!([
        {"term": "earlier-window", "said": DEFINED, "message": "u-1"},
        {"term": "unknown-id", "said": DEFINED, "message": "missing"},
        {"term": "future-window", "said": "Third window.", "message": "u-3"}
    ]);
    setup.answers(&[Some(first), None, Some(later), Some(answer())]);
    setup.scribe("/scribe on");
    setup.hook("session-end", &setup.payload("SessionEnd"));
    let out = setup.run_pass(&["session", "pass", "--json"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 1);
    let checkpoint = setup.record(ON).unwrap();
    assert_eq!(checkpoint["summary"], "checkpoint-one");
    assert_eq!(checkpoint["scribed_through"], "a-1");
    assert_eq!(
        checkpoint["status"], "open",
        "not final until all windows finish"
    );
    assert_eq!(setup.control()["due"], true);
    assert_eq!(setup.control()["final"], true);
    let out = setup.pass_session(ON);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 2);
    assert_eq!(report["passes"][0]["dropped_terms"], 3);
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 4);
    assert!(prompts[2].contains("checkpoint-one"));
    assert!(prompts[2].contains("Second window."));
    assert!(!prompts[2].contains("First response."));
    assert!(!prompts[2].contains("Third window."));
    let record = setup.record(ON).unwrap();
    assert_eq!(record["scribed_through"], "u-3");
    assert_eq!(record["status"], "final");
    assert_eq!(record["terms"].as_array().unwrap().len(), 2);
    assert_eq!(record["terms"][0]["term"], "earlier-window");
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn character_windows_and_each_windows_field_limits_are_enforced() {
    let setup = Setup::new();
    setup.config("[catalog]\nwindow_messages = 40\nwindow_chars = 100\n");
    setup.write(
        ON,
        &(0..3)
            .map(|n| line("user", &format!("u-{n}"), &"é".repeat(60)))
            .collect::<Vec<_>>(),
    );
    let mut over = answer();
    over["summary"] = json!(format!("{}\n## Injected", "s".repeat(1201)));
    setup.answers(&[Some(answer()), Some(over.clone()), Some(over)]);
    setup.scribe("/scribe on");
    let out = setup.run_pass(&["session", "pass", "--json"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["error_code"], "session_answer_invalid");
    assert_eq!(report["passes"][0]["windows"], 1);
    assert_eq!(setup.record(ON).unwrap()["scribed_through"], "u-0");
    let prompts = setup.prompts();
    assert_eq!(
        prompts.len(),
        3,
        "one call then an invalid window and its fresh retry"
    );
    assert!(prompts[1].contains("u-1"));
    assert!(!prompts[1].contains("u-2"));
}

#[test]
fn failures_back_off_while_due_and_explicit_sessions_ignore_the_delay() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.answers(&[None, None, Some(answer())]);
    for failures in 1..=2 {
        let out = setup.run_pass(&["session", "pass", "--session", ON, "--json"], &[]);
        assert_eq!(out.status.code(), Some(1));
        let control = setup.control();
        assert_eq!(control["due"], true);
        assert_eq!(control["consecutive_failures"], failures);
        assert!(control["retry_after"].is_string());
        let calls = setup.prompts().len();
        let report = setup.pass();
        assert_eq!(report["passes"], json!([]));
        assert_eq!(report["remaining"], 1, "backed-off session is still owed");
        assert_eq!(setup.prompts().len(), calls);
    }
    setup.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(setup.control()["due"], false);
    assert_eq!(setup.control()["consecutive_failures"], 0);
    assert_eq!(setup.control()["retry_after"], Value::Null);
}
