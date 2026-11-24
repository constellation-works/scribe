//! Decision and open-item entry writes through the CLI.

use super::{Fixture, json, support, text};

const DECISION_SESSION: &str = "8dc9df2b-5808-47cf-8be4-190c9c6e0711";
const LATER_SESSION: &str = "1b7e5a3c-9f2d-4c8e-b6a1-2d3e4f5a6b7c";
const DOC_LINK: &str = "doc:scribe/docs/design/scribe/4_decisions.md#stable-api";

fn decision_transcript(fx: &Fixture, turns: &[(&str, &str)]) -> String {
    decision_transcript_in(fx, DECISION_SESSION, "2026-11-10T01:00:00Z", turns)
}

/// Writes one session's transcript with every turn at `at`; returns its reference.
fn decision_transcript_in(fx: &Fixture, session: &str, at: &str, turns: &[(&str, &str)]) -> String {
    let dir = fx.home().join(".claude/projects/-work-scribe");
    fx.create_private_dir_all(&dir);
    let path = dir.join(format!("{session}.jsonl"));
    let rows: String = turns
        .iter()
        .enumerate()
        .map(|(i, (role, words))| {
            format!(
                "{}\n",
                serde_json::json!({
                    "type": role, "uuid": format!("test-{i}"), "cwd": "/work/scribe",
                    "timestamp": at, "sessionId": session,
                    "entrypoint": "cli", "message": {"role": role, "content": words}
                })
            )
        })
        .collect();
    std::fs::write(&path, rows).unwrap();
    support::make_private(&path);
    format!("session:{session}")
}

fn add_record(fx: &Fixture, tag: &str, args: &[&str]) -> std::process::Output {
    fx.run(
        &[
            &["entry", "add", "--tag", tag, "--author", "opus", "--json"][..],
            args,
        ]
        .concat(),
    )
}

