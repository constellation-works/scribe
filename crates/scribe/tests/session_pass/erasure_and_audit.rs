#[test]
fn forget_clears_quote_window_retirement_reason_and_admitted_matched_through_cli() {
    let s = Setup::new();
    // Configure a wildcard trigger so `admitted.matched` carries words from
    // Daniel's turn that must be erased on `#scribe forget`.
    s.config("[tags.preferences.capture]\ntriggers = [\"always * checked\"]\n");
    let candidate_quote = "I always keep secret-matched-word checked before merging.";
    s.write(
        ON,
        &[
            line("user", "u-1", candidate_quote),
            line("user", "u-2", "secret-window-context from earlier turn"),
            line(
                "user",
                "u-3",
                "secret-quoted-correction from Daniel's typed turn",
            ),
        ],
    );
    s.answer(&candidate_answer(candidate_quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);
    type_settlement(&s, "#scribe approve p-1");

    // 1) Candidate-backed entry with `admitted.matched`.
    let before_prop = proposals(&s)["proposals"][0].clone();
    assert_eq!(
        before_prop["candidate"]["admitted"]["trigger"],
        "always * checked"
    );
    assert_eq!(
        before_prop["candidate"]["admitted"]["matched"],
        "always keep secret-matched-word checked"
    );

    type_settlement(&s, "#scribe forget preferences-1");

    let forgot_pref: Value = s.entry_json("preferences-1");
    assert_eq!(forgot_pref["status"], "forgotten");
    assert_eq!(forgot_pref["said"], "");
    assert_eq!(forgot_pref["content"], "");
    let after_prop = proposals(&s)["proposals"][0].clone();
    assert_eq!(after_prop["candidate"]["quote"], "");
    assert_eq!(after_prop["candidate"]["admitted"]["matched"], "");
    assert_eq!(
        after_prop["candidate"]["admitted"]["trigger"],
        "always * checked"
    );
    let prop_list_plain = s.fx.check(Cli(&["proposal", "list"]), Text);
    let prop_list_table =
        s.fx.check(Cli(&["proposal", "list", "--format", "table"]), Text);
    for rendered in [&prop_list_plain, &prop_list_table] {
        assert!(!rendered.contains("secret-matched-word"), "{rendered}");
    }

    // 2) Action entry with a transcript `window`, as a `#scribe standard`
    //    line kept one before markers took line-start forms over.
    let standard = support::entry_text(1, "secret-window-rule for CI")
        .replace("id: preferences-1\n", "id: standard-1\n")
        .replace("tag: preferences\n", "tag: standard\n")
        .replace("#scribe preferences:", "#scribe standard:")
        .replace(
            "\nwindow: null\n",
            "\nwindow: {turns: [{role: user, message: u-2, at: 2026-01-02T03:04:05Z, text: \"secret-window-context from earlier turn\", cut: false}, {role: user, message: null, at: 2026-01-02T03:04:05Z, text: \"#scribe standard: secret-window-rule for CI\", cut: false}], truncated: false, omitted: 0}\n",
        );
    support::seed_records(&s.fx.root(), &[&standard]);
    let before_win: Value = s.fx.check(
        Cli(&["entry", "show", "standard-1", "--window", "--json"]),
        Json,
    );
    assert!(
        before_win["window"].is_object(),
        "action capture keeps its window: {before_win}"
    );
    assert!(before_win["window_turns"].as_u64().unwrap_or(0) > 0);
    type_settlement(&s, "#scribe forget standard-1");
    let after_win: Value = s.fx.check(
        Cli(&["entry", "show", "standard-1", "--window", "--json"]),
        Json,
    );
    assert_eq!(after_win["status"], "forgotten");
    assert_eq!(after_win["said"], "");
    assert_eq!(after_win["content"], "");
    assert!(after_win["window"].is_null(), "{after_win}");
    assert!(after_win["window_turns"].is_null(), "{after_win}");

    // 3) Agent action entry with `quote` and `retirement_reason`.
    let session_rel = format!("session:{ON}");
    let added: Value = s.fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "standard",
            "--author",
            "opus",
            "--related",
            &session_rel,
            "--quote",
            "secret-quoted-correction from Daniel's typed turn",
            "secret-agent-rule for builds",
            "--json",
        ]),
        Json,
    );
    assert_eq!(added["id"], "standard-2");
    s.fx.check(
        Cli(&[
            "entry",
            "retire",
            "standard-2",
            "--author",
            "opus",
            "--reason",
            "secret-retire-reason explaining why",
            "--json",
        ]),
        Success,
    );
    let before_retired: Value = s.entry_json("standard-2");
    assert_eq!(
        before_retired["quote"],
        "secret-quoted-correction from Daniel's typed turn"
    );
    assert_eq!(
        before_retired["retirement_reason"],
        "secret-retire-reason explaining why"
    );
    type_settlement(&s, "#scribe forget standard-2");
    let after_retired: Value = s.entry_json("standard-2");
    assert_eq!(after_retired["status"], "forgotten");
    assert_eq!(after_retired["said"], "");
    assert_eq!(after_retired["content"], "");
    assert!(after_retired["quote"].is_null(), "{after_retired}");
    assert!(
        after_retired["retirement_reason"].is_null(),
        "{after_retired}"
    );

    // Every entry reads back cleanly with zero warnings, renders in plain/table
    // without error, and none of the forgotten words remain on disk.
    let listed: Value = s.fx.check(Cli(&["entry", "list", "--json"]), Json);
    assert_eq!(listed["warnings"], json!([]));
    assert_eq!(listed["total"], 3);
    for id in ["preferences-1", "standard-1", "standard-2"] {
        let plain = s.fx.check(Cli(&["entry", "show", id]), Text);
        let table =
            s.fx.check(Cli(&["entry", "show", id, "--format", "table"]), Text);
        for rendered in [&plain, &table] {
            for secret in [
                "secret-matched-word",
                "secret-window-context",
                "secret-window-rule",
                "secret-quoted-correction",
                "secret-agent-rule",
                "secret-retire-reason",
            ] {
                assert!(!rendered.contains(secret), "{id}: {rendered}");
            }
        }
    }
    // The export, the old files' shapes: the move into `pre-sqlite-*` keeps
    // the user's originals by design, so the store's own content is checked.
    let root = s.fx.root();
    let disk = support::exported(&root, "proposals.json").unwrap()
        + &support::exported(&root, "entries/preferences/preferences-1.md").unwrap()
        + &support::exported(&root, "entries/standard/standard-1.md").unwrap()
        + &support::exported(&root, "entries/standard/standard-2.md").unwrap();
    for secret in [
        "secret-matched-word",
        "secret-window-context",
        "secret-window-rule",
        "secret-quoted-correction",
        "secret-agent-rule",
        "secret-retire-reason",
    ] {
        assert!(
            !disk.contains(secret),
            "forgotten text {secret:?} must leave disk"
        );
    }
}

