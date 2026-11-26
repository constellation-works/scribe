#[test]
fn session_start_skips_the_reminder_when_this_session_is_already_opted_in() {
    let fx = Fixture::new();
    let cwd = project_cwd(&fx);
    let now = now_rfc3339();
    write_session(&fx, PREVIOUS, "scribe", &now, &now, true);
    write_switch_on(&fx, CURRENT);

    assert_eq!(
        start_message(&fx, &start_payload(Some(&cwd), CURRENT)),
        "",
        "a session that is already on is not reminded"
    );

    // A record of this session, even beside a newer one, means it is already cataloged.
    let resumed = Fixture::new();
    let cwd = project_cwd(&resumed);
    write_session(&resumed, CURRENT, "scribe", &now, &now, true);
    write_session(&resumed, NEWER, "scribe", &now, &now, true);
    assert_eq!(
        start_message(&resumed, &start_payload(Some(&cwd), CURRENT)),
        ""
    );
}

#[test]
fn session_start_skips_the_reminder_when_no_recent_session_matches() {
    let now = now_rfc3339();
    let cases: &[(&str, &str, &str, bool, bool)] = &[
        ("stale", PREVIOUS, "scribe", true, false),
        ("other project", PREVIOUS, "other", true, true),
        ("inactive", PREVIOUS, "scribe", false, true),
    ];
    for (name, id, project, active, recent) in cases {
        let fx = Fixture::new();
        let cwd = project_cwd(&fx);
        let started = if *recent {
            now.clone()
        } else {
            String::from("2020-01-02T00:00:00Z")
        };
        let updated = started.clone();
        write_session(&fx, id, project, &started, &updated, *active);
        assert_eq!(
            start_message(&fx, &start_payload(Some(&cwd), CURRENT)),
            "",
            "{name}"
        );
    }

    let fx = Fixture::new();
    write_session(&fx, PREVIOUS, "scribe", &now, &now, true);
    assert_eq!(
        start_message(&fx, &start_payload(None, CURRENT)),
        "",
        "no cwd, no project to match"
    );
}

#[cfg(unix)]
#[test]
fn session_start_prints_no_reminder_when_the_sessions_cannot_be_read() {
    let fx = Fixture::new();
    let cwd = project_cwd(&fx);
    let now = now_rfc3339();
    write_session(&fx, PREVIOUS, "scribe", &now, &now, true);
    write_open_proposal(&fx);
    // A session row whose project is empty cannot be decoded.
    support::sql(
        &fx.root(),
        &format!("UPDATE sessions SET project = '' WHERE provider_session = '{PREVIOUS}'"),
    );

    let message = start_message(&fx, &start_payload(Some(&cwd), CURRENT));
    assert_eq!(
        message, "scribe: 1 proposal to review — scribe proposal list",
        "an unreadable session store adds no continuation, and the hook still starts"
    );
    assert!(audit(&fx).is_empty(), "a failed read is not audited");
}

#[test]
fn decision_and_open_item_markers_are_refused_before_a_model_call() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    marked_turns(
        &claude,
        &[
            (
                "u-1",
                "#scribe decisions: squash-merge agent PRs\n#scribe decision: rebase never\n#scribe dec: keep merge commits",
            ),
            (
                "u-2",
                "#scribe open-item: Which check runs?\n#scribe item: Restart the server\n#scribe open-items: Which host?",
            ),
        ],
    );
    let path = fake_claude(&fx, "exit 1");
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert_eq!(
        message
            .matches("record decisions and open items through scribe entry add")
            .count(),
        6,
        "{message}"
    );
    assert!(!fx.home().join("claude-stdin-0").exists());
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(codes, vec![json!("unknown_tag"); 6]);
    assert!(entries(&fx).is_empty());
}

#[test]
fn a_config_that_declares_decisions_makes_it_work_like_any_config_tag() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_config(
        &fx,
        "[tags.decisions]\naliases = [\"decision\", \"dec\"]\ndescription = \"choices Daniel has made\"\n",
    );
    let typed =
        "#scribe decisions: squash-merge agent PRs\n#scribe dec: keep merge commits for releases";
    marked_turns(&claude, &[("u-1", typed)]);
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let reply_json = reply(&out);
    assert_eq!(
        reply_json["systemMessage"],
        "scribe: #scribe in your message is the agent's to record — \"#scribe decisions: squash-merge agent PRs\"\n\
         scribe: #scribe in your message is the agent's to record — \"#scribe dec: keep merge commits for releases\""
    );
    assert_eq!(
        reply_json["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .matches("scribe entry add --tag decisions ")
            .count(),
        2
    );
    fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "decisions",
            "--author",
            "opus",
            "a note",
        ]),
        Success,
    );
    let list = fx.check(
        Cli(&["entry", "list", "--tag", "decisions", "--json"]),
        Success,
    );
    let list: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list["total"], 1, "{list}");
    let tags = fx.check(Cli(&["tag", "list", "--json"]), Success);
    let tags: Value = serde_json::from_slice(&tags.stdout).unwrap();
    let names: Vec<&str> = tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"decisions"), "{names:?}");
}

