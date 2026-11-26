#[test]
fn agent_write_shapes_match_goldens() {
    use scribe_core::{
        AgentEntry, Author, Label, Project, ProposalKind, Redactor, Store, StoreTag, Topics,
    };
    let fx = Fixture::new();
    let store = Store::open(fx.root());
    let now = time::macros::datetime!(2026-11-10 01:00 UTC);
    let author = || "opus".parse().unwrap();
    let redactor = Redactor::default();
    store
        .add_agent_tag(
            StoreTag {
                name: "notes".parse().unwrap(),
                aliases: vec!["note".parse().unwrap()],
                description: String::from("Agent notes"),
                stale_after_days: None,
                author: Author::agent(author()),
                updated_by: None,
                created: now,
                updated: now,
            },
            &redactor,
        )
        .unwrap();
    store
        .add_agent_entry(
            AgentEntry {
                tag: "notes".parse().unwrap(),
                content: String::from("Keep the API stable."),
                project: Project::sanitized("scribe"),
                topics: Topics::default(),
                related: Vec::new(),
                host: Label::sanitized("host", "host"),
                quote: None,
                evidence: None,
                as_of: None,
                handoff_mode: None,
                decision: None,
                item: None,
                self_refresh_verified: false,
                spoken: false,
            },
            author(),
            now,
            &redactor,
        )
        .unwrap();
    store
        .add_agent_proposal(
            ProposalKind::Stale,
            "notes-1",
            None,
            "Review this note.",
            author(),
            now,
            &redactor,
        )
        .unwrap();
    let mut failures = Vec::new();
    for (name, args) in [
        (
            "entry-agent-write",
            vec!["entry", "update", "notes-1", "--author", "opus"],
        ),
        (
            "tag-write",
            vec!["tag", "update", "notes", "--author", "opus"],
        ),
        (
            "proposal-write",
            vec![
                "proposal",
                "update",
                "p-1",
                "--author",
                "opus",
                "--reason",
                "Review this note.",
            ],
        ),
        ("tag-list-store", vec!["tag", "list"]),
    ] {
        for (suffix, mode) in MODES {
            let out = fx.check(&[args.as_slice(), *mode].concat(), Success);
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

#[test]
fn consistency_scan_closed_retired_tag_matches_goldens() {
    let proposal = json!({
        "id":"p-4", "kind":"candidate", "older":"decisions-1", "newer":null,
        "candidate": {
            "tag":"decisions", "quote":"I always keep the store format named.",
            "cite": {"session":"s-1", "message":"user-choice", "at":"2026-11-15T00:00:00Z",
                "prompt_sha256":"0".repeat(64), "line":1, "cwd":"/work/scribe"},
            "project":"scribe", "provider":"claude-code", "host":"host",
            "role":"user", "evidence":null, "admitted":null,
            "context":null, "statement":null, "subject":null
        },
        "reason":"Unmarked decisions statement: I always keep the store format named.",
        "quotes":[],
        "state":"open", "opened_at":"2026-11-15T00:01:00Z", "settled_at":null,
        "settled_by":null, "author":{"kind":"scribe", "name":"session-pass"},
        "settled_author":null, "updated_by":null, "closed_reason":null,
        "instruction":null
    });
    let mut failures = Vec::new();
    for (suffix, mode) in [
        ("plain.txt", &[][..]),
        ("table.txt", &["--format", "table"][..]),
    ] {
        let fx = Fixture::new();
        let body = json!({"format":31, "proposals":[proposal]}).to_string();
        support::seed_records(&fx.root(), &[&body]);
        let out = fx.check(&[&["consistency", "scan"][..], mode].concat(), Success);
        failures.extend(compare_golden(
            &format!("output/consistency-scan-closed.{suffix}"),
            &text(&out.stdout),
        ));
    }
    report(&failures);
}

#[test]
fn candidate_proposal_list_matches_goldens() {
    let fx = Fixture::seeded();
    let proposal = json!({
        "id":"p-3", "kind":"candidate", "older":"preferences-1", "newer":null,
        "candidate": {
            "tag":"preferences", "quote":"I prefer concise progress updates.",
            "cite": {"session":"s-1", "message":"user-choice", "at":"2026-11-15T00:00:00Z",
                "prompt_sha256":"0".repeat(64), "line":1, "cwd":"/work/scribe"},
            "project":"scribe", "provider":"claude-code", "host":"host",
            "role":"user", "evidence":null,
            "admitted": {"trigger":"I prefer", "matched":"I prefer",
                "criteria":"A standing rule from the user that applies beyond the work at hand."},
            "context":null, "statement":null, "subject":null
        },
        "reason":"Unmarked preference: I prefer concise progress updates.", "quotes":[],
        "state":"open", "opened_at":"2026-11-15T00:01:00Z", "settled_at":null,
        "settled_by":null, "author":{"kind":"scribe", "name":"session-pass"},
        "settled_author":null, "updated_by":null, "closed_reason":null,
        "instruction":null
    });
    // Seeding adds claims: drop the fixture's two so only `p-3` is listed.
    support::sql(&fx.root(), "DELETE FROM claims;");
    let body = json!({"format":31, "proposals":[proposal]}).to_string();
    support::seed_records(&fx.root(), &[&body]);
    let mut failures = Vec::new();
    for (suffix, mode) in MODES {
        let out = fx.check(&[&["proposal", "list"][..], *mode].concat(), Success);
        failures.extend(compare_golden(
            &format!("output/proposal-list-candidate.{suffix}"),
            &text(&out.stdout),
        ));
    }
    report(&failures);
}

/// `proposal show` of a claim with context (`p-7`) and of an older claim
/// (`p-6`, its statement in the context's place): each open claim can be
/// settled from it without opening its session (2_design.md §15.3).
#[test]
fn proposal_show_matches_goldens() {
    let fx = context_fixture();
    let cases: &[(&str, &str)] = &[
        ("proposal-show-context", "p-7"),
        ("proposal-show-older", "p-6"),
    ];
    let mut failures = Vec::new();
    for (name, id) in cases {
        for (suffix, mode) in MODES {
            let out = fx.check(&[&["proposal", "show", id][..], *mode].concat(), Success);
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    let out = fx.check(&["proposal", "list", "--state", "open"], Success);
    failures.extend(compare_golden(
        "output/proposal-list-context.plain.txt",
        &text(&out.stdout),
    ));
    report(&failures);
}

/// `scribe brief` with an open pass claim (the stated fixture's `p-6`, under
/// its tag, with its statement, subject and quote), and on an empty store.
#[test]
fn brief_matches_goldens() {
    let stated = stated_fixture();
    let empty = Fixture::new();
    let cases: &[(&str, &Fixture)] = &[("brief-claims", &stated), ("brief-empty", &empty)];
    let mut failures = Vec::new();
    for (name, fx) in cases {
        for (suffix, mode) in MODES {
            let args = [
                &["brief", "--project", "constellation-works/scribe"][..],
                *mode,
            ]
            .concat();
            let out = fx.check(&args, Success);
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

#[cfg(unix)]
#[test]
fn stated_claims_match_goldens() {
    let fx = stated_fixture();
    let commands: &[(&str, &[&str])] = &[
        ("entry-show-stated", &["entry", "show", "preferences-2"]),
        ("proposal-list-stated", &["proposal", "list"]),
        ("search-stated", &["search", "append-only log history"]),
    ];
    let mut failures = Vec::new();
    for (name, args) in commands {
        for (suffix, mode) in MODES {
            let out = run_with_input(&fx, &[*args, *mode].concat(), b"", None);
            assert_eq!(out.status.code(), Some(0), "{name}: {}", text(&out.stderr));
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

#[test]
fn instructed_approvals_match_goldens() {
    let fx = instructed_fixture();
    let commands: &[(&str, &[&str])] = &[
        ("entry-show-instructed", &["entry", "show", "preferences-2"]),
        ("proposal-list-instructed", &["proposal", "list"]),
        ("search-instructed", &["search", "append-only log history"]),
    ];
    let mut failures = Vec::new();
    for (name, args) in commands {
        for (suffix, mode) in MODES {
            let out = fx.check(&[*args, *mode].concat(), Success);
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

#[test]
fn only_an_instructed_approval_carries_the_approved_by_line() {
    let line = format!(
        "approved by opus on your instruction \"{INSTRUCTED_WORDS}\" ({INSTRUCTED_SESSION}, 2026-11-15)"
    );
    for (fx, instructed) in [(stated_fixture(), false), (instructed_fixture(), true)] {
        for mode in ["--format", "plain"] {
            let args: &[&str] = if mode == "plain" {
                &["entry", "show", "preferences-2"]
            } else {
                &["entry", "show", "preferences-2", "--format", "table"]
            };
            let out = fx.check(args, Text);
            assert_eq!(out.contains(&line), instructed, "{out}");
            assert_eq!(out.contains("approved by"), instructed, "{out}");
        }
        let search = fx.check(&["search", "append-only log history"], Text);
        assert_eq!(search.contains(&line), instructed, "{search}");
        let proposals = fx.check(&["proposal", "list", "--state", "approved"], Text);
        assert_eq!(proposals.contains(&line), instructed, "{proposals}");
        let rejected = fx.check(&["proposal", "list", "--state", "rejected"], Text);
        assert_eq!(
            rejected.contains(&format!(
                "rejected by opus on your instruction \"{REJECTED_WORDS}\""
            )),
            instructed,
            "{rejected}"
        );
    }
}

#[cfg(unix)]
#[test]
fn captured_ideas_match_goldens() {
    let fx = captured_fixture();
    let commands: &[(&str, &[&str])] = &[
        ("entry-show-captured", &["entry", "show", "idea-1"]),
        ("entry-show-typed-idea", &["entry", "show", "idea-2"]),
        ("search-captured", &["search", "routing table per host"]),
    ];
    let mut failures = Vec::new();
    for (name, args) in commands {
        for (suffix, mode) in MODES {
            let out = run_with_input(&fx, &[*args, *mode].concat(), b"", None);
            assert_eq!(out.status.code(), Some(0), "{name}: {}", text(&out.stderr));
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &without_age(&text(&out.stdout)),
            ));
        }
    }
    report(&failures);
}

/// The memory block, each file shown, the fixed rules, and the writes on
/// them, against the memory fixture (`tests/fixtures/memory`): a profile,
/// two `now` items and two milestones, all with fixed dates, so the output
/// is the same every day.
#[test]
fn memory_matches_goldens() {
    let fx = Fixture::seeded();
    support::copy_private(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memory"),
        &fx.root().join("memory"),
    );
    let session = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
    let mut failures = Vec::new();
    // The fixture config redacts GITHUB_TOKEN's value, so a write needs it set.
    let ok = |args: &[&str]| {
        let out = fx.check(
            with_env(args, "GITHUB_TOKEN", "synthetic-fixture-token-value"),
            Capture,
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        out
    };
    let mut capture = |name: &str, args: &[&str]| {
        for (suffix, mode) in MODES {
            let out = ok(&[args, *mode].concat());
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    };
    capture("memory-show", &["memory", "show"]);
    capture("memory-show-profile", &["memory", "show", "profile"]);
    capture(
        "memory-show-item",
        &["memory", "show", "2026-12-01-hackathon-deadline"],
    );
    capture(
        "memory-show-milestone",
        &["memory", "show", "2026-06-14-married"],
    );
    capture("memory-rules", &["memory", "rules"]);
    // The fixture's item was raised in this session already, so the ack
    // changes nothing and prints the same every day.
    capture(
        "memory-ack",
        &[
            "memory",
            "ack",
            "2026-12-12-sisters-wedding",
            "--session",
            session,
            "--author",
            "opus",
        ],
    );
    // Forget removes the file, so each mode forgets a fresh copy of it.
    let stone = "milestones/2026-10-02-pulsar-decommissioned.md";
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memory");
    for (suffix, mode) in MODES {
        let target = fx.root().join("memory").join(stone);
        std::fs::copy(fixture.join(stone), &target).unwrap();
        support::make_private(&target);
        let out = ok(&[
            &[
                "memory",
                "forget",
                "2026-10-02-pulsar-decommissioned",
                "--author",
                "opus",
                "--confirm",
            ][..],
            *mode,
        ]
        .concat());
        failures.extend(compare_golden(
            &format!("output/memory-forget.{suffix}"),
            &text(&out.stdout),
        ));
    }
    report(&failures);
}

#[test]
fn every_session_reads_the_same_after_the_move_into_scribe_db() {
    const NEW: &str = "9a8b7c6d-0000-4000-8000-000000000001";
    let fx = Fixture::seeded();
    let before = every_session_shown(&fx);
    let pretty = format!("{}\n", serde_json::to_string_pretty(&before).unwrap());
    let mut failures: Vec<String> =
        compare_golden("output/sessions-before-and-after-the-move.json", &pretty)
            .into_iter()
            .collect();

    // The first session write moves the files: here a switch for a session
    // the fixture never had.
    let store = scribe_core::Store::open(fx.root());
    let new: scribe_core::SessionId = NEW.parse().unwrap();
    store
        .update_control(|state| {
            scribe_core::control::switch(
                state,
                &new,
                &scribe_core::control::Switch::On,
                time::macros::datetime!(2026-11-21 00:00 UTC),
            );
        })
        .unwrap();
    assert!(
        !fx.root().join("sessions").exists(),
        "the files moved aside"
    );
    assert!(fx.root().join("scribe.db").exists());
    let after = every_session_shown(&fx);
    if after != before {
        failures.push(format!(
            "session show changed across the move\n--- before\n{pretty}\n--- after\n{}",
            serde_json::to_string_pretty(&after).unwrap()
        ));
    }

    // Old cites resolve; every record keeps its `s-<n>`, and the new
    // session numbers after them.
    for cite in [
        "session:7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02",
        "session:7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02#1",
        "7c2b9d14",
        "s-3",
    ] {
        let shown: Value = fx.check(&["session", "show", cite, "--json"], Json);
        assert_eq!(
            shown["id"], "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02",
            "{cite}"
        );
        assert_eq!(shown["short_id"], "s-3", "{cite}");
    }
    let numbers = store.session_numbers().unwrap();
    assert_eq!(numbers.len(), 4, "{numbers:?}");
    assert_eq!(
        numbers[&new].get(),
        4,
        "a new session numbers after the moved ones"
    );
    report(&failures);
}

#[test]
fn typed_records_match_goldens() {
    use scribe_core::{
        AgentEntry, AnswerIn, Decision, ItemAnswer, Label, OpenItem, Project, Redactor, Store,
        Topics,
    };

    let fx = Fixture::new();
    let store = Store::open(fx.root());
    let now = time::macros::datetime!(2026-11-10 01:00 UTC);
    let author = || "opus".parse().unwrap();
    let draft = |tag: &str, content: &str| AgentEntry {
        tag: tag.parse().unwrap(),
        content: content.to_owned(),
        project: Project::sanitized("scribe"),
        topics: Topics::default(),
        related: vec!["task:ORB-42".parse().unwrap()],
        host: Label::sanitized("host", "host"),
        quote: None,
        decision: None,
        item: None,
        evidence: None,
        as_of: None,
        handoff_mode: None,
        self_refresh_verified: false,
        spoken: false,
    };
    store
        .add_agent_entry(
            AgentEntry {
                item: Some(OpenItem {
                    asked_by: "orbit-0-capt".parse().unwrap(),
                    options: vec![
                        "Use the stable API".to_owned(),
                        "Use the experimental API".to_owned(),
                    ],
                    recommended: Some(1),
                    why: Some("Compatibility tests cover the stable API.".to_owned()),
                    blocked: "Publishing the release".to_owned(),
                    answer_in: AnswerIn::Any,
                    answer: None,
                    closed: None,
                }),
                ..draft("open-item", "Which API should the release use?")
            },
            author(),
            now,
            &Redactor::default(),
        )
        .unwrap();
    store
        .add_agent_entry(
            AgentEntry {
                decision: Some(Decision {
                    decided_by: "0-chief".parse().unwrap(),
                    rationale: Some(
                        "Keeping two interfaces would double the maintenance work.".to_owned(),
                    ),
                    reason_quote: None,
                    options: vec![
                        "Keep the stable API".to_owned(),
                        "Maintain both interfaces".to_owned(),
                    ],
                    recommended: Some(1),
                    supersedes: None,
                    settles: Some("open-item-1".parse().unwrap()),
                }),
                ..draft("decision", "Keep the stable API for this release series.")
            },
            author(),
            now,
            &Redactor::default(),
        )
        .unwrap();

    store
        .add_agent_entry(
            AgentEntry {
                item: Some(OpenItem {
                    asked_by: "0-chief".parse().unwrap(),
                    options: Vec::new(),
                    recommended: None,
                    why: None,
                    blocked: "nothing".to_owned(),
                    answer_in: AnswerIn::Hand,
                    answer: None,
                    closed: None,
                }),
                ..draft("open-item", "Restart the dashboard")
            },
            author(),
            now,
            &Redactor::default(),
        )
        .unwrap();

    let mut failures = Vec::new();
    for (name, id) in [
        ("entry-show-decision", "decision-1"),
        ("entry-show-open-item", "open-item-1"),
    ] {
        for (suffix, mode) in MODES {
            let out = fx.check(&[&["entry", "show", id][..], *mode].concat(), Success);
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }

    // §18.4: the user's answer, then the close naming decision-1. The
    // store's verbs are driven directly: the CLI's transcript check is
    // covered in tests/agent_writes/records.rs.
    let later = time::macros::datetime!(2026-11-10 02:00 UTC);
    store
        .answer_open_item(
            "open-item-1",
            ItemAnswer {
                words: "1".to_owned(),
                session: "8dc9df2b-5808-47cf-8be4-190c9c6e0711".parse().unwrap(),
                at: later,
                recorded_by: "0-aide".parse().unwrap(),
            },
            later,
            &Redactor::default(),
        )
        .unwrap();
    for (suffix, mode) in MODES {
        let out = fx.check(
            &[&["entry", "list", "--tag", "open-item"][..], *mode].concat(),
            Success,
        );
        failures.extend(compare_golden(
            &format!("output/entry-list-open-items.{suffix}"),
            &text(&out.stdout),
        ));
    }
    let decision = "decision-1".parse().unwrap();
    store
        .close_open_item(
            "open-item-1",
            Some(&decision),
            None,
            "orbit-0-capt".parse().unwrap(),
            later,
            &Redactor::default(),
        )
        .unwrap();
    for (suffix, mode) in MODES {
        let out = fx.check(
            &[&["entry", "show", "open-item-1"][..], *mode].concat(),
            Success,
        );
        failures.extend(compare_golden(
            &format!("output/entry-show-open-item-closed.{suffix}"),
            &text(&out.stdout),
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