#[test]
fn pass_and_catalog_audit_persisted_closures_when_marking_failure_aborts() {
    let seed_retired_candidate = |s: &Setup, session: &str| {
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
                    "session": session,
                    "message": "seed-1",
                    "at": "2026-11-15T00:00:00Z",
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
            "opened_at": "2026-11-15T00:01:00Z",
            "settled_at": null,
            "settled_by": null,
            "author": {"kind": "scribe", "name": "session-pass"},
            "updated_by": null,
            "settled_author": null,
            "closed_reason": null
        });
        seed_claims(&s.fx.root(), &[proposal]);
    };

    // (a) `session pass`: window 1 persists the retired-tag closure of `p-1`;
    // window 2 replaces `control.json` with a symlink and fails so
    // `job.failed(failure)?` returns `Err(store_symlink)` and bypasses
    // `Output::Pass`.
    {
        let s = Setup::new();
        s.config("[catalog]\nwindow_messages = 1\n");
        s.write(
            ON,
            &[
                line("user", "u-1", "First window message."),
                line("user", "u-2", "Second window message."),
            ],
        );
        s.scribe("/scribe on");
        seed_retired_candidate(&s, ON);
        let control_path = s.fx.root().join("control.json");
        s.answers(&[Some(answer()), None]);
        let staged = control_path.clone();
        let preserved = s.fx.elsewhere("previous-control.json");
        let save = preserved.clone();
        // Finish exactly one window before introducing the refusal.
        let prepared = pass::cli_step(&s.fx, &[], "prepare", json!({"sessions": [ON]})).unwrap();
        let fixed = s
            .fixed_response(&prepared["window_request"], false)
            .unwrap();
        let applied = pass::cli_step(
            &s.fx,
            &[],
            "apply",
            json!({
                "basis": prepared["basis"], "cursor": prepared["cursor"], "answer": fixed
            }),
        )
        .unwrap();
        assert_eq!(applied["status"], "written");
        s.before_answer.replace(Some(Box::new(move || {
            if staged.exists() {
                std::fs::rename(&staged, save).unwrap();
            }
            std::os::unix::fs::symlink("/dev/null", staged).unwrap();
        })));

        let out = s.run_pass(&["session", "pass", "--session", ON, "--json"], &[]);
        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        let err: Value = serde_json::from_str(stderr.lines().last().unwrap()).expect(&stderr);
        assert_eq!(err["code"], "store_symlink", "{err}");

        // The export refuses the staged symlink; the symlink did its job.
        std::fs::remove_file(&control_path).unwrap();
        if preserved.exists() {
            std::fs::rename(preserved, &control_path).unwrap();
        }
        let stored = support::proposals_doc(&s.fx.root());
        assert_eq!(stored["proposals"][0]["state"], "rejected");
        assert_eq!(stored["proposals"][0]["closed_reason"], "tag_retired");

        let new_audit: Vec<Value> = support::audit_lines(&s.fx.root()).into_iter().collect();
        assert_eq!(
            new_audit
                .iter()
                .filter(|e| {
                    e["command"] == "session pass"
                        && e["status"] == "success"
                        && e["target"] == "p-1"
                })
                .count(),
            1,
            "persisted closure p-1 must be audited on early ? abort: {new_audit:?}"
        );
        assert_eq!(
            new_audit
                .iter()
                .filter(|e| {
                    e["command"] == "session pass"
                        && e["status"] == "failure"
                        && e["error_code"] == "store_symlink"
                })
                .count(),
            1,
            "{new_audit:?}"
        );
    }

    // (b) `session catalog`: same two-window failure where `job.failed` aborts.
    {
        let s = Setup::new();
        s.config("[catalog]\nwindow_messages = 1\n");
        s.write(
            NEVER,
            &[
                line("user", "u-1", "First catalog window."),
                line("user", "u-2", "Second catalog window."),
            ],
        );
        seed_retired_candidate(&s, NEVER);
        let control_path = s.fx.root().join("control.json");
        s.answers(&[Some(answer()), None]);
        let staged = control_path.clone();
        // The second window's activity fails after staging the symlink.
        s.before_answer_after.set(1);
        s.before_answer.replace(Some(Box::new(move || {
            // As `ln -sf` did: replace the exported control file.
            if staged.exists() {
                std::fs::remove_file(&staged).unwrap();
            }
            std::os::unix::fs::symlink("/dev/null", staged).unwrap();
        })));

        let audit_before = s.audit_lines();
        let out = s.run_catalog(&["session", "catalog", NEVER, "--author", "opus", "--json"]);
        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        let err: Value = serde_json::from_str(stderr.lines().last().unwrap()).expect(&stderr);
        assert_eq!(err["code"], "store_symlink", "{err}");

        // The export refuses the staged symlink; the symlink did its job.
        std::fs::remove_file(&control_path).unwrap();
        let stored = support::proposals_doc(&s.fx.root());
        assert_eq!(stored["proposals"][0]["state"], "rejected");
        assert_eq!(stored["proposals"][0]["closed_reason"], "tag_retired");

        let new_audit: Vec<Value> = support::audit_lines(&s.fx.root())
            .into_iter()
            .skip(audit_before)
            .collect();
        assert_eq!(
            new_audit
                .iter()
                .filter(|e| {
                    e["command"] == "session catalog"
                        && e["status"] == "success"
                        && e["target"] == "p-1"
                        && e["author"] == "opus"
                })
                .count(),
            1,
            "persisted closure p-1 must be audited on catalog early ? abort: {new_audit:?}"
        );
        assert_eq!(
            new_audit
                .iter()
                .filter(|e| {
                    e["command"] == "session catalog"
                        && e["status"] == "failure"
                        && e["error_code"] == "store_symlink"
                })
                .count(),
            1,
            "{new_audit:?}"
        );
    }
}

// --- statements (2_design.md §11.1) ------------------------------------------
