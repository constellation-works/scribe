#[test]
fn stop_never_replays_historical_settling_commands() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_verbatim(
        &fx,
        1,
        "#scribe preferences: keep this entry",
        "an older prompt",
        1,
        Some("u-0"),
    );
    claude.write_transcript("#scribe forget preferences-1", "u-settle");
    let before = entries_snapshot(&fx);
    let audit_before = audit(&fx);
    let path = fake_claude(&fx, "exit 1");
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(
        claude_calls(&fx).is_empty(),
        "a settling line is not a marker"
    );
    assert_eq!(
        entries_snapshot(&fx),
        before,
        "history cannot reapply a destructive instruction"
    );
    assert_eq!(audit(&fx), audit_before);
}

#[test]
fn the_stop_hook_with_nothing_waiting_touches_nothing() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    claude.write_transcript("no scribe here", "u-1");
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert!(out.stdout.is_empty());
    assert!(!fx.root().exists());
}

#[test]
fn the_stop_hook_leaves_a_cite_waiting_until_its_prompt_is_in_the_transcript() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let payload = claude.payload("user-prompt-submit.json");
    write_fixture_entries(&fx, payload["prompt"].as_str().unwrap());
    claude.write_transcript("a different prompt", "u-other");
    hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let entries = entries(&fx);
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert_eq!(entry["cite"]["message"], Value::Null);
    }
}

// --- a fake `claude` on PATH ----------------------------------------------

#[test]
fn a_line_start_form_gives_the_agent_its_tag_as_the_hint() {
    let fx = Fixture::new();
    fx.config(support::IDEA_CATCHER);
    let claude = Claude::new(&fx);
    let typed =
        "#scribe idea: a dry-run flag for prune\n#scribe add this to preferences #merge-policy";
    marked_turns(
        &claude,
        &[("u-0", "We should squash-merge agent PRs."), ("u-1", typed)],
    );
    let stop = claude.payload("stop.json");
    let out = hook_ok(&fx, "stop", &stop);
    let reply_json = reply(&out);
    assert_eq!(
        reply_json["systemMessage"],
        "scribe: #scribe in your message is the agent's to record — \"#scribe idea: a dry-run flag for prune\"\n\
         scribe: #scribe in your message is the agent's to record — \"#scribe add this to preferences #merge-policy\""
    );
    let context = reply_json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let lines: Vec<&str> = context.lines().collect();
    assert_eq!(lines.len(), 2, "{context}");
    assert!(
        lines[0].contains("scribe entry add --tag idea --project scribe --author <name>"),
        "{context}"
    );
    assert!(
        lines[1].contains(
            "scribe entry add --tag preferences --project scribe --topic merge-policy --author <name>"
        ),
        "{context}"
    );
    assert!(
        lines
            .iter()
            .all(|l| l.contains(&format!("--related session:{SESSION}"))),
        "{context}"
    );
    assert!(entries(&fx).is_empty());
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(codes, vec![json!("agent_handoff"); 2]);

    // The next Stop neither hands over nor echoes.
    let out = hook_ok(&fx, "stop", &stop);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert_eq!(marks_json(&fx).len(), 2);
}