fn assert_record_refusal(out: std::process::Output, code: &str) {
    assert!(!out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert_eq!(json(&out.stderr)["code"], code, "{}", text(&out.stderr));
}

#[test]
fn user_decisions_check_both_quotes_and_allow_short_whole_turns() {
    let fx = Fixture::new();
    let session = decision_transcript(
        &fx,
        &[
            ("user", "yes"),
            ("user", "The compatibility guarantee is public."),
            ("user", "agreed, with changes"),
            ("user", "ok\n```text\nmore typed text\n```"),
            ("assistant", "The assistant would choose differently."),
            ("user", "why"),
        ],
    );
    let decide = |quote, reason| {
        add_record(
            &fx,
            "dec",
            &[
                "--decided-by",
                "user",
                "--related",
                &session,
                "--related",
                "task:ORB-42",
                "--related",
                DOC_LINK,
                "--quote",
                quote,
                "--reason-quote",
                reason,
                "Keep the stable API",
            ],
        )
    };

    for (quote, reason) in [
        ("agreed", "The compatibility guarantee"),
        ("ok", "The compatibility guarantee"),
        ("YES", "The compatibility guarantee"),
        (
            "The assistant would choose differently.",
            "The compatibility guarantee",
        ),
        ("yes", "compat"),
        ("yes", "An invented rationale"),
        ("yes", ""),
    ] {
        assert_record_refusal(decide(quote, reason), "quote_not_found");
    }
    let absent = add_record(
        &fx,
        "decision",
        &[
            "--decided-by",
            "user",
            "--related",
            "repo:scribe",
            "--related",
            "session:missing-transcript",
            "--quote",
            "yes",
            "A decision",
        ],
    );
    assert_record_refusal(absent, "quote_not_found");

    for reason in ["The compatibility guarantee", "why"] {
        let out = decide("yes", reason);
        assert!(out.status.success(), "{out:?}");
        let body = json(&out.stdout);
        let id = body["id"].as_str().unwrap();
        assert_eq!(body["author"]["kind"], "agent");
        assert_eq!(body["decision"]["decided_by"], "user");
        assert_eq!(body["quote"], "yes");
        assert_eq!(body["decision"]["reason_quote"], reason);
        assert_eq!(json(&fx.ok(&["entry", "show", id, "--json"]).stdout), body);
    }
    assert_eq!(
        json(
            &fx.ok(&["entry", "list", "--related", DOC_LINK, "--json"])
                .stdout
        )["total"],
        2
    );
}

#[test]
fn decision_creation_refuses_bad_fields_and_enforces_the_combined_size() {
    let fx = Fixture::new();
    let session = decision_transcript(&fx, &[("user", "yes")]);
    for (args, code) in [
        (&["A decision"][..], "decided_by_required"),
        (
            &["--decided-by", "0-chief", "A decision"],
            "rationale_required",
        ),
        (
            &["--decided-by", "0-chief", "--rationale", " ", "A decision"],
            "rationale_required",
        ),
        (
            &[
                "--decided-by",
                "0-chief",
                "--rationale",
                "Why",
                "A decision",
            ],
            "record_required",
        ),
        (&["--decided-by", "user", "A decision"], "quote_required"),
        (
            &["--decided-by", "user", "--quote", "yes", "A decision"],
            "quote_required",
        ),
        (
            &[
                "--decided-by",
                "user",
                "--rationale",
                "Why",
                "--quote",
                "yes",
                "--related",
                &session,
                "A decision",
            ],
            "rationale_not_allowed",
        ),
        (
            &[
                "--decided-by",
                "0-chief",
                "--quote",
                "yes",
                "--rationale",
                "Why",
                "A decision",
            ],
            "decision_quote_not_allowed",
        ),
        (
            &[
                "--decided-by",
                "0-chief",
                "--reason-quote",
                "yes",
                "--rationale",
                "Why",
                "A decision",
            ],
            "decision_quote_not_allowed",
        ),
        (
            &[
                "--decided-by",
                "0-chief",
                "--asked-by",
                "orbit-0-capt",
                "A decision",
            ],
            "record_fields",
        ),
        (
            &["--decided-by", "0-chief", "--spoken", "A decision"],
            "not_memory_tag",
        ),
        (&["--decided-by", "scribe", "A decision"], "reserved_author"),
        (&[""], "invalid_text"),
    ] {
        assert_record_refusal(add_record(&fx, "decision", args), code);
    }
    let decide = |extra: &[&str]| {
        let args = [
            &[
                "--decided-by",
                "0-chief",
                "--rationale",
                "Keep worker builds repeatable",
                "--related",
                "pr:example/scribe#1",
                "Keep the stable API",
            ][..],
            extra,
        ]
        .concat();
        add_record(&fx, "decision", &args)
    };
    for (args, code) in [
        (&["--option", "Only one"][..], "invalid_options"),
        (&["--option", "One", "--option", " "], "invalid_options"),
        (
            &[
                "--option", "A", "--option", "B", "--option", "C", "--option", "D", "--option",
                "E", "--option", "F", "--option", "G",
            ],
            "invalid_options",
        ),
        (&["--recommended", "1"], "invalid_recommendation"),
        (
            &["--option", "A", "--option", "B", "--recommended", "0"],
            "invalid_recommendation",
        ),
        (
            &["--option", "A", "--option", "B", "--recommended", "3"],
            "invalid_recommendation",
        ),
    ] {
        assert_record_refusal(decide(args), code);
    }
    assert_record_refusal(
        add_record(&fx, "preferences", &["--option", "A", "A preference"]),
        "record_fields",
    );
    let content = "é".repeat(2_993);
    let decision = |reason| {
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
                "yes",
                "--reason-quote",
                reason,
                "--option",
                "A",
                "--option",
                "B",
                &content,
            ],
        )
    };
    decision_transcript(&fx, &[("user", "yes"), ("user", "ok"), ("user", "why")]);
    assert_record_refusal(decision("why"), "decision_too_long");
    assert!(decision("ok").status.success());
    assert_record_refusal(
        fx.run(&[
            "entry",
            "update",
            "decision-1",
            "--author",
            "opus",
            "--json",
            "--content",
            &format!("{content}x"),
        ]),
        "decision_too_long",
    );
    assert_record_refusal(
        fx.run(&[
            "entry",
            "update",
            "decision-1",
            "--author",
            "opus",
            "--json",
            "--remove-related",
            &session,
        ]),
        "decision_session",
    );
    assert_record_refusal(
        fx.run(&[
            "entry",
            "update",
            "decision-1",
            "--author",
            "opus",
            "--json",
            "--remove-related",
            "repo:scribe",
        ]),
        "record_required",
    );
}

