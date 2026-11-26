#[test]
fn consistency_scan_closes_retired_tag_candidates_without_a_model_call() {
    let fx = Fixture::new();
    let path = fx.fake_program(
        "claude",
        "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\nexit 1\n",
    );
    let s = Setup {
        transcript: fx
            .home()
            .join(".claude/projects/-work-scribe")
            .join(format!("{SESSION}.jsonl")),
        cwd: fx.elsewhere("work/scribe"),
        fx,
        path,
    };
    let cite = |msg: &str| {
        json!({
            "session": SESSION,
            "message": msg,
            "at": "2026-11-15T00:00:00Z",
            "prompt_sha256": "0".repeat(64),
            "line": 1,
            "cwd": "/work/scribe"
        })
    };
    let mut initial = Vec::new();
    let mut n = 0usize;
    for (tag, count) in [
        ("decisions", 7),
        ("task", 7),
        ("finding", 2),
        ("preferences", 2),
        ("standard", 2),
    ] {
        for i in 1..=count {
            n += 1;
            let quote = format!("I always keep {tag} item {i} documented.");
            initial.push(json!({
                "id": format!("p-{n}"),
                "kind": "candidate",
                "older": format!("{tag}-{i}"),
                "newer": null,
                "candidate": {
                    "tag": tag,
                    "role": "user",
                    "quote": quote,
                    "evidence": null,
                    "cite": cite(&format!("seed-{n}")),
                    "project": "scribe",
                    "provider": "claude-code",
                    "host": "host",
                    "admitted": null
                },
                "reason": format!("Unmarked {tag} statement: {quote}"),
                "quotes": [],
                "state": "open",
                "opened_at": "2026-11-15T00:01:00Z",
                "settled_at": null,
                "settled_by": null,
                "author": {"kind": "scribe", "name": "session-pass"},
                "updated_by": null,
                "settled_author": null,
                "closed_reason": null
            }));
        }
    }
    s.write_proposals(&json!(initial));

    let report = s.scan();
    assert_eq!(s.calls(), 0, "retired-tag cleanup needs no model call");
    let entries = s.json(&["entry", "list", "--json"]);
    assert!(
        entries["entries"].as_array().unwrap().is_empty(),
        "no entry is written"
    );
    assert_eq!(report["closed"].as_array().unwrap().len(), 16, "{report}");
    for p in report["closed"].as_array().unwrap() {
        assert_eq!(p["state"], "rejected");
        assert_eq!(p["closed_reason"], "tag_retired");
        assert_eq!(
            p["settled_author"],
            json!({"kind": "scribe", "name": "tag-retired"})
        );
        assert!(p["settled_by"].is_null());
    }

    let audit = s.audit();
    assert_eq!(audit.len(), 16, "{audit:?}");
    assert!(
        audit
            .iter()
            .all(|l| l["command"] == "consistency scan" && l["status"] == "success")
    );
    let audit_targets: Vec<&str> = audit
        .iter()
        .map(|l| l["target"].as_str().unwrap())
        .collect();
    for i in 1..=16 {
        assert!(audit_targets.contains(&format!("p-{i}").as_str()));
    }

    // Running consistency scan again changes nothing more.
    let second = s.scan();
    assert_eq!(second["closed"], json!([]));
    assert_eq!(second["opened"], json!([]));
    assert_eq!(s.calls(), 0);
    assert_eq!(s.audit().len(), 16);

    // Open preferences and standard candidates remain untouched.
    let open = s.json(&["proposal", "list", "--state", "open", "--json"]);
    assert_eq!(open["total"], 4);
    let rejected_table = s.fx.check(
        Cli(&[
            "proposal", "list", "--state", "rejected", "--format", "table",
        ]),
        Text,
    );
    assert!(rejected_table.contains("CLOSED_REASON"), "{rejected_table}");
    assert_eq!(
        rejected_table.matches("tag_retired").count(),
        16,
        "{rejected_table}"
    );
}

