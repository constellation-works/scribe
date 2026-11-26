#[test]
fn forgetting_an_entry_deletes_the_stored_message_it_cites() {
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            line("user", "u-2", "Short explanations help reviewers too."),
        ],
    );
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);
    let messages = |terms: &str| -> Vec<String> {
        let found: Value =
            s.fx.check(Cli(&["search", terms, "--kind", "message", "--json"]), Json);
        let mut ids: Vec<String> = found["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["message"].as_str().unwrap().to_owned())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(messages("explanations"), ["u-1", "u-2"]);
    type_settlement(&s, "#scribe approve p-1");
    type_settlement(&s, "#scribe forget preferences-1");
    assert_eq!(messages("explanations"), ["u-2"]);
}

#[test]
fn forget_first_write_and_repeat_erase_legacy_session_archives() {
    let s = Setup::new();
    s.fx.seed();
    let root = s.fx.root();
    let shown: Value = s.fx.check(&["show", "s-1", "--json"], Json);
    let id = shown["id"].as_str().unwrap();
    let files = |base: &std::path::Path| -> Vec<PathBuf> {
        fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path, out);
                    } else {
                        out.push(path);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(base, &mut out);
        out
    };
    // Session records live in scribe.db: no record file exists to erase.
    // A residue an older build left in a pre-sqlite-* archive does.
    let bytes = b"residue of an older build's session record\n".to_vec();
    let out = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(out.contains("forgot s-1"), "{out}");
    let assert_absent = || {
        for path in files(&root) {
            let name = path.file_name().unwrap().to_string_lossy();
            assert!(
                name != format!("{id}.md") && name != format!(".{id}.rebuild.md"),
                "{}",
                path.display()
            );
        }
    };
    assert_absent();
    let other: Value = s.fx.check(&["show", "s-2", "--json"], Json);
    assert!(other["id"].is_string());
    // Model the residue left by an older build after a successful forget.
    let stale = root.join("pre-sqlite-stale/sessions/2026/11");
    s.fx.create_private_dir_all(&stale);
    for name in [format!("{id}.md"), format!(".{id}.rebuild.md")] {
        let path = stale.join(name);
        std::fs::write(&path, &bytes).unwrap();
        make_private(&path);
    }
    let again = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(again.contains("already forgotten"), "{again}");
    assert_absent();
}

#[test]
fn repeated_session_forget_audits_stale_checkpoint_cleanup_once() {
    let s = Setup::new();
    s.scribe("/scribe on");
    s.pass();
    type_settlement(&s, "#scribe forget s-1");
    let conn = rusqlite::Connection::open(s.fx.root().join("scribe.db")).unwrap();
    assert_eq!(conn.execute(
        "INSERT INTO rebuild_checkpoints (session, record, created) SELECT id, ?2, '2026-11-01T00:00:00Z' FROM sessions WHERE provider_session = ?1",
        (ON, "private stale checkpoint"),
    ).unwrap(), 1);
    drop(conn);
    let before = audit_log(&s).len();
    let reply = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(
        reply.contains("already forgotten") && reply.contains("checkpoint"),
        "{reply}"
    );
    assert_eq!(s.checkpoint_count(), 0);
    let audit = audit_log(&s);
    assert_eq!(audit.len(), before + 1, "{audit:?}");
    let line = &audit[before];
    assert_eq!(line["target"], "s-1");
    assert_eq!(line["status"], "success");
    assert!(!line.to_string().contains("private stale checkpoint"));
    let clean = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(
        clean.contains("already forgotten") && clean.contains("nothing changed"),
        "{clean}"
    );
    assert_eq!(audit_log(&s).len(), before + 1);
}

