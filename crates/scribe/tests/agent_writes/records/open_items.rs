#[test]
fn open_items_check_the_question_and_recommendation_fields() {
    let fx = Fixture::new();
    let session = decision_transcript(&fx, &[("user", "yes")]);
    for (args, code) in [
        (&["A question"][..], "asked_by_required"),
        (&["--asked-by", "0-chief", "A question"], "invalid_text"),
        (
            &["--asked-by", "0-chief", "--blocked", " ", "A question"],
            "invalid_text",
        ),
        (
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "nothing",
                "--option",
                "A",
                "A question",
            ],
            "invalid_options",
        ),
        (
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "nothing",
                "--option",
                "A",
                "--option",
                "B",
                "A question",
            ],
            "invalid_recommendation",
        ),
        (
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "nothing",
                "--option",
                "A",
                "--option",
                "B",
                "--recommended",
                "3",
                "A question",
            ],
            "invalid_recommendation",
        ),
        (
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "nothing",
                "--option",
                "A",
                "--option",
                "B",
                "--recommended",
                "1",
                "A question",
            ],
            "recommendation_why",
        ),
        (
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "nothing",
                "--why",
                "No option",
                "A question",
            ],
            "recommendation_why",
        ),
        (
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "nothing",
                "--answer-in",
                "session:",
                "A question",
            ],
            "usage_error",
        ),
        (
            &["--asked-by", "user", "--blocked", "nothing", "A question"],
            "reserved_author",
        ),
        (
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "nothing",
                "--quote",
                "yes",
                "A question",
            ],
            "not_action_tag",
        ),
        (
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "nothing",
                "--rationale",
                "why",
                "A question",
            ],
            "record_fields",
        ),
    ] {
        assert_record_refusal(add_record(&fx, "open-item", args), code);
    }
    for (i, answer_in) in [None, Some("hand"), Some("session:8dc9df2b")]
        .into_iter()
        .enumerate()
    {
        let mut args = vec![
            "--asked-by",
            "0-chief",
            "--blocked",
            "nothing",
            "Restart the server",
        ];
        if let Some(location) = answer_in {
            args.extend(["--answer-in", location]);
        }
        let out = add_record(&fx, "item", &args);
        assert!(out.status.success(), "{out:?}");
        let item = &json(&out.stdout)["item"];
        assert_eq!(item["answer_in"], ["any", "hand", &session][i]);
        assert_eq!(item["options"], serde_json::json!([]));
        assert!(item["answer"].is_null());
        assert!(item["closed"].is_null());
    }
    let out = add_record(
        &fx,
        "open-items",
        &[
            "--asked-by",
            "orbit-0-capt",
            "--blocked",
            "Next release",
            "--option",
            "Use the cache",
            "--option",
            "Rebuild",
            "--recommended",
            "1",
            "--why",
            "Uses the tested path",
            "Choose the build",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let body = json(&out.stdout);
    assert_eq!(
        body["item"]["options"],
        serde_json::json!(["Use the cache", "Rebuild"])
    );
    assert_eq!(body["item"]["recommended"], 1);
    assert_eq!(body["item"]["why"], "Uses the tested path");
}

#[test]
fn checked_decision_quotes_are_refused_if_redaction_would_change_them() {
    let fx = Fixture::new();
    let secret = "ghp_abcdefghijklmnop";
    let session = decision_transcript(&fx, &[("user", "yes"), ("user", secret)]);
    for (quote, reason) in [("yes", secret), (secret, "yes")] {
        assert_record_refusal(
            add_record(
                &fx,
                "decision",
                &[
                    "--decided-by",
                    "user",
                    "--related",
                    &session,
                    "--related",
                    "repo:scribe",
                    "--quote",
                    quote,
                    "--reason-quote",
                    reason,
                    "A decision",
                ],
            ),
            "needs_redaction",
        );
    }
    let out = add_record(
        &fx,
        "decision",
        &[
            "--decided-by",
            "0-chief",
            "--related",
            "repo:scribe",
            "--rationale",
            secret,
            "--option",
            secret,
            "--option",
            "Another option",
            "A decision",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(!text(&out.stdout).contains(secret));
    assert!(!support::audit_text(&fx.root()).contains(secret));
}

#[test]
fn decision_corrections_keep_the_decider_and_check_the_revised_options_and_rationale() {
    let fx = Fixture::new();
    let add = add_record(
        &fx,
        "decision",
        &[
            "--decided-by",
            "0-chief",
            "--rationale",
            "Original reason",
            "--related",
            "repo:scribe",
            "An agent decision",
        ],
    );
    assert!(add.status.success(), "{add:?}");
    let update = |id, args: &[&str]| {
        fx.check(
            Cli(&[
                &["entry", "update", id, "--author", "sol", "--json"][..],
                args,
            ]
            .concat()),
            Capture,
        )
    };
    assert_record_refusal(
        update("decision-1", &["--option", "One"]),
        "invalid_options",
    );
    assert_record_refusal(
        update("decision-1", &["--rationale", " "]),
        "rationale_required",
    );
    assert_record_refusal(
        update("decision-1", &["--decided-by", "user"]),
        "usage_error",
    );

    let out = update(
        "decision-1",
        &[
            "--option",
            "One",
            "--option",
            "Two",
            "--recommended",
            "2",
            "--rationale",
            "Corrected reason",
            "--content",
            "Clarified choice",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let decision = json(&out.stdout);
    assert_eq!(decision["decision"]["decided_by"], "0-chief");
    assert_eq!(
        decision["decision"]["options"],
        serde_json::json!(["One", "Two"])
    );
    assert_eq!(decision["decision"]["recommended"], 2);
    assert_eq!(decision["decision"]["rationale"], "Corrected reason");
    assert_eq!(decision["updated_by"]["name"], "sol");
    assert_eq!(decision["content"], "Clarified choice");

    let out = update("decision-1", &["--clear-options"]);
    assert!(out.status.success(), "{out:?}");
    let decision = json(&out.stdout);
    assert_eq!(decision["decision"]["options"], serde_json::json!([]));
    assert!(decision["decision"]["recommended"].is_null());
    assert_record_refusal(
        update("decision-1", &["--recommended", "1"]),
        "invalid_recommendation",
    );

    let session = decision_transcript(&fx, &[("user", "yes")]);
    let add = add_record(
        &fx,
        "decision",
        &[
            "--decided-by",
            "user",
            "--related",
            &session,
            "--related",
            "repo:scribe",
            "--quote",
            "yes",
            "A user decision",
        ],
    );
    assert!(add.status.success(), "{add:?}");
    assert_record_refusal(
        update("decision-2", &["--rationale", "Invented reason"]),
        "rationale_not_allowed",
    );

    let item = add_record(
        &fx,
        "open-item",
        &[
            "--asked-by",
            "0-chief",
            "--blocked",
            "Build",
            "Restart the server",
        ],
    );
    assert!(item.status.success(), "{item:?}");
    assert_record_refusal(
        update("open-item-1", &["--rationale", "Why"]),
        "record_fields",
    );
}

fn settle(fx: &Fixture, verb: &str, id: &str, args: &[&str]) -> std::process::Output {
    fx.check(
        Cli(&[
            &["entry", verb, id, "--author", "0-chief", "--json"][..],
            args,
        ]
        .concat()),
        Capture,
    )
}

fn states(fx: &Fixture, states: &[&str]) -> Vec<String> {
    let mut args = vec!["entry", "list", "--json"];
    for state in states {
        args.extend(["--state", state]);
    }
    json(&fx.check(Cli(&args), Success).stdout)["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            format!(
                "{} {}",
                e["id"].as_str().unwrap(),
                e["state"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn open_items_are_answered_with_checked_words_then_closed_by_their_decision() {
    let fx = Fixture::new();
    let session = decision_transcript(
        &fx,
        &[
            ("user", "1"),
            ("user", "Go with the stable API for now."),
            ("assistant", "I recommend option 2."),
        ],
    );
    let choice = [
        "--asked-by",
        "orbit-0-capt",
        "--blocked",
        "Publishing the release",
        "--option",
        "Use the stable API",
        "--option",
        "Use the experimental API",
        "--recommended",
        "1",
        "--why",
        "Compatibility tests cover it.",
        "--related",
        "task:ORB-42",
        "Which API should the release use?",
    ];
    assert!(add_record(&fx, "open-item", &choice).status.success());
    assert_eq!(states(&fx, &["open"]), ["open-item-1 open"]);

    // The words must be the user's, typed in a named session.
    for (quote, related, code) in [
        ("I recommend option 2.", session.as_str(), "quote_not_found"),
        ("stable", session.as_str(), "quote_not_found"),
        ("2", session.as_str(), "quote_not_found"),
        ("1", "session:missing-transcript", "quote_not_found"),
        ("1", "task:ORB-42", "quote_required"),
    ] {
        assert_record_refusal(
            settle(
                &fx,
                "answer",
                "open-item-1",
                &["--quote", quote, "--related", related],
            ),
            code,
        );
    }
    // A choice is not closed before it is answered.
    assert_record_refusal(
        settle(&fx, "close", "open-item-1", &["--note", "Shipped"]),
        "not_answered",
    );

    let answered = settle(
        &fx,
        "answer",
        "open-item-1",
        &["--quote", "1", "--related", &session],
    );
    assert!(answered.status.success(), "{answered:?}");
    let body = json(&answered.stdout);
    assert_eq!(body["state"], "answered");
    assert_eq!(body["status"], "active");
    assert_eq!(body["item"]["answer"]["words"], "1");
    assert_eq!(body["item"]["answer"]["session"], DECISION_SESSION);
    assert_eq!(body["item"]["answer"]["at"], "2026-11-10T01:00:00Z");
    assert_eq!(body["item"]["answer"]["recorded_by"], "0-chief");
    // Replaying the same typed turn changes nothing; newer words replace them.
    let again = settle(
        &fx,
        "answer",
        "open-item-1",
        &["--quote", "1", "--related", &session],
    );
    assert!(again.status.success());
    assert!(text(&again.stderr).contains("nothing changed"), "{again:?}");
    let newer = settle(
        &fx,
        "answer",
        "open-item-1",
        &["--quote", "Go with the stable API", "--related", &session],
    );
    assert_eq!(
        json(&newer.stdout)["item"]["answer"]["words"],
        "Go with the stable API"
    );
    assert_eq!(states(&fx, &["answered"]), ["open-item-1 answered"]);
    assert!(states(&fx, &["open"]).is_empty());

    // --decision must name an active decision that settles this item.
    let decide = |settles: &str| {
        add_record(
            &fx,
            "decision",
            &[
                "--decided-by",
                "orbit-0-capt",
                "--rationale",
                "The user chose it.",
                "--related",
                "task:ORB-42",
                "--settles",
                settles,
                "Ship the stable API",
            ],
        )
    };
    assert!(
        add_record(
            &fx,
            "open-item",
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "nothing",
                "Restart the dashboard"
            ],
        )
        .status
        .success()
    );
    assert!(decide("open-item-2").status.success());
    for decision in ["decision-1", "decision-9", "open-item-2"] {
        assert_record_refusal(
            settle(&fx, "close", "open-item-1", &["--decision", decision]),
            "invalid_decision",
        );
    }
    assert!(decide("open-item-1").status.success());
    let closed = settle(&fx, "close", "open-item-1", &["--decision", "decision-2"]);
    assert!(closed.status.success(), "{closed:?}");
    let body = json(&closed.stdout);
    assert_eq!(body["state"], "closed");
    assert_eq!(body["status"], "retired");
    assert_eq!(body["item"]["closed"]["decision"], "decision-2");
    assert_eq!(body["item"]["closed"]["by"], "0-chief");
    assert_eq!(body["item"]["answer"]["words"], "Go with the stable API");

    // A closed item takes neither verb again.
    assert_record_refusal(
        settle(
            &fx,
            "answer",
            "open-item-1",
            &["--quote", "1", "--related", &session],
        ),
        "item_settled",
    );
    assert_record_refusal(
        settle(&fx, "close", "open-item-1", &["--note", "Again"]),
        "item_settled",
    );
    // Only open items are answered or closed.
    assert_record_refusal(
        settle(
            &fx,
            "answer",
            "decision-1",
            &["--quote", "1", "--related", &session],
        ),
        "not_open_item",
    );
    assert_record_refusal(
        settle(&fx, "close", "decision-1", &["--note", "Done"]),
        "not_open_item",
    );

    // An action item closes without an answer; a retired one is withdrawn.
    assert_record_refusal(
        settle(&fx, "close", "open-item-2", &["--note", " "]),
        "invalid_text",
    );
    let usage = settle(&fx, "close", "open-item-2", &[]);
    assert_eq!(usage.status.code(), Some(2), "{usage:?}");
    assert!(
        settle(
            &fx,
            "close",
            "open-item-2",
            &["--note", "Restarted; nothing lasting"]
        )
        .status
        .success()
    );
    assert!(
        add_record(
            &fx,
            "open-item",
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "nothing",
                "Rotate the key"
            ],
        )
        .status
        .success()
    );
    fx.check(
        &[
            "retire",
            "open-item-3",
            "--author",
            "0-chief",
            "--reason",
            "The question went away",
        ],
        Success,
    );
    assert_record_refusal(
        settle(&fx, "close", "open-item-3", &["--note", "Done"]),
        "item_settled",
    );
    assert_eq!(
        states(&fx, &["closed", "withdrawn"]),
        [
            "open-item-1 closed",
            "open-item-2 closed",
            "open-item-3 withdrawn"
        ]
    );
    let bad = fx.check(&["list", "--state", "pending"], Capture);
    assert_eq!(bad.status.code(), Some(2), "{bad:?}");
    // Other entries carry no state.
    assert_eq!(
        json(&fx.check(&["show", "decision-1", "--json"], Success).stdout)["state"],
        serde_json::Value::Null
    );

    // Audited by id and code, never by the words.
    let lines: Vec<serde_json::Value> = support::audit_lines(&fx.root())
        .into_iter()
        .filter(|l| l["command"] == "entry answer" || l["command"] == "entry close")
        .filter(|l| l["target"] == "open-item-1")
        .map(|l| serde_json::json!([l["command"], l["status"], l["error_code"]]))
        .collect();
    assert!(
        lines.contains(&serde_json::json!(["entry answer", "success", null]))
            && lines.contains(&serde_json::json!([
                "entry close",
                "failure",
                "not_answered"
            ]))
            && lines.contains(&serde_json::json!(["entry close", "success", null])),
        "{lines:?}"
    );
    let audit = support::audit_text(&fx.root());
    assert!(!audit.contains("Go with the stable API"));
    assert!(!audit.contains("Restarted; nothing lasting"));
}