#[test]
fn a_config_naming_the_retired_referent_model_loads_with_a_warning() {
    let fx = Fixture::new();
    write_config(&fx, "[models]\nreferent = \"claude-haiku-5-5\"\n");
    let out = fx.check(Cli(&["tag", "list", "--json"]), Success);
    let err = text(&out.stderr);
    assert!(
        err.contains("config key models.referent is unused"),
        "{err}"
    );
    assert_eq!(
        err.matches("models.referent").count(),
        1,
        "warned once: {err}"
    );
}

#[test]
fn entries_under_decisions_in_a_store_without_that_tag_stay_readable_and_settleable() {
    // A store from before the tag stopped shipping: entries under
    // `decisions`, no config naming it.
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    fx.seed();
    write_config(&fx, "# no tag table: the defaults apply\n");
    let tags = fx.check(Cli(&["tag", "list", "--json"]), Success);
    let tags: Value = serde_json::from_slice(&tags.stdout).unwrap();
    let row = tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "decisions")
        .unwrap();
    assert_eq!(row["entries"], 2, "{row}");
    assert_eq!(row["configured"], false, "{row}");

    // `decisions` now resolves to the shipped `decision` tag. List without a
    // tag to verify the historical entries remain readable under their
    // original stored tag.
    let list = fx.check(Cli(&["entry", "list", "--json"]), Success);
    let list: Value = serde_json::from_slice(&list.stdout).unwrap();
    let listed: Vec<&str> = list["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert!(listed.contains(&"decisions-1"), "{listed:?}");
    assert!(listed.contains(&"decisions-2"), "{listed:?}");
    let shown = fx.check(Cli(&["entry", "show", "decisions-2", "--json"]), Success);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert!(
        shown["content"]
            .as_str()
            .unwrap()
            .contains("squash-merge agent PRs"),
        "{shown}"
    );
    let found = fx.check(Cli(&["search", "squash", "--json"]), Success);
    assert!(
        text(&found.stdout).contains("decisions-2"),
        "search finds it: {}",
        text(&found.stdout)
    );

    // The alias requires typed fields for a new decision. The older entries
    // still settle by their original ids.
    let refused = fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "decisions",
            "--author",
            "opus",
            "a new one",
        ]),
        Capture,
    );
    assert_ne!(refused.status.code(), Some(0));
    assert!(
        text(&refused.stderr).contains("a decision requires --decided-by"),
        "{}",
        text(&refused.stderr)
    );
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe retire decisions-2");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(
        text(&out.stdout).contains("retired decisions-2"),
        "{}",
        text(&out.stdout)
    );
    let shown = fx.check(Cli(&["entry", "show", "decisions-2", "--json"]), Success);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["status"], "retired");
}

// --- annotations: `#scribe` inside a sentence, handed over at Stop (§14) ---

#[test]
fn a_marker_inside_a_sentence_is_echoed_at_submit_and_writes_nothing() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let path = fake_claude(&fx, "exit 1");
    let out = hook_path(
        &fx,
        "user-prompt-submit",
        &prompt_payload(&claude, MARKED),
        &path,
    );
    let reply = reply(&out);
    assert_eq!(
        reply["systemMessage"],
        "scribe: marked — cataloged after the reply"
    );
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("hands this #scribe marker to you with the command that records it; record nothing for it before then"),
        "{context}"
    );
    assert!(claude_calls(&fx).is_empty(), "no model call at submit");
    assert!(!fx.root().exists(), "nothing written at submit");
}

#[test]
fn a_marker_after_a_stray_quote_or_ending_a_global_sentence_is_echoed_and_handed_over() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    // A lone `"` (inches) opens no quoted span, and sentence punctuation
    // may follow `#scribe/global`.
    let typed = "I buy the 12\" pipe from the corner shop #scribe/global.";
    let submitted = hook_ok(&fx, "user-prompt-submit", &prompt_payload(&claude, typed));
    assert_eq!(
        reply(&submitted)["systemMessage"],
        "scribe: marked — cataloged after the reply"
    );
    marked_turns(&claude, &[("u-1", typed)]);
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let reply_json = reply(&out);
    let message = reply_json["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.starts_with("scribe: #scribe in your message is the agent's to record"),
        "{message}"
    );
    let context = reply_json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("scribe entry add --tag <tag> --project global"),
        "{context}"
    );
    let marks = marks_json(&fx);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0]["outcome"], "refused");
    assert_eq!(marks[0]["code"], "agent_handoff");
}