#[test]
fn a_scan_that_fails_after_closing_candidates_rolls_back_and_audits_failure() {
    let fx = Fixture::new();
    let path = fx.fake_program(
        "claude",
        "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\nexit 1\n",
    );
    let s = Setup {
        transcript: fx
            .home()
            .join(".claude/projects/-work-scribe")
            .join(format!("{SESSION}.jsonl")),
        cwd: fx.elsewhere("work/scribe"),
        fx,
        path,
    };
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
                "session": SESSION,
                "message": "seed-1",
                "at": "2026-11-15T00:00:00Z",
                "prompt_sha256": "0".repeat(64),
                "line": 1,
                "cwd": "/work/scribe"
            },
            "project": "scribe",
            "provider": "claude-code",
            "host": "host",
            "admitted": null
        },
        "reason": "Unmarked decisions statement.",
        "quotes": [],
        "state": "open",
        "opened_at": "2026-11-15T00:01:00Z",
        "settled_at": null,
        "settled_by": null,
        "author": {"kind": "scribe", "name": "session-pass"},
        "updated_by": null,
        "settled_author": null,
        "closed_reason": null
    });
    s.write_proposals(&json!([proposal]));

    // The seeded files move into scribe.db first, then a missing sessions
    // table is invisible to the close (that reads proposals only) and fails
    // list_sessions afterwards, deterministically.
    break_sessions(&s);

    let out = s.run(&["consistency", "scan", "--json"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "store_database", "{err}");
    assert_eq!(s.calls(), 0, "the failure is before any model call");

    mend_sessions(&s);
    let stored = support::proposals_doc(&s.fx.root());
    assert_eq!(stored["proposals"][0]["state"], "open");
    assert!(stored["proposals"][0]["closed_reason"].is_null());

    let scan_lines: Vec<Value> = s
        .audit()
        .into_iter()
        .filter(|line| line["command"] == "consistency scan")
        .collect();
    let closures = scan_lines
        .iter()
        .filter(|line| line["status"] == "success")
        .count();
    assert_eq!(closures, 0, "{scan_lines:?}");
    let failures = scan_lines
        .iter()
        .filter(|line| line["status"] == "failure" && line["error_code"] == "store_database")
        .count();
    assert_eq!(failures, 1, "{scan_lines:?}");

    let report = s.scan();
    assert_eq!(ids(&report["closed"]), ["p-1"]);
    assert_eq!(s.proposal("p-1")["closed_reason"], "tag_retired");
    assert_eq!(
        s.audit()
            .into_iter()
            .filter(|line| line["command"] == "consistency scan" && line["status"] == "success")
            .count(),
        1
    );
}

#[test]
fn consistency_scan_with_custom_tag_config_keeps_open_preferences_candidates() {
    let fx = Fixture::new();
    let path = fx.fake_program(
        "claude",
        "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\nexit 1\n",
    );
    let s = Setup {
        transcript: fx
            .home()
            .join(".claude/projects/-work-scribe")
            .join(format!("{SESSION}.jsonl")),
        cwd: fx.elsewhere("work/scribe"),
        fx,
        path,
    };
    std::fs::create_dir_all(s.fx.root()).unwrap();
    make_private(&s.fx.root());
    let config = s.fx.root().join("config.toml");
    std::fs::write(&config, "[tags.remind]\naction = true\n").unwrap();
    make_private(&config);

    let candidate = |n: usize, tag: &str| {
        let quote = format!("I always keep {tag} item {n} documented.");
        json!({
            "id": format!("p-{n}"),
            "kind": "candidate",
            "older": format!("{tag}-{n}"),
            "newer": null,
            "candidate": {
                "tag": tag,
                "role": "user",
                "quote": quote,
                "evidence": null,
                "cite": {
                    "session": SESSION,
                    "message": format!("seed-{n}"),
                    "at": "2026-11-15T00:00:00Z",
                    "prompt_sha256": "0".repeat(64),
                    "line": 1,
                    "cwd": "/work/scribe"
                },
                "project": "scribe",
                "provider": "claude-code",
                "host": "host",
                "admitted": null
            },
            "reason": format!("Unmarked {tag} statement: {quote}"),
            "quotes": [],
            "state": "open",
            "opened_at": "2026-11-15T00:01:00Z",
            "settled_at": null,
            "settled_by": null,
            "author": {"kind": "scribe", "name": "session-pass"},
            "updated_by": null,
            "settled_author": null,
            "closed_reason": null
        })
    };
    s.write_proposals(&json!([
        candidate(1, "preferences"),
        candidate(2, "decisions")
    ]));

    let report = s.scan();
    assert_eq!(s.calls(), 0);
    let closed = report["closed"].as_array().unwrap();
    assert_eq!(closed.len(), 1, "{report}");
    assert_eq!(closed[0]["id"], "p-2");
    assert_eq!(closed[0]["closed_reason"], "tag_retired");

    let open = s.json(&["proposal", "list", "--state", "open", "--json"]);
    assert_eq!(open["total"], 1, "{open}");
    assert_eq!(open["proposals"][0]["id"], "p-1");
    assert_eq!(open["proposals"][0]["candidate"]["tag"], "preferences");
}

