#[test]
fn the_pass_writes_daniels_idea_itself_as_an_entry_it_authors_with_no_proposal() {
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let idea = "What if we cached the routing table per host so lookups stay local?";
    let statement = "Daniel suggests caching the routing table per host so lookups stay local.";
    s.write(
        ON,
        &[
            line("user", "u-plain", "Please look at the routing table."),
            line("user", "u-idea", idea),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([idea_claim(idea, "u-idea", statement)]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);

    // The request names the turn under the written tag, and gives the
    // tag's definition and the written-tag rules.
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains(
            "- message u-idea (the user's turn): tag idea (written; trigger \"what if we\", matched \"What if we\")"
        ),
        "{prompt}"
    );
    assert!(!prompt.contains("message u-plain"), "{prompt}");
    assert!(
        prompt.contains("- idea: qualifies: The user proposes something that could be built"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Already recorded under written tags: []"),
        "{prompt}"
    );

    // No proposal: one active entry, the statement as content, his span
    // as quote, authored and stated by the pass, outcome open.
    assert_eq!(proposals(&s)["total"], 0);
    let listed = entries_json(&s, &["--tag", "idea"]);
    assert_eq!(listed["total"], 1, "{listed}");
    let shown: Value = s.entry_json("idea-1");
    assert_eq!(shown["status"], "active");
    assert_eq!(shown["content"], context_of(statement));
    assert_eq!(shown["quote"], idea);
    assert_eq!(shown["said"], "");
    assert_eq!(
        shown["author"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    assert_eq!(
        shown["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    assert_eq!(shown["content_from"]["role"], "user");
    assert_eq!(shown["cite"]["message"], "u-idea");
    assert_eq!(
        shown["outcome"],
        json!({"state": "open", "reason": null, "task": null})
    );

    // Never listed as the user's own, and retired `daniel` fails as a CLI
    // usage error; listed under the pass's channel.
    assert_eq!(
        entries_json(&s, &["--tag", "idea", "--author-kind", "user"])["total"],
        0
    );
    let legacy = s.fx.check(
        Cli(&["entry", "list", "--tag", "idea", "--author-kind", "daniel"]),
        Capture,
    );
    assert_eq!(legacy.status.code(), Some(2), "{}", text(&legacy.stderr));
    assert_eq!(
        entries_json(&s, &["--tag", "idea", "--author-kind", "scribe"])["total"],
        1
    );

    // `entry show` says the pass captured it, in plain and table form.
    let plain = s.fx.check(Cli(&["entry", "show", "idea-1"]), Text);
    assert!(plain.lines().any(|l| l == "outcome\topen\t-"), "{plain}");
    assert!(
        plain
            .lines()
            .any(|l| l == format!("stated_by\tscribe:session-pass\t{idea}")),
        "{plain}"
    );
    let table =
        s.fx.check(Cli(&["entry", "show", "idea-1", "--format", "table"]), Text);
    assert!(
        table.contains("captured by the session pass (not typed by the user)"),
        "{table}"
    );

    // One audit line for the session and one for the idea written.
    let audit = audit_log(&s);
    assert!(
        audit
            .iter()
            .any(|l| l["command"] == "session pass" && l["target"] == "idea-1"),
        "{audit:?}"
    );
}

#[test]
fn an_ordinary_word_or_part_of_an_id_named_as_the_task_writes_the_idea_open() {
    // Each task the model names appears in the session's text, but neither
    // is a task id the session wrote: `task` is an ordinary word, and
    // `ORB-42` is only part of `ORB-4242`. Both ideas are written open.
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let word = "What if we made the stale-session sweep a task for the box?";
    let part = "What if we split the pass prompt? I filed ORB-4242 for that.";
    s.write(ON, &[line("user", "u-1", word), line("user", "u-2", part)]);
    let mut word_claim = idea_claim(word, "u-1", "Daniel suggests a box task for the sweep.");
    word_claim["outcome"] = json!("task");
    word_claim["task"] = json!("task");
    let mut part_claim = idea_claim(part, "u-2", "Daniel suggests splitting the pass prompt.");
    part_claim["outcome"] = json!("task");
    part_claim["task"] = json!("ORB-42");
    let mut body = answer();
    body["candidates"] = json!([word_claim, part_claim]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let listed = entries_json(&s, &["--tag", "idea"]);
    let entries = listed["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{listed}");
    for (entry, quote) in entries.iter().zip([word, part]) {
        assert_eq!(entry["quote"], quote);
        assert_eq!(
            entry["outcome"],
            json!({"state": "open", "reason": null, "task": null})
        );
    }
}

#[test]
fn a_parked_idea_keeps_daniels_words_and_one_that_became_a_task_writes_nothing() {
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let sharding = "What if we sharded the audit log by month?";
    let dry_run = "Here's an idea: a dry-run flag for the session pass.";
    let popup = "What if we dropped the popup preview entirely?";
    let parked = "let's park the sharding idea until the log is big";
    s.write(
        ON,
        &[
            line("user", "u-1", sharding),
            line("user", "u-2", dry_run),
            line("assistant", "a-1", "Filed ORB-4242 for the dry-run flag."),
            line("user", "u-3", &format!("ok, {parked}.")),
            line("user", "u-4", popup),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": sharding, "message": "u-1", "tag": "idea", "subject": "the audit log",
         "statement": "Daniel suggests sharding the audit log by month.",
         "outcome": "parked", "outcome_said": parked, "outcome_message": "u-3"},
        {"quote": dry_run, "message": "u-2", "tag": "idea", "subject": "session pass",
         "statement": "Daniel suggests a dry-run flag for the session pass.",
         "outcome": "task", "task": "ORB-4242"},
        // Parking words that are not his, verbatim: written open.
        {"quote": popup, "message": "u-4", "tag": "idea", "subject": "the tmux popup",
         "statement": "Daniel suggests dropping the tmux popup's preview.",
         "outcome": "parked", "outcome_said": "drop it for now", "outcome_message": "u-4"}
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let listed = entries_json(&s, &["--tag", "idea"]);
    let entries = listed["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "the task became a task: {listed}");
    assert_eq!(entries[0]["quote"], sharding);
    assert_eq!(
        entries[0]["outcome"],
        json!({"state": "parked", "reason": parked, "task": null})
    );
    assert_eq!(entries[1]["quote"], popup);
    assert_eq!(entries[1]["outcome"]["state"], "open");
    assert!(
        entries
            .iter()
            .all(|e| e["quote"] != dry_run && e["content"] != dry_run)
    );
    let plain = s.fx.check(Cli(&["entry", "show", "idea-1"]), Text);
    assert!(
        plain
            .lines()
            .any(|l| l == format!("outcome\tparked\t{parked}")),
        "{plain}"
    );

    // Forgetting it blanks his parking words with the rest of his words.
    let out = type_settlement(&s, "#scribe forget idea-1");
    assert_eq!(out.status.code(), Some(0));
    let forgotten: Value = s.entry_json("idea-1");
    assert_eq!(forgotten["status"], "forgotten");
    assert_eq!(
        forgotten["outcome"],
        json!({"state": "parked", "reason": null, "task": null})
    );
    assert_eq!(forgotten["quote"], Value::Null);
}

#[test]
fn an_assistant_turn_never_produces_an_idea_entry() {
    let s = Setup::new();
    let suggestion = "What if we cached the routing table per host instead?";
    s.write(
        ON,
        &[
            line("user", "u-1", "What if we looked at the routing next?"),
            line("assistant", "a-1", suggestion),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        idea_claim(
            suggestion,
            "a-1",
            "The assistant suggests a per-host cache."
        ),
        // Named under Daniel's firing turn, but the words are the assistant's.
        idea_claim(
            suggestion,
            "u-1",
            "The assistant suggests a per-host cache."
        )
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let prompt = &s.prompts()[0];
    assert!(
        !prompt.contains("message a-1 (the user's turn): tag"),
        "{prompt}"
    );
    assert_eq!(entries_json(&s, &["--tag", "idea"])["total"], 0);
    assert_eq!(proposals(&s)["total"], 0);
}

#[test]
fn a_fourth_idea_in_one_pass_is_not_written_and_proposals_keep_their_own_slots() {
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let ideas = [
        "What if we cached the routing table per host?",
        "What if we sharded the audit log by month?",
        "What if we added a dry-run flag to the pass?",
        "What if we dropped the popup preview entirely?",
    ];
    let preference = "I prefer short progress updates while an agent is working.";
    let mut lines: Vec<Value> = ideas
        .iter()
        .enumerate()
        .map(|(n, idea)| line("user", &format!("u-{n}"), idea))
        .collect();
    lines.push(line("user", "u-pref", preference));
    s.write(ON, &lines);
    let mut claims: Vec<Value> = ideas
        .iter()
        .enumerate()
        .map(|(n, idea)| {
            idea_claim(
                idea,
                &format!("u-{n}"),
                &format!(
                    "Daniel suggests: {}",
                    idea.trim_start_matches("What if we ")
                ),
            )
        })
        .collect();
    claims.push(json!({"quote": preference, "message": "u-pref", "tag": "preferences"}));
    let mut body = answer();
    body["candidates"] = json!(claims);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains(
            "at most 3 in this window under proposed tags and at most 3 under written tags"
        ),
        "{prompt}"
    );
    let listed = entries_json(&s, &["--tag", "idea"]);
    let quotes: Vec<&str> = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["quote"].as_str().unwrap())
        .collect();
    assert_eq!(quotes, ideas[..3].to_vec());
    // The preference still opens its candidate: its slots are its own.
    let open = proposals(&s);
    assert_eq!(open["total"], 1, "{open}");
    assert_eq!(open["proposals"][0]["candidate"]["tag"], "preferences");
}

#[test]
fn ideas_an_earlier_window_drafted_but_did_not_write_leave_later_windows_their_slots() {
    // One message per window. In each of the first three, Daniel's idea
    // became a task in his own turn, so the drafted idea writes nothing;
    // the fourth window's idea must still be requested and written, since
    // only ideas the store wrote spend the pass's three write slots.
    let s = Setup::new();
    let filed = [
        "What if we sharded the audit log by month? I filed ORB-4241 for it.",
        "What if we added a dry-run flag to the pass? I filed ORB-4242 for it.",
        "What if we dropped the popup preview? I filed ORB-4243 for it.",
    ];
    let open = "What if we cached the routing table per host?";
    let mut lines: Vec<Value> = filed
        .iter()
        .enumerate()
        .map(|(n, idea)| line("user", &format!("u-{n}"), idea))
        .collect();
    lines.push(line("user", "u-open", open));
    s.write(ON, &lines);
    let mut answers: Vec<Option<Value>> = filed
        .iter()
        .enumerate()
        .map(|(n, idea)| {
            let mut claim = idea_claim(
                idea,
                &format!("u-{n}"),
                &format!("Daniel suggests idea {n}, now a task."),
            );
            claim["outcome"] = json!("task");
            claim["task"] = json!(format!("ORB-424{}", n + 1));
            let mut body = answer();
            body["candidates"] = json!([claim]);
            Some(body)
        })
        .collect();
    let mut last = answer();
    last["candidates"] = json!([idea_claim(
        open,
        "u-open",
        "Daniel suggests caching the routing table per host."
    )]);
    answers.push(Some(last));
    s.answers(&answers);
    s.scribe("/scribe on");
    s.config(&format!(
        "[catalog]\nwindow_messages = 1\n{}",
        support::IDEA_CATCHER
    ));
    s.pass_session(ON);
    let prompts = s.prompts();
    assert_eq!(prompts.len(), 4);
    assert!(
        prompts[3].contains("at most 3 under written tags"),
        "{}",
        prompts[3]
    );
    let listed = entries_json(&s, &["--tag", "idea"]);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["entries"][0]["quote"], open);
}

#[test]
fn an_idea_an_active_entry_already_covers_is_not_written_and_ideas_are_sent_as_exclusions() {
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let idea = "What if we cached the routing table per host so lookups stay local?";
    let statement = "Daniel suggests caching the routing table per host so lookups stay local.";
    s.write(ON, &[line("user", "u-1", idea)]);
    let mut body = answer();
    body["candidates"] = json!([idea_claim(idea, "u-1", statement)]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    assert_eq!(entries_json(&s, &["--tag", "idea"])["total"], 1);

    // Later in the session he floats it again: once in the same words, once
    // paraphrased with the statement the model gives the same.
    let again =
        "What if we cached the routing table per host so lookups stay local? Still worth it.";
    let paraphrase = "What if we kept a per-host copy of the routing table?";
    s.append(
        ON,
        &[line("user", "u-2", again), line("user", "u-3", paraphrase)],
    );
    let mut body = answer();
    body["candidates"] = json!([
        idea_claim(
            idea,
            "u-2",
            "Daniel again suggests a per-host routing table cache."
        ),
        idea_claim(paraphrase, "u-3", statement)
    ]);
    s.answer(&body);
    s.pass_session(ON);
    assert_eq!(
        entries_json(&s, &["--tag", "idea"])["total"],
        1,
        "the quote and the context are each covered by idea-1"
    );
    // The existing idea went to the model as an exclusion, context and
    // his words.
    let prompts = s.prompts();
    let second = prompts.last().unwrap();
    let excluded = format!(
        "Already recorded under written tags: [\"{} (the user's words: \\\"{idea}\\\")\"]",
        context_of(statement)
    );
    assert!(second.contains(&excluded), "{second}");
}

#[test]
fn the_session_start_stuck_line_does_not_count_pass_captured_ideas() {
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let idea = "What if we cached the routing table per host so lookups stay local?";
    s.write(ON, &[line("user", "u-1", idea)]);
    let mut body = answer();
    body["candidates"] = json!([idea_claim(
        idea,
        "u-1",
        "Daniel suggests caching the routing table per host."
    )]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    // Older than the stuck threshold, and nobody picked it up.
    support::sql(
        &s.fx.root(),
        "UPDATE entries SET created = '2026-01-01T00:00:00Z' WHERE id = 'idea-1';",
    );
    let mut payload = s.payload("SessionStart");
    payload["source"] = json!("startup");
    let out = s.hook("session-start", &payload);
    assert!(
        !text(&out.stdout).contains("not picked up"),
        "{}",
        text(&out.stdout)
    );
}

// --- memory files (2_design.md §15.4–§15.6, §15.11) -----------------------