#[test]
fn a_long_marked_line_reaches_the_agent_whole_and_only_the_echo_is_cut() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    // Over 200 characters, with runs of spaces inside and a suffix that only
    // the whole line carries. The agent records the user's words, so its
    // copy keeps every character; the user's echo is a bounded preview.
    let filler = "word   ".repeat(40);
    let typed = format!("#scribe preferences: keep  this   {filler}ENDING  tail.");
    assert!(typed.chars().count() > 200, "{typed}");
    let submitted = hook_ok(&fx, "user-prompt-submit", &prompt_payload(&claude, &typed));
    assert_eq!(
        reply(&submitted)["systemMessage"],
        "scribe: marked — cataloged after the reply"
    );
    marked_turns(&claude, &[("u-1", &typed)]);
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let reply_json = reply(&out);
    let message = reply_json["systemMessage"].as_str().unwrap();
    let context = reply_json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains(&format!("\"{typed}\"")), "{context}");
    assert!(message.ends_with("…\""), "{message}");
    assert!(!message.contains("ENDING"), "{message}");
    let marks = marks_json(&fx);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0]["code"], "agent_handoff");
}

#[test]
fn a_fourth_marker_is_refused_too_many_marks() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let four = "a #scribe b #scribe c #scribe d #scribe";
    let submitted = hook_ok(&fx, "user-prompt-submit", &prompt_payload(&claude, four));
    let echo = reply(&submitted)["systemMessage"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        echo.contains("scribe: marked 3 times — cataloged after the reply"),
        "{echo}"
    );
    assert!(
        echo.contains("at most 3 #scribe markers in one message"),
        "{echo}"
    );

    marked_turns(&claude, &[("u-1", four)]);
    hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let marks = marks_json(&fx);
    assert_eq!(marks.len(), 4);
    let fourth = marks.iter().find(|m| m["marker"] == 4).unwrap();
    assert_eq!(fourth["outcome"], "refused");
    assert_eq!(fourth["code"], "too_many_marks");
    assert!(
        marks
            .iter()
            .filter(|m| m["marker"] != 4)
            .all(|m| m["code"] == "agent_handoff"),
        "{marks:?}"
    );
    assert!(
        audit(&fx)
            .iter()
            .any(|l| l["error_code"] == "too_many_marks")
    );
}

#[test]
fn a_mention_or_an_agent_turn_is_never_cataloged() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    marked_turns(
        &claude,
        &[
            ("u-1", "the `#scribe` marker and \"#scribe\" in quotes"),
            (
                "u-2",
                "<agent-message from=\"x\">\nplease note #scribe\n</agent-message>",
            ),
        ],
    );
    let path = fake_claude(&fx, "exit 1");
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(claude_calls(&fx).is_empty());
    assert!(marks_json(&fx).is_empty());
}

#[test]
fn session_start_gives_an_interactive_agent_the_brief() {
    let fx = Fixture::new();
    let cwd = project_cwd(&fx);
    write_session(
        &fx,
        PREVIOUS,
        "scribe",
        "2026-11-16T10:00:00Z",
        "2026-11-16T11:00:00Z",
        true,
    );
    write_session(
        &fx,
        NEWER,
        "orbit",
        "2026-11-17T10:00:00Z",
        "2026-11-17T11:00:00Z",
        true,
    );
    write_session(
        &fx,
        CURRENT,
        "scribe",
        "2026-11-18T10:00:00Z",
        "2026-11-18T11:00:00Z",
        true,
    );
    write_open_proposal(&fx);
    let before = snapshot(&fx.root());
    let payload = start_payload(Some(&cwd), CURRENT);

    let interactive = start_from(&fx, "cli", &payload);
    let context = interactive["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("{interactive}"));
    assert!(context.starts_with("# scribe brief\n"), "{context}");
    // This project's session first, then another project's; never the
    // session starting.
    assert!(
        context.contains(
            "## Recent sessions\n- s-1 · 2026-11-16 11:00Z · scribe · Continue the work\n- s-2 · 2026-11-17 11:00Z · orbit · Continue the work\n"
        ),
        "{context}"
    );
    assert!(!context.contains(CURRENT), "{context}");
    assert!(
        context.contains(
            "## Open claims\n- p-1 · 2026-11-16 12:00Z · rituals · \"run the full gate\"\n"
        ),
        "{context}"
    );
    // The user's count stays beside it.
    assert_eq!(
        interactive["systemMessage"],
        "scribe: 1 proposal to review — scribe proposal list"
    );

    let headless = start_from(&fx, "sdk-cli", &payload);
    assert_eq!(headless["hookSpecificOutput"], Value::Null, "{headless}");
    assert!(audit(&fx).is_empty(), "the brief is not audited");
    assert_eq!(
        snapshot(&fx.root()),
        before,
        "session-start wrote the store"
    );
}
