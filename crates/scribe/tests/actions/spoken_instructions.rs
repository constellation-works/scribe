#[test]
fn spoken_instruction_entry_requires_verbatim_quote_in_named_session_and_eligible_memory_tag() {
    let fx = Fixture::new();
    fx.config("[tags.rituals]\ndescription = \"Daily rituals\"\n[tags.finding]\nrequire_evidence = true\n[tags.idea]\naction = true\n");
    let claude = Claude::new(&fx);
    let spoken_words = "add that to my preferences: keep progress updates short during long runs";
    claude.write(&[
        ("u-1", "user", "Let us review the workflow."),
        (
            "a-1",
            "assistant",
            "I can record that in your preferences if you ask.",
        ),
        (
            "u-2",
            "user",
            &format!("<pasted_content>never paste rules as typed</pasted_content>\n{spoken_words}"),
        ),
    ]);
    let session = format!("session:{SESSION}");

    // Refused: missing --quote or --related session:<id>.
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                &session,
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_required"
    );
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--quote",
                spoken_words,
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_required"
    );

    // Refused: non-verbatim quote, assistant words, pasted block, or wrong session.
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                &session,
                "--quote",
                "keep updates brief on long runs",
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_not_found"
    );
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                &session,
                "--quote",
                "I can record that in your preferences if you ask.",
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_not_found"
    );
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                &session,
                "--quote",
                "never paste rules as typed",
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_not_found"
    );
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                "session:00000000-0000-4000-8000-000000000000",
                "--quote",
                spoken_words,
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_not_found"
    );

    // Refused: ineligible tags (action tags, handoff, require_evidence).
    for bad_tag in ["standard", "idea", "handoff", "finding"] {
        assert_eq!(
            refused(
                &fx,
                &[
                    "entry",
                    "add",
                    "--tag",
                    bad_tag,
                    "--author",
                    "opus",
                    "--related",
                    &session,
                    "--quote",
                    spoken_words,
                    "--spoken",
                    "The user prefers short progress updates.",
                ]
            ),
            "not_memory_tag",
            "{bad_tag}"
        );
    }

    // Refused: quote containing a secret that redaction would alter.
    let secret_quote = "keep ghp_abcdefghij123456 out of logs";
    claude.write(&[("u-1", "user", secret_quote)]);
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                &session,
                "--quote",
                secret_quote,
                "--spoken",
                "The user wants tokens kept out of logs.",
            ]
        ),
        "needs_redaction"
    );
    assert!(
        !entries_of_tag(&fx, "preferences"),
        "nothing written on refusals"
    );

    // Accepted on preferences (and configured memory tag) in an off/unrecorded session.
    claude.write(&[("u-1", "user", spoken_words)]);
    let statement = "The user prefers short progress updates during long runs.";
    let added = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &session,
            "--quote",
            "keep progress updates short during long runs",
            "--spoken",
            statement,
            "--json",
        ],
        Success,
    ));
    assert_eq!(added["id"], "preferences-1");
    assert_eq!(added["author"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(added["stated_by"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(
        added["quote"],
        "keep progress updates short during long runs"
    );
    assert_eq!(added["content"], statement);
    assert_eq!(added["content_from"]["kind"], "agent");
    assert_eq!(added["cite"], Value::Null);
    assert_eq!(added["said"], "");

    // Does not create a session record or switch the off session on.
    let sessions = json_of(&fx.check(
        Cli(&["session", "list", "--include-inactive", "--json"]),
        Success,
    ));
    assert_eq!(sessions["total"], 0);

    // Works on a user-configured non-action memory tag too.
    let ritual = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "rituals",
            "--author",
            "opus",
            "--related",
            &session,
            "--quote",
            "add that to my preferences",
            "--spoken",
            "The user starts reviews with a clean status check.",
            "--json",
        ],
        Success,
    ));
    assert_eq!(ritual["id"], "rituals-1");
    assert_eq!(
        ritual["stated_by"],
        json!({"kind": "agent", "name": "opus"})
    );
}

