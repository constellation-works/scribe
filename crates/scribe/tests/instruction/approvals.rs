#[test]
fn an_instructed_approval_writes_daniels_reserved_entry_with_the_agent_as_approver() {
    let fx = store_with_candidates();
    let words = "approve p-1, that is how I want reviews";
    claude(
        &fx,
        Some("claude-desktop"),
        &[
            ("a-1", "assistant", "p-1 is open: approve it?", 2),
            ("u-1", "user", words, 1),
        ],
    );
    // Without an instruction a claim stays his to settle.
    let before = state(&fx);
    assert_eq!(
        run(
            &fx,
            &["proposal", "settle", "p-1", "--approve", "--author", "opus"]
        ),
        (Some(1), Some(String::from("user_only")))
    );
    assert_eq!(state(&fx), before);

    let out = fx.check(&settle("p-1", "--approve", words), Success);
    assert_eq!(text(&out.stderr).trim(), "", "it settled the claim");
    let p = show(&fx, "proposal", "p-1");
    assert_eq!(p["state"], "approved");
    assert_eq!(
        p["settled_author"],
        json!({"kind": "agent", "name": "opus"})
    );
    assert_eq!(p["settled_by"]["session"], SESSION);
    assert_eq!(p["settled_by"]["message"], "u-1");
    assert_eq!(p["instruction"]["session"], SESSION);
    assert_eq!(p["instruction"]["message"], "u-1");
    assert_eq!(p["instruction"]["quote"], words);
    assert_eq!(p["closed_reason"], Value::Null);

    // His entry, under the id the claim reserved: his cite and words.
    let e = show(&fx, "entry", "preferences-1");
    assert_eq!(e["author"], json!({"kind": "user", "name": "user"}));
    assert_eq!(e["content"], CLAIM_STATEMENT);
    assert_eq!(e["quote"], CLAIM_QUOTE);
    assert_eq!(e["cite"]["session"], "s-1");
    assert_eq!(e["cite"]["message"], "turn-p-1");
    assert_eq!(e["content_from"]["kind"], "message");
    assert_eq!(e["content_from"]["role"], "user");
    assert_eq!(e["approved_by"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(e["instruction"], p["instruction"]);
    let said = format!(
        "scribe proposal settle p-1 --approve --author opus --session {SESSION} --instruction '{words}'"
    );
    assert_eq!(e["said"], said.as_str());

    // The same command again changes nothing; another settlement is refused.
    let again = fx.check(&settle("p-1", "--approve", words), Success);
    assert!(
        text(&again.stderr).contains("nothing changed"),
        "{}",
        text(&again.stderr)
    );
    assert_eq!(
        run(&fx, &settle("p-1", "--reject", "reject p-1 after all")),
        (Some(1), Some(String::from("instruction_not_found")))
    );

    // One audit line per change or refusal: agent, target and the
    // instruction's ids, never its words.
    let lines = audit_lines(&fx, "proposal settle");
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0]["error_code"], "user_only");
    assert_eq!(lines[0]["instruction_session"], Value::Null);
    assert_eq!(lines[1]["status"], "success");
    assert_eq!(lines[2]["error_code"], "instruction_not_found");
    assert_eq!(lines[2]["instruction_message"], Value::Null);
    for line in &lines {
        assert_eq!(line["author"], "opus");
        assert_eq!(line["target"], "p-1");
    }
    assert_eq!(lines[1]["instruction_session"], SESSION);
    assert_eq!(lines[1]["instruction_message"], "u-1");
    assert_eq!(lines[2]["instruction_session"], SESSION);
    let audit = support::audit_text(&fx.root());
    assert!(!audit.contains("how I want reviews"), "{audit}");
}

#[test]
fn an_instructed_approval_of_an_assistant_claim_writes_one_agent_entry() {
    let fx = store_with_candidates();
    let words = "accept p-2, keep that one";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    fx.check(&settle("p-2", "--approve", words), Success);
    let e = show(&fx, "entry", "preferences-2");
    assert_eq!(e["author"], json!({"kind": "agent", "name": "assistant"}));
    assert_eq!(e["content"], ASSISTANT_QUOTE);
    assert_eq!(e["content_from"]["role"], "assistant");
    assert_eq!(e["approved_by"], json!({"kind": "agent", "name": "opus"}));
    assert!(
        e["said"]
            .as_str()
            .unwrap()
            .starts_with("scribe proposal settle p-2 --approve --author opus"),
        "{e}"
    );
    // It rereads from disk in every listing.
    let listed: Value =
        serde_json::from_slice(&fx.check(&["entry", "list", "--json"], Success).stdout).unwrap();
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["warnings"], json!([]), "{listed}");
}