#[test]
fn a_scan_that_fails_after_reapply_and_open_rolls_back_every_write() {
    let fx = Fixture::new();
    let path = fx.fake_program(
        "claude",
        "#!/bin/sh\necho call >> \"$HOME/claude-calls\"\nexit 1\n",
    );
    let s = Setup {
        transcript: fx
            .home()
            .join(".claude/projects/-work-scribe")
            .join(format!("{SESSION}.jsonl")),
        cwd: fx.elsewhere("work/scribe"),
        fx,
        path,
    };
    // Active entries:
    // - preferences-1 would be superseded when p-1 is reapplied
    // - preferences-2 would cover approved candidate p-2 (closed as covered)
    // - rituals-1 is past review_after so open_proposals would open p-4
    s.entry(
        "preferences-1",
        "2026-01-02T03:04:05Z",
        OLDER,
        "active",
        None,
    );
    s.entry(
        "preferences-2",
        "2026-02-02T03:04:05Z",
        NEWER,
        "active",
        None,
    );
    s.entry(
        "rituals-1",
        "2026-01-05T03:04:05Z",
        RITUAL,
        "active",
        Some("2026-03-01"),
    );
    let cite = |msg: &str| {
        json!({
            "session": SESSION,
            "message": msg,
            "at": "2026-11-15T00:00:00Z",
            "prompt_sha256": "0".repeat(64),
            "line": 1,
            "cwd": "/work/scribe"
        })
    };
    let doc = json!({
        "proposals": [
            {
                "id": "p-1",
                "kind": "supersedes",
                "older": "preferences-1",
                "newer": "preferences-2",
                "candidate": null,
                "reason": "Newer replaces older.",
                "quotes": [],
                "state": "approved",
                "opened_at": "2026-11-15T00:00:00Z",
                "settled_at": "2026-11-15T00:05:00Z",
                "settled_by": cite("u-approve-1"),
                "author": {"kind": "scribe", "name": "consistency"},
                "updated_by": null,
                "settled_author": {"kind": "daniel", "name": "daniel"},
                "closed_reason": null
            },
            {
                "id": "p-2",
                "kind": "candidate",
                "older": "preferences-3",
                "newer": null,
                "candidate": {
                    "tag": "preferences",
                    "role": "user",
                    "quote": NEWER,
                    "evidence": null,
                    "cite": cite("u-cand-2"),
                    "project": "scribe",
                    "provider": "claude-code",
                    "host": "host",
                    "admitted": null
                },
                "reason": "Covered approved candidate.",
                "quotes": [],
                "state": "approved",
                "opened_at": "2026-11-15T00:01:00Z",
                "settled_at": "2026-11-15T00:06:00Z",
                "settled_by": cite("u-approve-2"),
                "author": {"kind": "scribe", "name": "session-pass"},
                "updated_by": null,
                "settled_author": {"kind": "daniel", "name": "daniel"},
                "closed_reason": null
            },
            {
                "id": "p-3",
                "kind": "candidate",
                "older": "decisions-1",
                "newer": null,
                "candidate": {
                    "tag": "decisions",
                    "role": "user",
                    "quote": "I always keep decisions item 1 documented.",
                    "evidence": null,
                    "cite": cite("u-cand-3"),
                    "project": "scribe",
                    "provider": "claude-code",
                    "host": "host",
                    "admitted": null
                },
                "reason": "Retired tag candidate.",
                "quotes": [],
                "state": "open",
                "opened_at": "2026-11-15T00:02:00Z",
                "settled_at": null,
                "settled_by": null,
                "author": {"kind": "scribe", "name": "session-pass"},
                "updated_by": null,
                "settled_author": null,
                "closed_reason": null
            }
        ]
    });
    s.write_proposals(&doc["proposals"]);

    // A missing segments table fails deactivate_sessions AFTER
    // reapply_approved, close_retired_candidates, and open_proposals have run
    // inside the scan's outer transaction.
    break_sessions(&s);

    let out = s.run(&["consistency", "scan", "--json"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "store_database", "{err}");
    assert_eq!(
        s.calls(),
        0,
        "stale-only open_proposals needs no model call"
    );
    mend_sessions(&s);

    assert_eq!(s.entry_json("preferences-1")["status"], "active");
    let stored = support::proposals_doc(&s.fx.root());
    let proposals = stored["proposals"].as_array().unwrap();
    assert_eq!(proposals.len(), 3, "{proposals:?}");
    assert_eq!(proposals[1]["state"], "approved");
    assert!(proposals[1]["closed_reason"].is_null());
    assert_eq!(proposals[2]["state"], "open");
    assert!(proposals[2]["closed_reason"].is_null());

    let scan_lines: Vec<Value> = s
        .audit()
        .into_iter()
        .filter(|line| line["command"] == "consistency scan")
        .collect();
    assert_eq!(
        scan_lines
            .iter()
            .filter(|line| line["status"] == "success")
            .count(),
        0,
        "failed scan rolls back all writes and emits no success audit lines: {scan_lines:?}"
    );
    assert_eq!(
        scan_lines
            .iter()
            .filter(|line| line["status"] == "failure" && line["error_code"] == "store_database")
            .count(),
        1,
        "{scan_lines:?}"
    );

    let report = s.scan();
    assert_eq!(ids(&report["opened"]), ["p-4"]);
    assert_eq!(s.entry_json("preferences-1")["status"], "superseded");
    let scan_lines: Vec<Value> = s
        .audit()
        .into_iter()
        .filter(|line| line["command"] == "consistency scan")
        .collect();
    for expected in ["preferences-1", "p-2", "p-3", "p-4"] {
        assert_eq!(
            scan_lines
                .iter()
                .filter(|line| line["status"] == "success" && line["target"] == expected)
                .count(),
            1,
            "expected committed write {expected} to be audited once: {scan_lines:?}"
        );
    }
}

#[test]
fn a_routine_consistency_scan_commits_with_audit_and_rolls_back_on_failure() {
    let s = Setup::new();
    let root = s.fx.root();
    let config = json!({"root": root, "task_workspaces": []});

    break_sessions(&s);
    let reply = plugin(&s, "scribe.consistency-scan", &config);
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error"]["code"], "store_database", "{reply}");
    mend_sessions(&s);

    let list = s.json(&["proposal", "list", "--json"]);
    assert!(
        list["proposals"].as_array().unwrap().is_empty(),
        "failed routine scan rolled back opened proposal: {list}"
    );
    let scan_lines = s.audit();
    assert_eq!(scan_lines.len(), 1, "{scan_lines:?}");
    assert_eq!(scan_lines[0]["command"], "consistency scan");
    assert_eq!(scan_lines[0]["status"], "failure");
    assert_eq!(scan_lines[0]["error_code"], "store_database");

    let reply = plugin(&s, "scribe.consistency-scan", &config);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(ids(&reply["output"]["opened"]), ["p-1"]);
    assert_eq!(s.proposal("p-1")["state"], "open");
    let scan_lines = s.audit();
    assert_eq!(scan_lines.len(), 2, "{scan_lines:?}");
    assert_eq!(scan_lines[1]["command"], "consistency scan");
    assert_eq!(scan_lines[1]["status"], "success");
    assert_eq!(scan_lines[1]["target"], "p-1");
}