#[test]
fn decision_reversal_checks_links_and_commits_the_pair() {
    let fx = Fixture::new();
    let session = decision_transcript(&fx, &[("user", "1")]);
    let decide = |extra: &[&str]| {
        add_record(
            &fx,
            "decision",
            &[
                &[
                    "--decided-by",
                    "0-chief",
                    "--rationale",
                    "Preserve compatibility",
                    "--related",
                    "repo:scribe",
                    "Keep the stable API",
                ][..],
                extra,
            ]
            .concat(),
        )
    };
    let decide_as_user = |extra: &[&str]| {
        add_record(
            &fx,
            "decisions",
            &[
                &[
                    "--decided-by",
                    "user",
                    "--quote",
                    "1",
                    "--related",
                    &session,
                    "--related",
                    "task:ORB-42",
                    "Adopt the versioned API",
                ][..],
                extra,
            ]
            .concat(),
        )
    };
    assert!(decide(&[]).status.success());
    assert!(decide_as_user(&[]).status.success());
    assert!(
        add_record(
            &fx,
            "open-item",
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "API publication",
                "Which API ships?",
            ]
        )
        .status
        .success()
    );
    for id in ["decision-9", "open-item-1"] {
        assert_record_refusal(decide(&["--supersedes", id]), "invalid_supersedes");
    }
    for id in ["open-item-9", "decision-1"] {
        assert_record_refusal(decide(&["--settles", id]), "invalid_settles");
    }
    assert_record_refusal(
        decide(&["--supersedes", "decision-2"]),
        "supersede_user_decision",
    );

    // Fail the second SQL write deterministically, after the new row's insert.
    support::sql(
        &fx.root(),
        "CREATE TRIGGER refuse_reversal BEFORE UPDATE OF status ON entries
         WHEN OLD.id = 'decision-1' BEGIN SELECT RAISE(ABORT, 'test refusal'); END;",
    );
    let failed = decide(&["--supersedes", "decision-1", "--settles", "open-item-1"]);
    assert!(!failed.status.success(), "{failed:?}");
    assert_eq!(
        json(&fx.ok(&["entry", "show", "decision-1", "--json"]).stdout)["status"],
        "active"
    );
    assert_eq!(
        json(
            &fx.ok(&["entry", "list", "--tag", "decision", "--json"])
                .stdout
        )["total"],
        2
    );
    support::sql(&fx.root(), "DROP TRIGGER refuse_reversal");

    let new = decide(&["--supersedes", "decision-1", "--settles", "open-item-1"]);
    assert!(new.status.success(), "{new:?}");
    let new = json(&new.stdout);
    assert_eq!(new["id"], "decision-3");
    assert_eq!(new["decision"]["supersedes"], "decision-1");
    assert_eq!(new["decision"]["settles"], "open-item-1");
    assert_eq!(
        json(&fx.ok(&["entry", "show", "open-item-1", "--json"]).stdout)["item"]["answer"],
        serde_json::Value::Null
    );
    let old = json(&fx.ok(&["entry", "show", "decision-1", "--json"]).stdout);
    assert_eq!(old["status"], "superseded");
    assert_eq!(old["superseded_by"], "decision-3");
    assert_eq!(old["content"], "Keep the stable API");
    assert_record_refusal(
        decide(&["--supersedes", "decision-1"]),
        "invalid_supersedes",
    );
    assert!(
        decide_as_user(&["--supersedes", "decision-2"])
            .status
            .success()
    );
    let audit = support::audit_text(&fx.root());
    assert!(!audit.contains("Preserve compatibility"));
    assert!(!audit.contains("Keep the stable API"));
}

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
        fx.run(
            &[
                &["entry", "update", id, "--author", "sol", "--json"][..],
                args,
            ]
            .concat(),
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
    fx.run(
        &[
            &["entry", verb, id, "--author", "0-chief", "--json"][..],
            args,
        ]
        .concat(),
    )
}