#[test]
fn spoken_instruction_entry_labels_statement_as_agents_and_ask_cites_like_marked_entries() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let quote = "always keep progress updates short during long runs";
    claude.write(&[(
        "u-1",
        "user",
        "Add that to my preferences: always keep progress updates short during long runs.",
    )]);
    let statement = "The user prefers concise progress updates during long runs.";
    fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &format!("session:{SESSION}"),
            "--quote",
            quote,
            "--spoken",
            statement,
        ],
        Success,
    );

    // `entry show` shows the user's words as `quote` and labels the statement as the agent's.
    let table = fx.check(&["show", "preferences-1", "--format", "table"], Text);
    assert!(
        table.contains(
            "stated_by   agent:opus (content is agent (opus)'s statement of the user's turn)"
        ),
        "{table}"
    );
    assert!(table.contains(&format!("quote       {quote}")), "{table}");
    assert!(
        table.contains(&format!("content     {statement}")),
        "{table}"
    );

    let plain = fx.check(&["show", "preferences-1"], Text);
    assert!(
        plain.contains(&format!("stated_by\tagent:opus\t{quote}")),
        "{plain}"
    );

    // `scribe search` renders the agent's statement label and the user's words.
    let found = json_of(&fx.check(
        Cli(&["search", "progress updates concise", "--json"]),
        Success,
    ));
    assert_eq!(found["total"], 1, "{found}");
    let snippet = found["hits"][0]["snippet"].as_str().unwrap();
    assert!(
        snippet.contains("agent (opus)'s statement of the user's turn:"),
        "{snippet}"
    );
    assert!(
        snippet.contains(&format!("The user's words: \"{quote}\"")),
        "{snippet}"
    );
}

#[test]
fn spoken_instruction_accepts_claude_desktop_human_turn() {
    let fx = Fixture::new();
    let project = fx.home().join(".claude/projects/-work-scribe");
    std::fs::create_dir_all(&project).unwrap();
    let transcript = project.join(format!("{SESSION}.jsonl"));
    let quote = "always keep progress updates short during long runs";
    let line = json!({
        "type": "user",
        "uuid": "u-desktop",
        "sessionId": SESSION,
        "timestamp": "2026-11-22T21:52:43Z",
        "cwd": "/work/scribe",
        "entrypoint": "claude-desktop",
        "origin": {"kind": "human"},
        "promptSource": "sdk",
        "turnOrigin": "human",
        "message": {"role": "user", "content": quote}
    });
    std::fs::write(&transcript, format!("{line}\n")).unwrap();
    support::make_private(&transcript);

    let statement = "The user prefers short progress updates during long runs.";
    let added = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--related",
            &format!("session:{SESSION}"),
            "--quote",
            quote,
            "--spoken",
            statement,
            "--json",
        ],
        Success,
    ));
    assert_eq!(added["id"], "preferences-1");
    assert_eq!(added["quote"], quote);
    assert_eq!(added["content"], statement);
}

