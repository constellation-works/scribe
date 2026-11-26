#[test]
fn a_window_where_no_daniel_turn_fires_a_trigger_requests_no_claims() {
    let s = Setup::new();
    let quote = "Let us use the blue-green rollout for this migration.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            // An assistant turn's trigger words never count.
            line("assistant", "a-1", "I always prefer to check first."),
        ],
    );
    // A model that ignores the request still opens nothing.
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);
    assert_eq!(proposals(&s)["total"], 0);
    let prompts = s.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("Return candidates: [] (none are requested in this window)."),
        "{}",
        prompts[0]
    );
    assert!(!prompts[0].contains("Already recorded or proposed"));
}

#[test]
fn a_firing_turn_is_named_with_its_tag_and_trigger_and_idea_configured_to_propose_proposes() {
    let s = Setup::new();
    // Config can put `idea` back on the candidate route.
    s.config(&support::idea_catcher_on("propose"));
    let idea = "What if we cached the routing table per host?";
    s.write(
        ON,
        &[
            line("user", "u-plain", "Please look at the routing table."),
            line("user", "u-idea", idea),
        ],
    );
    s.answer(&candidate_answer(idea, "u-idea", "idea"));
    s.scribe("/scribe on");
    s.pass_session(ON);
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains(
            "- message u-idea (the user's turn): tag idea (proposed; trigger \"what if we\", matched \"What if we\")"
        ),
        "{prompt}"
    );
    assert!(!prompt.contains("message u-plain"), "{prompt}");
    assert!(
        prompt.contains("- idea: qualifies: The user proposes something that could be built"),
        "{prompt}"
    );
    assert!(!prompt.contains("under written tags:"), "{prompt}");
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    let p = &listed["proposals"][0];
    assert_eq!(p["kind"], "candidate", "the propose route");
    assert_eq!(p["state"], "open");
    assert_eq!(p["candidate"]["tag"], "idea");
    assert_eq!(p["candidate"]["admitted"]["trigger"], "what if we");
    assert_eq!(p["candidate"]["admitted"]["matched"], "What if we");
}

#[test]
fn a_marked_turn_supplies_no_written_idea_and_an_unmarked_one_is_written() {
    // §14.7: a turn Daniel marked with `#scribe` (inside a sentence, or a
    // line that opens with it) is cataloged at Stop, so the pass skips the
    // whole turn, even its unmarked lines. His unmarked turn is captured.
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let inline = "What if we sharded the audit log by month? #scribe";
    let opened = "What if we dropped the popup preview?";
    let opened_turn = format!("{opened}\n#scribe idea: drop the popup preview");
    let plain = "What if we cached the routing table per host?";
    s.write(
        ON,
        &[
            line("user", "u-inline", inline),
            line("user", "u-opened", &opened_turn),
            line("user", "u-plain", plain),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        idea_claim(
            inline,
            "u-inline",
            "Daniel suggests sharding the audit log by month."
        ),
        idea_claim(
            opened,
            "u-opened",
            "Daniel suggests dropping the popup preview."
        ),
        idea_claim(
            plain,
            "u-plain",
            "Daniel suggests caching the routing table per host."
        ),
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains("- message u-plain (the user's turn): tag idea"),
        "{prompt}"
    );
    assert!(!prompt.contains("- message u-inline:"), "{prompt}");
    assert!(!prompt.contains("- message u-opened:"), "{prompt}");
    let listed = entries_json(&s, &["--tag", "idea"]);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["entries"][0]["quote"], plain);
}

#[test]
fn a_marked_turn_supplies_no_candidate_and_an_unmarked_one_proposes() {
    let s = Setup::new();
    s.config(&support::idea_catcher_on("propose"));
    let marked = "What if we sharded the audit log by month? #scribe/global";
    let plain = "What if we cached the routing table per host?";
    s.write(
        ON,
        &[
            line("user", "u-marked", marked),
            line("user", "u-plain", plain),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": marked, "message": "u-marked", "tag": "idea", "subject": "the audit log",
         "statement": "Daniel suggests sharding the audit log by month."},
        {"quote": plain, "message": "u-plain", "tag": "idea", "subject": "the routing table",
         "statement": "Daniel suggests caching the routing table per host."}
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["proposals"][0]["candidate"]["quote"], plain);
}