#[test]
fn forgetting_a_session_erases_its_words_keeps_its_id_and_takes_no_more_writes() {
    let s = Setup::new();
    let quote = "I prefer the lighthouse keeper plan for every Saturday release.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            line(
                "user",
                "u-2",
                "The lighthouse budget stays fixed this quarter.",
            ),
        ],
    );
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);
    let search = |terms: &str| -> Value {
        s.fx.check(Cli(&["search", terms, "--kind", "message", "--json"]), Json)
    };
    let show = |id: &str| -> Value { s.fx.check(&["show", id, "--json"], Json) };
    assert_eq!(search("lighthouse")["total"], 2);
    let before = show(ON);
    assert_eq!(before["short_id"], "s-1");
    assert_eq!(before["forgotten"], Value::Null);
    assert_eq!(before["on"], true);
    assert_ne!(before["log"][0]["text"], "");

    let audit_before = audit_log(&s).len();
    for prompt in [
        "<agent-message>\n#scribe forget s-1\n</agent-message>",
        "<scheduled-prompt>\n#scribe forget s-1\n</scheduled-prompt>",
        "<task-notification>\n#scribe forget s-1\n</task-notification>",
        "<pasted_content>\n#scribe forget s-1\n</pasted_content>",
        "```\n#scribe forget s-1\n```",
    ] {
        let skipped = type_settlement(&s, prompt);
        assert!(
            skipped.stdout.is_empty(),
            "{prompt}: {}",
            text(&skipped.stdout)
        );
        assert_eq!(show("s-1"), before);
    }
    assert!(
        !s.run_any(&["session", "forget", "s-1"], b"")
            .status
            .success()
    );
    assert_eq!(audit_log(&s).len(), audit_before);
    let transcript_before = std::fs::read(s.transcript(ON)).unwrap();
    // Typed while the session is on: it is switched off in the same write.
    let forgot = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(forgot.contains("scribe: forgot s-1"), "{forgot}");
    assert!(forgot.contains("switched off for good"), "{forgot}");
    assert!(
        forgot.contains("provider transcript was left untouched"),
        "{forgot}"
    );
    assert_eq!(std::fs::read(s.transcript(ON)).unwrap(), transcript_before);
    assert_eq!(search("lighthouse")["total"], 0, "{}", search("lighthouse"));
    let db = std::fs::read(s.fx.root().join("scribe.db")).unwrap();
    let wal = std::fs::read(s.fx.root().join("scribe.db-wal")).unwrap_or_default();
    for bytes in [&db, &wal] {
        let bytes = String::from_utf8_lossy(bytes);
        for words in ["lighthouse budget", "Level 4 is defined."] {
            assert!(!bytes.contains(words), "{words:?} is left in the database");
        }
    }
    let after = show("s-1");
    assert_eq!(after["id"], ON);
    assert_eq!(after["short_id"], "s-1");
    assert!(after["forgotten"].is_string(), "{after}");
    assert_eq!(after["on"], false);
    assert_eq!(after["summary"], "");
    assert_eq!(after["terms"], json!([]));
    assert_eq!(after["log"], json!([]));
    for field in ["title", "cwd", "summary", "goals", "next"] {
        assert_eq!(after[field], "", "{field}: {after}");
    }
    assert_eq!(after["topics"], json!([]));
    let listing: Value = s.fx.check(&["list", "--include-inactive", "--json"], Json);
    assert_eq!(listing["total"], 0);
    let pass = s.run_any(&["session", "pass", "--session", ON, "--json"], b"");
    assert!(!pass.status.success());
    assert!(text(&pass.stderr).contains("session_forgotten"));
    let table = s.fx.check(&["show", "s-1"], Text);
    assert!(table.contains("forgotten"), "{table}");
    let claim: Value =
        s.fx.check(Cli(&["proposal", "show", "p-1", "--json"]), Json);
    assert_eq!(claim["state"], "rejected", "{claim}");
    assert_eq!(claim["closed_reason"], "withdrawn");
    assert_eq!(claim["candidate"]["quote"], "");
    assert_eq!(claim["candidate"]["cite"]["session"], "deleted");
    let audit = audit_log(&s);
    let forgets: Vec<_> = audit[audit_before..]
        .iter()
        .filter(|line| line["command"] == "hook user-prompt-submit")
        .collect();
    assert_eq!(forgets.len(), 1, "{audit:?}");
    assert_eq!(forgets[0]["target"], "s-1");
    assert!(!forgets[0].to_string().contains("lighthouse"));
    let export = s.fx.home().join("jsonl-export");
    s.fx.check(
        Cli(&["export", "--as", "jsonl", export.to_str().unwrap()]),
        Success,
    );
    let rows = std::fs::read_to_string(export.join("sessions.jsonl")).unwrap();
    let row: Value = serde_json::from_str(rows.trim()).unwrap();
    assert_eq!(row["has_record"], 0);
    assert_eq!(row["has_switch"], 0);
    assert_eq!(row["title"], Value::Null);
    assert_eq!(row["cwd"], Value::Null);
    assert!(!export.join("segments.jsonl").exists());
    // Replayed: nothing changes.
    let again = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(again.contains("already forgotten"), "{again}");
    // It cannot be switched on again, and a pass writes nothing to it.
    let on = s.scribe("/scribe on");
    assert!(on.contains("forgotten"), "{on}");
    assert_eq!(show("s-1")["on"], false);

    // The provider-id form, from another session; a new session after it
    // never takes a forgotten number.
    const OTHER: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61";
    const THIRD: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e62";
    let switch_on = |session: &str| {
        s.write(
            session,
            &[line("user", "o-1", "The harbour crane needs a new cable.")],
        );
        let mut payload = s.payload("UserPromptSubmit");
        payload["session_id"] = json!(session);
        payload["transcript_path"] = json!(s.transcript(session));
        payload["prompt"] = json!("/scribe on");
        s.hook("user-prompt-submit", &payload);
    };
    switch_on(OTHER);
    s.answer(&answer());
    s.pass_session(OTHER);
    assert_eq!(show(OTHER)["short_id"], "s-2");
    assert_eq!(search("harbour")["total"], 1);
    let forgot = text(&type_settlement(&s, &format!("#scribe forget {OTHER}")).stdout);
    assert!(forgot.contains("scribe: forgot s-2"), "{forgot}");
    assert_eq!(search("harbour")["total"], 0);
    assert!(show(OTHER)["forgotten"].is_string());
    assert_eq!(audit_log(&s).pop().unwrap()["target"], "s-2");
    switch_on(THIRD);
    s.pass_session(THIRD);
    assert_eq!(show(THIRD)["short_id"], "s-3");
}