#[test]
fn an_evidence_tag_handoff_gives_the_evidence_pointer_its_command_needs() {
    let fx = Fixture::new();
    fx.config("[tags.finding]\nrequire_evidence = true\n");
    let claude = Claude::new(&fx);
    let typed = "\
#scribe finding: the reviewer mix alone predicts a 7-point gap
#scribe preferences: keep merge commits for releases";
    marked_turns(
        &claude,
        &[("u-0", "We compared the reviewer mixes."), ("u-1", typed)],
    );
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let context = reply(&out)["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned();
    let lines: Vec<&str> = context.lines().collect();
    assert_eq!(lines.len(), 2, "{context}");

    // The evidence tag's command gives the pointer `entry add` requires
    // (evidence_required) and neither --quote nor --spoken, which it refuses
    // under a finding: the entry is the agent's own result.
    let finding = lines[0];
    assert!(
        finding.contains(
            "scribe entry add --tag finding --project scribe --author <name> \
             --related session:"
        ),
        "{context}"
    );
    assert!(
        finding.contains("--evidence '<pointer>' '<statement>'"),
        "{context}"
    );
    assert!(!finding.contains("--quote"), "{context}");
    assert!(!finding.contains("--spoken"), "{context}");

    // An ordinary memory tag keeps its existing flags: --spoken, no pointer.
    let preference = lines[1];
    assert!(
        preference.contains("scribe entry add --tag preferences --project scribe"),
        "{context}"
    );
    assert!(preference.contains("--spoken '<statement>'"), "{context}");
    assert!(!preference.contains("--evidence"), "{context}");

    // The command with its placeholders filled in passes the finding checks.
    fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "finding",
            "--project",
            "scribe",
            "--author",
            "opus",
            "--related",
            &format!("session:{SESSION}"),
            "--evidence",
            "exp-1",
            "The reviewer mix alone predicts a 7-point gap",
        ]),
        Success,
    );
    let recorded = entries(&fx);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0]["tag"], "finding");
}

#[test]
fn a_quote_and_evidence_tag_handoff_gives_both_required_values() {
    let fx = Fixture::new();
    fx.config("[tags.finding]\nrequire_quote = true\nrequire_evidence = true\n");
    let claude = Claude::new(&fx);
    let quote = "the reviewer mix alone predicts a 7-point gap";
    marked_turns(&claude, &[("u-0", &format!("#scribe finding: {quote}"))]);

    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let context = reply(&out)["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        context.contains(
            "scribe entry add --tag finding --project scribe --author <name> \
             --related session:"
        ),
        "{context}"
    );
    assert!(
        context.contains("--quote '<their words>' --evidence '<pointer>' '<statement>'"),
        "{context}"
    );
    assert!(!context.contains("--spoken"), "{context}");

    // Filling both placeholders with the user's verbatim words and a pointer
    // passes the entry checks for this non-action tag.
    fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "finding",
            "--project",
            "scribe",
            "--author",
            "opus",
            "--related",
            &format!("session:{SESSION}"),
            "--quote",
            quote,
            "--evidence",
            "exp-1",
            "The reviewer mix alone predicts a 7-point gap",
        ]),
        Success,
    );
    let recorded = entries(&fx);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0]["tag"], "finding");
    assert_eq!(recorded[0]["quote"], quote);
}

#[test]
fn store_tag_capture_and_daniel_approval_of_agent_proposal() {
    let fx = Fixture::new();
    fx.check(
        Cli(&[
            "tag",
            "add",
            "notes",
            "--author",
            "opus",
            "--description",
            "Agent notes",
            "--alias",
            "note",
        ]),
        Success,
    );
    let claude = Claude::new(&fx);
    let typed = "#scribe keep this note to note";
    marked_turns(&claude, &[("u-note", typed)]);
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let context = reply(&out)["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        context.contains("scribe entry add --tag notes --project scribe"),
        "an alias at the line's end is the hint: {context}"
    );
    fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "notes",
            "--author",
            "opus",
            "--related",
            &format!("session:{SESSION}"),
            "--quote",
            "keep this note",
            "--spoken",
            "Daniel wants this note kept.",
        ]),
        Success,
    );
    let recorded = entries(&fx);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["tag"], "notes");
    assert_eq!(recorded[0]["quote"], "keep this note");
    fx.check(
        Cli(&[
            "proposal",
            "add",
            "--kind",
            "stale",
            "--older",
            "notes-1",
            "--reason",
            "Review this",
            "--author",
            "opus",
        ]),
        Success,
    );
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe approve p-1");
    claude.write_transcript("#scribe approve p-1", "u-approve");
    hook_ok(&fx, "user-prompt-submit", &payload);
    assert_eq!(entries(&fx)[0]["status"], "retired");
    let proposals: Value = fx.check(Cli(&["proposal", "list", "--json"]), Json);
    assert_eq!(proposals["proposals"][0]["settled_author"]["kind"], "user");
}