#[test]
fn a_claim_whose_turn_fires_no_trigger_of_its_tag_is_refused() {
    // The model names a firing turn under another tag than the one fired.
    let s = Setup::new();
    let quote = "I always keep the release notes short.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "idea"));
    s.scribe("/scribe on");
    s.pass_session(ON);
    assert_eq!(proposals(&s)["total"], 0);
}

#[test]
fn the_trigger_is_checked_again_under_the_lock_against_the_config_then() {
    // The config changes while the model runs: the turn fired `always` when
    // the request was built, but under the lock `preferences` no longer has
    // that trigger, so the claim is refused and nothing is opened.
    let s = Setup::new();
    let quote = "I always keep the release notes short.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.scribe("/scribe on");
    let config = s.fx.root().join("config.toml");
    let changed = config.clone();
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.before_answer.replace(Some(Box::new(move || {
        std::fs::write(
            &changed,
            "[tags.preferences.capture]\ntriggers = [\"from now on\"]\n",
        )
        .unwrap();
        make_private(&changed);
    })));
    s.pass_session(ON);
    assert!(s.prompts()[0].contains("message u-1 (the user's turn): tag preferences"));
    assert!(config.exists(), "the fake model rewrote the config");
    assert_eq!(proposals(&s)["total"], 0);
    assert_eq!(s.record(ON).unwrap()["scribed_through"], "u-1");
}

#[test]
fn pass_proposes_no_candidate_under_task_without_a_config_declaring_it() {
    let s = Setup::new();
    let quote = "The migration still has to be filed as work.";
    s.write(
        ON,
        &[
            line("user", "u-1", "Please plan the migration."),
            line("assistant", "a-work", quote),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([{"quote": quote, "message": "a-work", "tag": "task"}]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    assert_eq!(proposals(&s)["total"], 0, "task is not a shipped tag");
}

#[test]
fn no_candidate_is_opened_from_a_tool_payload() {
    let s = Setup::new();
    let payload_text = "Measured: the cache holds 42 entries after warmup.";
    let tool_line = json!({
        "type": "assistant", "uuid": "a-tool", "timestamp": "2026-11-03T06:00:00Z",
        "cwd": "/nonexistent/work/scribe",
        "message": {"role": "assistant", "content": [
            {"type": "text", "text": "Running the check now."},
            {"type": "tool_use", "name": "Bash", "input": {"command": payload_text}}
        ]}
    });
    s.write(
        ON,
        &[line("user", "u-1", "Please check the cache."), tool_line],
    );
    let mut body = answer();
    body["candidates"] = json!([{
        "quote": payload_text, "message": "a-tool", "tag": "preferences"
    }]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    assert_eq!(proposals(&s)["total"], 0, "tool payloads are not turns");
}

#[test]
fn session_start_output_is_the_same_with_many_findings() {
    let s = Setup::new();
    s.fx.create_private_dir_all(&s.fx.root());
    let config = s.fx.root().join("config.toml");
    std::fs::write(&config, "[tags.finding]\nrequire_evidence = true\n").unwrap();
    make_private(&config);
    let before = s.hook("session-start", &s.payload("SessionStart")).stdout;
    for n in 0..6 {
        s.fx.check(
            Cli(&[
                "entry",
                "add",
                "--tag",
                "finding",
                "--author",
                "opus",
                "--related",
                &format!("session:s-{n}"),
                "--evidence",
                "exp-1",
                &format!("Measured widget result {n} holds under load"),
                "--json",
            ]),
            Success,
        );
    }
    let after = s.hook("session-start", &s.payload("SessionStart")).stdout;
    assert_eq!(before, after, "findings never grow the start line");

    // An intake's pickup refuses a finding: it has no worker.
    let out = s.fx.check(
        Cli(&["entry", "pickup", "finding-1", "--author", "opus", "--json"]),
        Code(1),
    );
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "not_action_tag", "{err}");
}