#[test]
fn an_instructed_rejection_writes_no_entry_and_closes_the_claim_instructed() {
    let fx = store_with_candidates();
    let words = "reject p-1 and p-2, neither is a rule";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    for id in ["p-1", "p-2"] {
        fx.check(&settle(id, "--reject", words), Success);
        let p = show(&fx, "proposal", id);
        assert_eq!(p["state"], "rejected");
        assert_eq!(p["closed_reason"], "instructed");
        assert_eq!(p["settled_author"]["name"], "opus");
        assert_eq!(p["instruction"]["message"], "u-1");
        assert_eq!(p["instruction"]["quote"], words);
    }
    let entries: Value =
        serde_json::from_slice(&fx.check(&["entry", "list", "--json"], Success).stdout).unwrap();
    assert!(
        entries["entries"].as_array().unwrap().is_empty(),
        "no entry is written"
    );
    // An approval after it is refused: the rejection stands.
    let approve = "approve p-1 after all";
    claude(
        &fx,
        Some("cli"),
        &[("u-1", "user", words, 2), ("u-2", "user", approve, 1)],
    );
    assert_eq!(
        run(&fx, &settle("p-1", "--approve", approve)),
        (Some(1), Some(String::from("proposal_settled")))
    );
    let lines = audit_lines(&fx, "proposal settle");
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[2]["error_code"], "proposal_settled");
    assert_eq!(lines[2]["instruction_message"], "u-2");
}

#[test]
fn an_instruction_does_not_settle_a_proposal_about_daniels_entry() {
    let fx = Fixture::seeded();
    // The seeded `p-1` proposes that Daniel's `decisions-2` supersede his
    // `decisions-1`.
    let words = "approve p-1 please";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    let before = state(&fx);
    for act in ["--approve", "--reject"] {
        let words = if act == "--approve" {
            words
        } else {
            "reject p-1 please"
        };
        claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
        assert_eq!(
            run(&fx, &settle("p-1", act, words)),
            (Some(1), Some(String::from("user_only"))),
            "{act}"
        );
    }
    assert_eq!(state(&fx), before);
    let lines = audit_lines(&fx, "proposal settle");
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert_eq!(line["error_code"], "user_only");
        assert_eq!(line["target"], "p-1");
        assert_eq!(line["instruction_session"], SESSION);
        assert_eq!(line["instruction_message"], "u-1");
    }
}

#[test]
fn a_scan_replays_an_interrupted_instructed_approval_with_its_attribution() {
    let fx = store_with_candidates();
    let words = "approve p-1";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    fx.check(&settle("p-1", "--approve", words), Success);
    let entry = "entries/preferences/preferences-1.md";
    let written = support::exported(&fx.root(), entry).unwrap();
    // A crash between the claim and the entry leaves the approval.
    support::sql(&fx.root(), "DELETE FROM entries WHERE id = 'preferences-1'");
    let out = fx.check(&["consistency", "scan", "--json"], Success);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["reapplied"][0]["entry"], "preferences-1", "{report}");
    let e = show(&fx, "entry", "preferences-1");
    assert_eq!(e["approved_by"]["name"], "opus");
    assert_eq!(e["instruction"]["message"], "u-1");
    assert!(
        e["said"]
            .as_str()
            .unwrap()
            .starts_with("scribe proposal settle p-1 --approve"),
        "never a typed #scribe approve: {e}"
    );
    assert_eq!(
        support::exported(&fx.root(), entry).unwrap(),
        written,
        "the same entry"
    );
    // A typed repeat changes nothing either.
    let p = show(&fx, "proposal", "p-1");
    assert_eq!(p["state"], "approved");
}