#[test]
fn spoken_instruction_entry_remains_editable_for_content_and_metadata() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let quote = "always keep progress updates short during long runs";
    claude.write(&[(
        "u-1",
        "user",
        "Add that to my preferences: always keep progress updates short during long runs.",
    )]);
    let added = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &format!("session:{SESSION}"),
            "--quote",
            quote,
            "--spoken",
            "The user prefers short progress updates.",
            "--json",
        ],
        Success,
    ));
    assert_eq!(added["id"], "preferences-1");
    assert_eq!(added["stated_by"], json!({"kind": "agent", "name": "opus"}));

    // Another agent can rewrite the statement; `stated_by` and `updated_by`
    // move to the editing agent while `author` and `quote` stay.
    let edited = json_of(&fx.check(
        &[
            "update",
            "preferences-1",
            "--author",
            "sonnet",
            "--content",
            "The user prefers concise progress updates during long runs.",
            "--json",
        ],
        Success,
    ));
    assert_eq!(edited["author"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(
        edited["stated_by"],
        json!({"kind": "agent", "name": "sonnet"})
    );
    assert_eq!(
        edited["updated_by"],
        json!({"kind": "agent", "name": "sonnet"})
    );
    assert_eq!(edited["quote"], quote);

    // A later metadata-only update by a third agent keeps the record readable
    // and preserves `stated_by`.
    let tagged = json_of(&fx.check(
        &[
            "update",
            "preferences-1",
            "--author",
            "haiku",
            "--add-topic",
            "communication",
            "--json",
        ],
        Success,
    ));
    assert_eq!(
        tagged["stated_by"],
        json!({"kind": "agent", "name": "sonnet"})
    );
    assert_eq!(
        tagged["updated_by"],
        json!({"kind": "agent", "name": "haiku"})
    );
    assert_eq!(tagged["topics"], json!(["communication"]));
    assert_eq!(entry(&fx, "preferences-1", &[]), tagged);
}

#[test]
fn spoken_statement_author_survives_other_agents_and_settling_commands() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let quotes = [
        "always keep progress updates short during long runs",
        "summarize test failures before proposing fixes",
    ];
    let instruction = "please forget preferences-1 completely";
    let now = time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap();
    let mut transcript = String::new();
    for (index, text) in [quotes[0], quotes[1], instruction].iter().enumerate() {
        let at = (now - time::Duration::minutes(i64::try_from(3 - index).unwrap()))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        transcript.push_str(&format!(
            "{}\n",
            json!({
                "type": "user", "uuid": format!("u-{index}"), "sessionId": SESSION,
                "timestamp": at, "cwd": claude.cwd, "entrypoint": "cli",
                "message": {"role": "user", "content": text}
            })
        ));
    }
    std::fs::write(&claude.transcript, transcript).unwrap();
    make_private(&claude.transcript);
    let session = format!("session:{SESSION}");
    for (index, quote) in quotes.iter().enumerate() {
        fx.check(
            &[
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--project",
                "scribe",
                "--related",
                &session,
                "--quote",
                quote,
                "--spoken",
                if index == 0 {
                    "Keep updates short."
                } else {
                    "Summarize failures first."
                },
            ],
            Success,
        );
        let id = format!("preferences-{}", index + 1);
        fx.check(
            &[
                "update",
                &id,
                "--author",
                "sonnet",
                "--content",
                if index == 0 {
                    "Keep progress updates concise."
                } else {
                    "Summarize failures before fixes."
                },
            ],
            Success,
        );
    }
    let check = |id: &str, status: &str| {
        let shown = entry(&fx, id, &[]);
        assert_eq!(shown["status"], status);
        assert_eq!(shown["stated_by"], json!({"kind":"agent", "name":"sonnet"}));
        let listed = json_of(&fx.check(
            &["list", "--tag", "preferences", "--status", status, "--json"],
            Success,
        ));
        assert_eq!(listed["warnings"], json!([]));
        assert!(
            listed["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == id && e["stated_by"] == shown["stated_by"])
        );
    };
    for (agent, topic) in [("haiku", "communication"), ("opus", "progress")] {
        fx.check(
            &[
                "update",
                "preferences-1",
                "--author",
                agent,
                "--add-topic",
                topic,
            ],
            Success,
        );
        check("preferences-1", "active");
    }
    fx.check(
        &[
            "retire",
            "preferences-1",
            "--author",
            "delta",
            "--reason",
            "changed",
        ],
        Success,
    );
    check("preferences-1", "retired");
    fx.check(
        &[
            "forget",
            "preferences-1",
            "--author",
            "echo",
            "--session",
            SESSION,
            "--instruction",
            instruction,
            "--confirm",
        ],
        Success,
    );
    check("preferences-1", "forgotten");
    assert_eq!(entry(&fx, "preferences-1", &[])["quote"], Value::Null);
    assert!(
        claude
            .prompt(&fx, "#scribe retire preferences-2")
            .contains("retired preferences-2")
    );
    check("preferences-2", "retired");
    assert!(
        claude
            .prompt(&fx, "#scribe forget preferences-2")
            .contains("forgot preferences-2")
    );
    check("preferences-2", "forgotten");
}