#[test]
fn each_marker_hands_over_global_or_the_cwd_project() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let typed = "\
#scribe/global preferences: squash-merge agent PRs
#scribe preferences: keep merge commits for releases
#scribe/global add this to preferences";
    marked_turns(
        &claude,
        &[("u-0", "I want short commit subjects."), ("u-1", typed)],
    );
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let reply_json = reply(&out);
    let message = reply_json["systemMessage"].as_str().unwrap().to_owned();
    assert_eq!(
        message
            .matches("scribe: #scribe in your message is the agent's to record")
            .count(),
        3,
        "{message}"
    );
    let context = reply_json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let lines: Vec<&str> = context.lines().collect();
    assert_eq!(lines.len(), 3, "{context}");
    let expect = [
        (
            "#scribe/global preferences: squash-merge agent PRs",
            "global",
        ),
        (
            "#scribe preferences: keep merge commits for releases",
            "scribe",
        ),
        ("#scribe/global add this to preferences", "global"),
    ];
    for (line, (said, project)) in lines.iter().zip(expect) {
        assert!(line.contains(&format!("\"{said}\"")), "{context}");
        assert!(
            line.contains(&format!(
                "scribe entry add --tag preferences --project {project} --author <name>"
            )),
            "{context}"
        );
    }
    assert!(entries(&fx).is_empty(), "{:?}", entries(&fx));
}

#[test]
fn a_malformed_opener_or_a_global_settling_line_is_refused_and_teaches_the_opener() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe/all preferences: hello");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert_eq!(message.lines().count(), 1, "{message}");
    let reason = message
        .split(" — ")
        .nth(1)
        .unwrap_or_else(|| panic!("no reason: {message}"));
    assert!(reason.contains("#scribe/global"), "{reason}");
    assert!(
        !fx.root().exists(),
        "a refused marker writes nothing at submit"
    );

    write_verbatim(
        &fx,
        1,
        "#scribe preferences: keep settling unmarked",
        "an older prompt",
        1,
        Some("u-0"),
    );
    propose_stale(&fx);
    payload["prompt"] = json!("#scribe/global approve p-1");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    let reason = message.split(" — ").nth(1).expect("reason");
    assert!(reason.contains("#scribe/global"), "{message}");
    assert_eq!(entries(&fx)[0]["status"], "active");
    assert!(listed(&fx, "global").is_empty());
}

#[test]
fn session_start_suggests_a_single_record_and_names_both_when_ambiguous() {
    let fx = Fixture::new();
    let cwd = project_cwd(&fx);
    let now_dt = time::OffsetDateTime::now_utc();
    let earlier_dt = now_dt - time::Duration::hours(1);
    let format = &time::format_description::well_known::Rfc3339;
    let now = now_dt.format(format).unwrap();
    let earlier = earlier_dt.format(format).unwrap();
    write_session(&fx, PREVIOUS, "scribe", &earlier, &earlier, true);
    let alone = hook_ok(&fx, "session-start", &start_payload(Some(&cwd), CURRENT));
    let alone_reply = reply(&alone);
    assert_eq!(
        alone_reply["systemMessage"].as_str(),
        Some(continuation_line(PREVIOUS).as_str())
    );
    assert!(
        alone_reply.get("hookSpecificOutput").is_none(),
        "the agent is not told: {alone_reply}"
    );

    write_session(&fx, NEWER, "scribe", &now, &now, true);
    write_open_proposal(&fx);
    let before = snapshot(&fx.root());
    let message = start_message(&fx, &start_payload(Some(&cwd), CURRENT));
    assert_eq!(
        message,
        format!(
            "scribe: 1 proposal to review — scribe proposal list\nscribe: possible recorded predecessors {PREVIOUS}, {NEWER} — type /scribe on --from <id> to choose one"
        )
    );
    let out = hook_ok(&fx, "session-start", &start_payload(Some(&cwd), CURRENT));
    let parsed = reply(&out);
    assert!(
        parsed.get("hookSpecificOutput").is_none(),
        "the agent is not told: {parsed}"
    );
    assert!(audit(&fx).is_empty(), "a reminder is not audited");
    assert_eq!(
        snapshot(&fx.root()),
        before,
        "session-start wrote the store"
    );
}
