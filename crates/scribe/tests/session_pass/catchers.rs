#[test]
fn config_only_catchers_from_the_spec_catch_from_their_speakers_with_no_code_change() {
    // The orbit collector reads the user, the todo catcher the assistant
    // and the issue example both, on an incident turn; each is only the
    // config block specs/catchers.md documents.
    let s = Setup::new();
    s.config(&format!(
        "{}\n{}\n{}",
        catcher_example("orbit"),
        catcher_example("todo"),
        catcher_example("issue")
    ));
    let orbit = "The orbit drain keeps stalling on big backlogs.";
    let todo = "You need to approve the GitHub app install yourself, since it needs your login.";
    let asked = "can you investigate why the drain keeps failing with HTTP 429";
    let found =
        "The root cause was parallel drains sharing one token; fixed by serialising pushes.";
    s.write(
        ON,
        &[
            line("user", "u-orbit", orbit),
            line("assistant", "a-todo", todo),
            line("user", "u-asked", asked),
            line("assistant", "a-found", found),
            line("user", "u-plain", "Thanks, that is all."),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        written_claim(
            orbit,
            "u-orbit",
            "orbit",
            "The user says the Orbit drain stalls on big backlogs."
        ),
        written_claim(
            todo,
            "a-todo",
            "todo",
            "The user must approve the GitHub app install."
        ),
        written_claim(
            asked,
            "u-asked",
            "issue",
            "The user asked why the drain fails with HTTP 429."
        ),
        written_claim(
            found,
            "a-found",
            "issue",
            "Parallel drains shared one token; serialising pushes fixed it."
        ),
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);

    let prompt = &s.prompts()[0];
    for listed in [
        "- message u-orbit (the user's turn): tag orbit (written; trigger \"orbit\", matched \"orbit\")",
        "- message a-todo (the assistant's turn): tag todo (written; trigger \"you need to\", matched \"You need to\")",
        "- message u-asked (the user's turn): tag issue (written; trigger \"can you investigate\", matched \"can you investigate\")",
        "- message a-found (the assistant's turn): tag issue (written; trigger \"root cause\", matched \"root cause\")",
    ] {
        assert!(prompt.contains(listed), "{listed}\n{prompt}");
    }
    assert!(!prompt.contains("message u-plain"), "{prompt}");
    // `todo` reads only the assistant: the user's turns never reach it.
    assert!(!prompt.contains("(the user's turn): tag todo"), "{prompt}");

    assert_eq!(proposals(&s)["total"], 0, "write catchers open no claims");
    for (tag, quotes) in [
        ("orbit", vec![(orbit, "user")]),
        ("todo", vec![(todo, "assistant")]),
        ("issue", vec![(asked, "user"), (found, "assistant")]),
    ] {
        let listed = entries_json(&s, &["--tag", tag]);
        assert_eq!(listed["total"], quotes.len(), "{tag}: {listed}");
        for (quote, role) in quotes {
            let entry = listed["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["quote"] == quote)
                .unwrap_or_else(|| panic!("{tag}: {quote}: {listed}"));
            assert_eq!(entry["content_from"]["role"], role, "{entry}");
            assert_eq!(entry["author"]["name"], "session-pass", "{entry}");
        }
    }
    // Readers never give the assistant's words to the user.
    let found: Value = s.fx.check(
        Cli(&[
            "search",
            "approve GitHub app install",
            "--kind",
            "entry",
            "--json",
        ]),
        Json,
    );
    let hit = found.to_string();
    assert!(hit.contains("the assistant's turn"), "{hit}");
    assert!(hit.contains("The assistant's words:"), "{hit}");
    assert!(!hit.contains("The user's words:"), "{hit}");
}

#[test]
fn an_assistant_quote_under_a_user_catcher_is_refused() {
    // `orbit` reads the user only: a claim quoting the assistant's turn is
    // not one of the listed turns, and the store writes nothing.
    let s = Setup::new();
    s.config(&catcher_example("orbit"));
    let said = "I restarted the orbit drain on the box.";
    s.write(
        ON,
        &[
            line("user", "u-1", "Please restart it."),
            line("assistant", "a-1", said),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([written_claim(
        said,
        "a-1",
        "orbit",
        "The drain was restarted."
    )]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    assert!(
        s.prompts()[0].contains("Return candidates: [] (none are requested in this window)."),
        "{}",
        s.prompts()[0]
    );
    assert_eq!(entries_json(&s, &["--tag", "orbit"])["total"], 0);
}

#[test]
fn max_open_caps_one_tags_open_claims_and_leaves_the_others_theirs() {
    let s = Setup::new();
    s.config("[tags.preferences.capture]\nmax_open = 1\n");
    let quotes = [
        ("u-1", "I always want short commit subjects.", "preferences"),
        (
            "u-2",
            "I prefer tabs to spaces in Makefiles.",
            "preferences",
        ),
        (
            "u-3",
            "As a standard every repo keeps a CHANGELOG.",
            "standard",
        ),
    ];
    s.write(
        ON,
        &quotes
            .iter()
            .map(|(id, quote, _)| line("user", id, quote))
            .collect::<Vec<_>>(),
    );
    let mut body = answer();
    body["candidates"] = json!(
        quotes
            .iter()
            .map(|(id, quote, tag)| json!({"quote": quote, "message": id, "tag": tag}))
            .collect::<Vec<_>>()
    );
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let open = proposals(&s);
    let tags: Vec<&str> = open["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["candidate"]["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, ["preferences", "standard"], "{open}");
    assert_eq!(open["proposals"][0]["candidate"]["quote"], quotes[0].1);
}

#[test]
fn approved_assistant_claim_shows_statement_and_assistant_words_in_ask_and_search() {
    let s = Setup::new();
    s.config(
        "[tags.notes]\n\
         description = \"Noteworthy assistant observations.\"\n\n\
         [tags.notes.capture]\n\
         from = \"assistant\"\n\
         route = \"propose\"\n\
         triggers = [\"root cause\"]\n",
    );
    let quote = "The root cause is that parallel drains share a single auth token.";
    let context = "The user and the agent investigated repeated drain stalls. The agent reported that parallel drains share one auth token and collide on refresh.";
    s.write(
        ON,
        &[
            line("user", "u-1", "Why does the drain stall?"),
            line("assistant", "a-1", quote),
        ],
    );
    let mut pass_body = answer();
    pass_body["candidates"] = json!([{
        "quote": quote,
        "message": "a-1",
        "tag": "notes",
        "context": context,
        "subject": "shared drain auth token"
    }]);
    s.answer(&pass_body);
    s.scribe("/scribe on");
    s.pass_session(ON);

    let open = proposals(&s);
    assert_eq!(open["total"], 1, "{open}");
    assert_eq!(open["proposals"][0]["candidate"]["role"], "assistant");
    let id = open["proposals"][0]["id"].as_str().unwrap().to_owned();
    type_settlement(&s, &format!("#scribe approve {id}"));

    let shown: Value = s.entry_json("notes-1");
    assert_eq!(shown["author"]["kind"], "agent");
    assert_eq!(shown["author"]["name"], "assistant");
    assert_eq!(shown["content"], context);
    assert_eq!(shown["quote"], quote);
    assert_eq!(
        shown["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );

    let table = s.fx.check(
        Cli(&["entry", "show", "notes-1", "--format", "table"]),
        Text,
    );
    assert!(
        table.contains("scribe's statement of the assistant's turn"),
        "{table}"
    );
    assert!(!table.contains("the user's turn"), "{table}");

    // `scribe search` labels it as scribe's statement of the assistant's turn
    // with the assistant's quote, never as an agent note or the user's words.
    let found: Value = s.fx.check(
        Cli(&[
            "search",
            "parallel drains share",
            "--kind",
            "entry",
            "--json",
        ]),
        Json,
    );
    assert_eq!(found["total"], 1, "{found}");
    let snippet = found["hits"][0]["snippet"].as_str().unwrap();
    assert!(
        snippet.contains("scribe's statement of the assistant's turn:"),
        "{snippet}"
    );
    assert!(
        snippet.contains(&format!("The assistant's words: \"{quote}\"")),
        "{snippet}"
    );
    assert!(!snippet.contains("agent note"), "{snippet}");
    assert!(!snippet.contains("The user's words"), "{snippet}");
    assert!(!snippet.contains("the user's turn"), "{snippet}");
}

#[test]
fn approved_assistant_claim_refuses_content_update_and_stays_readable() {
    let s = Setup::new();
    s.config(
        "[tags.notes]\n\
         description = \"Noteworthy assistant observations.\"\n\n\
         [tags.notes.capture]\n\
         from = \"assistant\"\n\
         route = \"propose\"\n\
         triggers = [\"root cause\"]\n",
    );
    let quote = "The root cause is that parallel drains share a single auth token.";
    let context = "The user and the agent investigated repeated drain stalls. The agent reported that parallel drains share one auth token and collide on refresh.";
    s.write(
        ON,
        &[
            line("user", "u-1", "Why does the drain stall?"),
            line("assistant", "a-1", quote),
        ],
    );
    let mut pass_body = answer();
    pass_body["candidates"] = json!([{
        "quote": quote,
        "message": "a-1",
        "tag": "notes",
        "context": context,
        "subject": "shared drain auth token"
    }]);
    s.answer(&pass_body);
    s.scribe("/scribe on");
    s.pass_session(ON);

    let open = proposals(&s);
    let id = open["proposals"][0]["id"].as_str().unwrap().to_owned();
    type_settlement(&s, &format!("#scribe approve {id}"));

    let before: Value = s.entry_json("notes-1");

    // Content edits (even alongside metadata flags) must be refused before
    // changing the approved assistant-context entry, keeping it readable.
    let out = s.fx.check(
        Cli(&[
            "entry",
            "update",
            "notes-1",
            "--author",
            "opus",
            "--content",
            "Rewritten by opus.",
            "--add-topic",
            "drains",
            "--json",
        ]),
        Capture,
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "not_agent_note", "{err}");

    let after_refusal: Value = s.entry_json("notes-1");
    assert_eq!(after_refusal, before);

    // Metadata-only updates remain allowed and keep `stated_by` intact.
    let patched: Value = s.fx.check(
        Cli(&[
            "entry",
            "update",
            "notes-1",
            "--author",
            "opus",
            "--add-topic",
            "drains",
            "--add-related",
            "repo:scribe",
            "--json",
        ]),
        Json,
    );
    assert_eq!(patched["content"], context);
    assert_eq!(
        patched["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    assert_eq!(patched["topics"], json!(["drains"]));
    assert_eq!(patched["related"], json!(["repo:scribe"]));
    let reread: Value = s.entry_json("notes-1");
    assert_eq!(reread["content"], context);
    assert_eq!(
        reread["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    assert_eq!(reread["topics"], json!(["drains"]));
    assert_eq!(reread["related"], json!(["repo:scribe"]));
    assert_eq!(reread["admitted"], before["admitted"]);
}

#[test]
fn older_approved_assistant_finding_refuses_content_update() {
    let quote = "Measured: the reviewer mix alone predicts a 7-point first-pass gap.";
    let s = setup_stored_assistant_finding_candidate(quote);
    type_settlement(&s, "#scribe approve p-1");
    let before: Value = s.entry_json("finding-1");

    let out = s.fx.check(
        Cli(&[
            "entry",
            "update",
            "finding-1",
            "--author",
            "opus",
            "--content",
            "Rewritten finding.",
            "--json",
        ]),
        Capture,
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "not_agent_note", "{err}");

    let after: Value = s.entry_json("finding-1");
    assert_eq!(after, before);
}

#[test]
fn unmatched_session_matrix_outcomes() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    let before = setup.control_bytes();
    let reason = setup.scribe(&format!("/scribe link {UNKNOWN}"));
    let expected = support::session_id_outcome("manual_link");
    assert_eq!(expected["exit"], 0); // scribe observed hook exit 0
    assert!(
        reason.contains(expected["value"].as_str().unwrap()),
        "{reason}"
    );
    assert_eq!(setup.audit_codes().last().unwrap(), &expected["code"]);
    assert_eq!(setup.control_bytes(), before);
    let out = setup.run_any(&["session", "pass", "--session", UNKNOWN, "--json"], b"");
    let expected = support::session_id_outcome("session_pass");
    assert_eq!(
        out.status.code(),
        expected["exit"].as_i64().map(|n| i32::try_from(n).unwrap())
    );
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], expected["code"]);
}