#[test]
fn a_scan_closes_an_instructed_approval_covered_since_as_covered_not_rejected() {
    let fx = store_with_candidates();
    let words = "approve p-1";
    claude(&fx, Some("cli"), &[("u-1", "user", words, 1)]);
    fx.check(&settle("p-1", "--approve", words), Success);
    support::sql(&fx.root(), "DELETE FROM entries WHERE id = 'preferences-1'");
    // An active entry now says the same.
    let added: Value = serde_json::from_slice(
        &fx.check(
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--project",
                "scribe",
                "--author",
                "opus",
                CLAIM_STATEMENT,
                "--json",
            ],
            Success,
        )
        .stdout,
    )
    .unwrap();
    let out = fx.check(&["consistency", "scan", "--json"], Success);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["covered"][0]["proposal"], "p-1", "{report}");
    assert_eq!(report["covered"][0]["by"], added["id"], "{report}");
    // The store still reads, and the approval and instruction are kept.
    let p = show(&fx, "proposal", "p-1");
    assert_eq!(p["state"], "rejected");
    assert_eq!(p["closed_reason"], "covered");
    assert_eq!(p["settled_author"]["name"], "opus");
    assert_eq!(p["instruction"]["message"], "u-1");
    assert_eq!(p["instruction"]["quote"], words);
    assert!(support::exported(&fx.root(), "entries/preferences/preferences-1.md").is_none());
    // The same instruction again is done; nothing changes.
    let again = fx.check(&settle("p-1", "--approve", words), Success);
    assert!(
        text(&again.stderr).contains("nothing changed"),
        "{}",
        text(&again.stderr)
    );
}

#[test]
fn without_an_instruction_a_claim_is_refused_with_the_wording_it_always_had() {
    let fx = store_with_candidates();
    for act in ["--approve", "--reject"] {
        let (code, err) = refusal(&fx, &["proposal", "settle", "p-1", act, "--author", "opus"]);
        assert_eq!(code, Some(1), "{act}");
        assert_eq!(err["code"], "user_only", "{act}");
        assert_eq!(
            err["error"], "The user settles candidate proposals through #scribe approve or reject",
            "{act}"
        );
    }
}

#[test]
fn an_instruction_turn_without_a_message_id_is_refused_before_any_write() {
    for uuid in ["", "  "] {
        let fx = store_with_candidates();
        let words = "approve p-1, that is how I want reviews";
        claude(
            &fx,
            Some("claude-desktop"),
            &[
                ("a-1", "assistant", "p-1 is open: approve it?", 2),
                (uuid, "user", words, 1),
            ],
        );
        let before = state(&fx);
        assert_eq!(
            run(&fx, &settle("p-1", "--approve", words)),
            (Some(1), Some(String::from("instruction_not_found"))),
            "{uuid:?}"
        );
        assert_eq!(state(&fx), before, "nothing is written");
        // The store still reads.
        assert_eq!(show(&fx, "proposal", "p-1")["state"], "open");
        let lines = audit_lines(&fx, "proposal settle");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["instruction_message"], Value::Null);
    }
}

#[test]
fn an_instruction_typed_before_a_claim_opened_cannot_settle_it() {
    // The claims opened a minute ago; his instruction is three minutes old.
    let opened = (OffsetDateTime::now_utc() - time::Duration::minutes(1))
        .format(&Rfc3339)
        .unwrap();
    let fx = store_with_candidates_opened_at(&opened);
    let approve = "approve p-1, that is how I want reviews";
    let reject = "reject p-2, it is noise";
    claude(
        &fx,
        Some("cli"),
        &[("u-1", "user", approve, 3), ("u-2", "user", reject, 3)],
    );
    let before = state(&fx);
    assert_eq!(
        run(&fx, &settle("p-1", "--approve", approve)),
        (Some(1), Some(String::from("instruction_stale")))
    );
    assert_eq!(
        run(&fx, &settle("p-2", "--reject", reject)),
        (Some(1), Some(String::from("instruction_stale")))
    );
    assert_eq!(state(&fx), before, "neither claim changed");
    assert_eq!(show(&fx, "proposal", "p-1")["state"], "open");
    assert_eq!(show(&fx, "proposal", "p-2")["state"], "open");
    // Typed after it opened, the same words settle it.
    claude(&fx, Some("cli"), &[("u-3", "user", approve, 0)]);
    fx.check(&settle("p-1", "--approve", approve), Success);
    assert_eq!(show(&fx, "proposal", "p-1")["state"], "approved");
}