#[test]
fn typed_forget_takes_a_printed_session_prefix_and_refuses_an_ambiguous_one() {
    const SIBLING: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61";
    const OTHER: &str = "9e3a4f10-7b2c-4d58-a6f1-3c8d9e0b1a24";
    let s = Setup::new();
    let show = |id: &str| -> Value { s.fx.check(&["show", id, "--json"], Json) };
    let search = |word: &str| -> Value {
        s.fx.check(Cli(&["search", word, "--kind", "message", "--json"]), Json)
    };
    let switch_on = |session: &str, said: &str| {
        s.write(session, &[line("user", "o-1", said)]);
        let mut payload = s.payload("UserPromptSubmit");
        payload["session_id"] = json!(session);
        payload["transcript_path"] = json!(s.transcript(session));
        payload["prompt"] = json!("/scribe on");
        s.hook("user-prompt-submit", &payload);
        s.answer(&answer());
        s.pass_session(session);
    };
    s.write(ON, &[line("user", "u-1", "The quarry needs a new fence.")]);
    s.scribe("/scribe on");
    s.answer(&answer());
    s.pass_session(ON);
    switch_on(SIBLING, "The harbour crane needs a new cable.");
    switch_on(OTHER, "The lighthouse needs a new lamp.");
    assert_eq!(search("quarry")["total"], 1);
    assert_eq!(search("harbour")["total"], 1);
    assert_eq!(search("lighthouse")["total"], 1);

    // The first group is shared by ON and SIBLING: ambiguous, so nothing
    // changes and the refusal is audited with its code.
    let audited = audit_log(&s).len();
    let refused = text(&type_settlement(&s, "#scribe forget 5d1c0b7e").stdout);
    assert!(refused.contains("could not apply"), "{refused}");
    assert!(refused.contains("ambiguous"), "{refused}");
    assert_eq!(audit_log(&s).pop().unwrap()["error_code"], "id_ambiguous");
    assert_eq!(audit_log(&s).len(), audited + 1);
    assert_eq!(search("quarry")["total"], 1);
    assert_eq!(search("harbour")["total"], 1);
    assert!(show(ON)["forgotten"].is_null());
    assert!(show(SIBLING)["forgotten"].is_null());

    // A unique printed prefix erases the same session as the full id would,
    // and the echo names the full id.
    let forgot = text(&type_settlement(&s, "#scribe forget 9e3a4f10").stdout);
    assert!(
        forgot.contains(&format!("scribe: forgot s-3 ({OTHER})")),
        "{forgot}"
    );
    assert_eq!(search("lighthouse")["total"], 0);
    assert!(show(OTHER)["forgotten"].is_string());
    assert_eq!(search("quarry")["total"], 1);
    assert_eq!(search("harbour")["total"], 1);

    // A longer prefix both siblings still share is refused; the full id of one erases it.
    let forgot =
        text(&type_settlement(&s, "#scribe forget 5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e").stdout);
    assert!(forgot.contains("could not apply"), "{forgot}");
    let forgot =
        text(&type_settlement(&s, "#scribe forget 5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61").stdout);
    assert!(
        forgot.contains(&format!("forgot s-2 ({SIBLING})")),
        "{forgot}"
    );
    assert_eq!(search("harbour")["total"], 0);
    assert_eq!(search("quarry")["total"], 1);

    // A prefix that matches nothing erases nothing.
    let none = text(&type_settlement(&s, "#scribe forget abcdef12").stdout);
    assert!(none.contains("could not apply"), "{none}");
    assert_eq!(search("quarry")["total"], 1);
}

#[test]
fn session_forget_preserves_an_approved_entry_with_a_deleted_cite() {
    let s = Setup::new();
    let quote = "I always prefer the approved bluebird release process.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);
    let approved = type_settlement(&s, "#scribe approve p-1");
    assert!(text(&approved.stdout).contains("recorded preferences-1"));
    let show = || -> Value { s.entry_json("preferences-1") };
    let original = show();
    let forgot = type_settlement(&s, "#scribe forget s-1");
    assert!(text(&forgot.stdout).contains("forgot s-1"));
    let retained = show();
    assert_eq!(retained["content"], original["content"]);
    assert_eq!(retained["quote"], quote);
    assert_eq!(retained["status"], "active");
    assert_eq!(retained["cite"]["session"], "deleted");
    assert_eq!(retained["cite"]["message"], Value::Null);
    let after = type_settlement(&s, "#scribe approve p-1");
    assert!(text(&after.stdout).contains("already approved"));
    assert_eq!(show(), retained);
    type_settlement(&s, "#scribe forget preferences-1");
    assert_eq!(show()["status"], "forgotten");
    assert_eq!(show()["content"], "");
}