fn states(fx: &Fixture, states: &[&str]) -> Vec<String> {
    let mut args = vec!["entry", "list", "--json"];
    for state in states {
        args.extend(["--state", state]);
    }
    json(&fx.ok(&args).stdout)["entries"]
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
    // The same words again change nothing; newer words replace them.
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
    fx.ok(&[
        "entry",
        "retire",
        "open-item-3",
        "--author",
        "0-chief",
        "--reason",
        "The question went away",
    ]);
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
    let bad = fx.run(&["entry", "list", "--state", "pending"]);
    assert_eq!(bad.status.code(), Some(2), "{bad:?}");
    // Other entries carry no state.
    assert_eq!(
        json(&fx.ok(&["entry", "show", "decision-1", "--json"]).stdout)["state"],
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

#[test]
fn delayed_older_answer_does_not_replace_a_newer_choice() {
    let fx = Fixture::new();
    let earlier = decision_transcript_in(
        &fx,
        DECISION_SESSION,
        "2026-11-10T01:00:00Z",
        &[("user", "Choose A")],
    );
    let later = decision_transcript_in(
        &fx,
        LATER_SESSION,
        "2026-11-11T02:00:00Z",
        &[("user", "Choose B")],
    );
    assert!(
        add_record(
            &fx,
            "open-item",
            &["--asked-by", "0-chief", "--blocked", "Build", "Choose"]
        )
        .status
        .success()
    );
    let answer = |words, session| {
        settle(
            &fx,
            "answer",
            "open-item-1",
            &["--quote", words, "--related", session],
        )
    };
    assert!(answer("Choose A", &earlier).status.success());
    let newer = answer("Choose B", &later);
    assert!(newer.status.success(), "{newer:?}");
    let expected = json(&newer.stdout)["item"]["answer"].clone();
    assert_eq!(expected["words"], "Choose B");
    assert_eq!(expected["at"], "2026-11-11T02:00:00Z");
    for (words, session) in [("Choose A", &earlier), ("Choose B", &later)] {
        let retry = answer(words, session);
        assert!(retry.status.success(), "{retry:?}");
        assert!(text(&retry.stderr).contains("nothing changed"), "{retry:?}");
        assert_eq!(json(&retry.stdout)["item"]["answer"], expected);
    }
}

#[test]
fn an_answer_typed_outside_the_named_session_is_recorded_with_a_notice() {
    let fx = Fixture::new();
    let session = decision_transcript(&fx, &[("user", "yes, restart it")]);
    let elsewhere = "session:5f0c3c1e-2b7a-4d7e-9a43-0c6f1d2e3b4a";
    let added = add_record(
        &fx,
        "open-item",
        &[
            "--asked-by",
            "orbit-0-capt",
            "--blocked",
            "nothing",
            "--answer-in",
            elsewhere,
            "Restart the dashboard",
        ],
    );
    assert!(added.status.success(), "{added:?}");
    let out = settle(
        &fx,
        "answer",
        "open-item-1",
        &["--quote", "yes, restart it", "--related", &session],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(
        text(&out.stderr).contains("must also type it in that session"),
        "{out:?}"
    );
    assert_eq!(json(&out.stdout)["state"], "answered");
}

#[test]
fn an_answer_takes_the_latest_matching_turn_across_named_sessions_in_either_order() {
    let fx = Fixture::new();
    let earlier = decision_transcript_in(
        &fx,
        DECISION_SESSION,
        "2026-11-10T01:00:00Z",
        &[("user", "yes, restart it")],
    );
    let later = decision_transcript_in(
        &fx,
        LATER_SESSION,
        "2026-11-11T02:00:00Z",
        &[("user", "yes, restart it")],
    );
    // The latest typed turn is in LATER_SESSION, so the answer belongs there
    // whichever session is named first; the notice fires only when the item
    // names another session.
    let mut item = 0;
    for (answer_in, notice) in [(&later, false), (&earlier, true)] {
        for order in [[&earlier, &later], [&later, &earlier]] {
            item += 1;
            let added = add_record(
                &fx,
                "open-item",
                &[
                    "--asked-by",
                    "orbit-0-capt",
                    "--blocked",
                    "nothing",
                    "--answer-in",
                    answer_in,
                    "Restart the dashboard",
                ],
            );
            assert!(added.status.success(), "{added:?}");
            let out = settle(
                &fx,
                "answer",
                &format!("open-item-{item}"),
                &[
                    "--quote",
                    "yes, restart it",
                    "--related",
                    order[0],
                    "--related",
                    order[1],
                ],
            );
            assert!(out.status.success(), "{out:?}");
            let body = json(&out.stdout);
            let answer = &body["item"]["answer"];
            assert_eq!(answer["session"], LATER_SESSION, "{order:?}");
            assert_eq!(answer["at"], "2026-11-11T02:00:00Z", "{order:?}");
            assert_eq!(
                text(&out.stderr).contains("must also type it in that session"),
                notice,
                "{order:?} {out:?}"
            );
        }
    }
}

#[test]
fn the_brief_overflow_list_reaches_an_answered_item_beyond_ten_open_ones() {
    // 2_design.md §18.5: the brief shows ten lines in all, and its overflow hint
    // names `--state open --state answered`, so that list must reach the rest.
    let fx = Fixture::new();
    let session = decision_transcript(&fx, &[("user", "1")]);
    for n in 1..=11 {
        let question = format!("Restart the dashboard {n}");
        assert!(
            add_record(
                &fx,
                "open-item",
                &[
                    "--asked-by",
                    "0-chief",
                    "--blocked",
                    "Local checks",
                    &question
                ],
            )
            .status
            .success()
        );
    }
    // open-item-11 is answered; the other ten stay open.
    let answered = settle(
        &fx,
        "answer",
        "open-item-11",
        &["--quote", "1", "--related", &session],
    );
    assert!(answered.status.success(), "{answered:?}");

    let open = states(&fx, &["open"]);
    assert_eq!(open.len(), 10, "{open:?}");
    assert!(!open.iter().any(|s| s.ends_with(" answered")), "{open:?}");

    let both = states(&fx, &["open", "answered"]);
    assert_eq!(both.len(), 11, "{both:?}");
    assert!(
        both.iter().any(|s| s == "open-item-11 answered"),
        "{both:?}"
    );
}