#[test]
fn a_refusal_after_the_instruction_turn_is_found_audits_its_message_id() {
    let fx = store_with_candidates();
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-1", "user", "approve p-1, that is how I want reviews", 50),
            ("u-2", "user", "approve p-9, a different claim", 2),
            ("u-3", "user", "anything else for now", 1),
        ],
    );
    // Found but stale, found but naming another claim, and never found.
    let cases = [
        (
            "approve p-1, that is how",
            "instruction_stale",
            json!("u-1"),
        ),
        (
            "approve p-9, a different",
            "instruction_mismatch",
            json!("u-2"),
        ),
        (
            "never typed by anyone",
            "instruction_not_found",
            Value::Null,
        ),
    ];
    for (words, code, _) in &cases {
        assert_eq!(
            run(&fx, &settle("p-1", "--approve", words)),
            (Some(1), Some(String::from(*code))),
            "{words}"
        );
    }
    let lines = audit_lines(&fx, "proposal settle");
    assert_eq!(lines.len(), cases.len(), "{lines:?}");
    for (line, (_, code, message)) in lines.iter().zip(&cases) {
        assert_eq!(line["error_code"], *code);
        assert_eq!(line["instruction_session"], SESSION);
        assert_eq!(line["instruction_message"], *message, "{code}");
    }
}

#[test]
fn retiring_an_instructed_approval_keeps_the_approval_apart_from_the_retirement() {
    let fx = store_with_candidates();
    let approve = "approve p-1, that is how I want reviews";
    claude(&fx, Some("cli"), &[("u-1", "user", approve, 2)]);
    fx.check(&settle("p-1", "--approve", approve), Success);
    let approved = show(&fx, "entry", "preferences-1");
    let retire_words = "please retire preferences-1 now";
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-1", "user", approve, 2),
            ("u-2", "user", retire_words, 1),
        ],
    );
    fx.check(&retire("preferences-1", retire_words), Success);
    let e = show(&fx, "entry", "preferences-1");
    assert_eq!(e["status"], "retired");
    // The approval: who, the command and the instruction it was run on.
    assert_eq!(e["approved_by"], approved["approved_by"]);
    assert_eq!(e["said"], approved["said"]);
    assert_eq!(e["instruction"], approved["instruction"]);
    assert_eq!(e["instruction"]["message"], "u-1");
    assert_eq!(e["instruction"]["quote"], approve);
    // The retirement, on its own.
    assert_eq!(e["removal_instruction"]["message"], "u-2");
    assert_eq!(e["removal_instruction"]["quote"], retire_words);
    // Forgetting it later blanks both quotes and keeps both ids apart.
    let forget_words = "forget preferences-1 for good";
    claude(
        &fx,
        Some("cli"),
        &[
            ("u-2", "user", retire_words, 2),
            ("u-3", "user", forget_words, 1),
        ],
    );
    fx.check(&forget("preferences-1", forget_words), Success);
    let e = show(&fx, "entry", "preferences-1");
    assert_eq!(e["status"], "forgotten");
    assert_eq!(e["instruction"]["message"], "u-1");
    assert_eq!(e["instruction"]["quote"], Value::Null);
    assert_eq!(e["removal_instruction"]["message"], "u-3");
    assert_eq!(e["removal_instruction"]["quote"], Value::Null);
}

#[test]
fn a_multiline_instruction_cites_the_line_its_quote_starts_on() {
    let fx = store_with_candidates();
    // The quote's first line also appears on line 1, but the quote as a
    // whole starts on line 3.
    let turn =
        "approve p-1, that is\nwhat I said earlier\napprove p-1, that is\nhow I want reviews";
    let words = "approve p-1, that is\nhow I want reviews";
    claude(&fx, Some("cli"), &[("u-1", "user", turn, 1)]);
    fx.check(&settle("p-1", "--approve", words), Success);
    let p = show(&fx, "proposal", "p-1");
    assert_eq!(p["settled_by"]["line"], 3, "{p}");
}
