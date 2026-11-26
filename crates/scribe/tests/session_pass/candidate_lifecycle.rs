#[test]
fn candidate_sources_and_existing_statements_are_checked_at_the_binary_boundary() {
    let quote = "I prefer concise messages during lengthy builds.";
    let cases = [
        ("assistant", quote.to_owned(), "u-1", "preferences", false),
        (
            "user",
            format!("Read this: <pasted_content>{quote}</pasted_content>"),
            "u-1",
            "preferences",
            false,
        ),
        (
            "user",
            format!("```text\n{quote}\n```"),
            "u-1",
            "preferences",
            false,
        ),
        ("user", format!("> {quote}"), "u-1", "preferences", false),
        (
            "user",
            quote.to_owned(),
            "missing-turn",
            "preferences",
            false,
        ),
        (
            "user",
            "I never said the model's proposed sentence.".to_owned(),
            "u-1",
            "preferences",
            false,
        ),
        ("user", quote.to_owned(), "u-1", "task", false),
        (
            "user",
            format!("#scribe preferences: {quote}"),
            "u-1",
            "preferences",
            false,
        ),
        ("user", quote.to_owned(), "u-1", "preferences", true),
    ];
    for (role, text, message, tag, existing) in cases {
        let s = Setup::new();
        s.write(ON, &[line(role, "u-1", &text)]);
        if existing {
            support::seed_records(&s.fx.root(), &[&support::entry_text(1, quote)]);
        }
        s.answer(&candidate_answer(quote, message, tag));
        s.scribe("/scribe on");
        s.pass_session(ON);
        assert_eq!(
            proposals(&s)["total"],
            0,
            "unsafe source or duplicate: {role} {text}"
        );
    }
}

#[test]
fn rejected_candidates_are_never_repeated_and_are_counted_at_session_start() {
    let s = Setup::new();
    let quote = "I prefer concise messages during lengthy builds.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let start = s.hook("session-start", &s.payload("SessionStart"));
    assert!(text(&start.stdout).contains("1 proposal to review"));
    type_settlement(&s, "#scribe reject p-1");
    type_settlement(&s, "#scribe reject p-1");
    // New turn and punctuation/case changes still name the same statement.
    let repeated = "I prefer concise messages during lengthy builds!";
    s.append(ON, &[line("user", "u-2", repeated)]);
    s.answer(&candidate_answer(repeated, "u-2", "preferences"));
    s.run(&["session", "pass", "--session", ON], b"");
    let ps = proposals(&s);
    assert_eq!(ps["total"], 1);
    assert_eq!(ps["proposals"][0]["state"], "rejected");
    let es: Value = s.fx.check(Cli(&["entry", "list", "--json"]), Json);
    assert_eq!(es["total"], 0);
    assert_eq!(
        s.hook("session-start", &s.payload("SessionStart")).stdout,
        b""
    );
}

#[test]
fn candidates_share_a_three_slot_budget_across_windows_and_keep_cataloging() {
    let s = Setup::new();
    let lines: Vec<_> = (0..5)
        .map(|n| {
            line(
                "user",
                &format!("u-{n}"),
                &format!("I always choose option {n}."),
            )
        })
        .collect();
    s.write(ON, &lines);
    let answers: Vec<_> = (0..5)
        .map(|n| {
            Some(candidate_answer(
                &format!("I always choose option {n}."),
                &format!("u-{n}"),
                "preferences",
            ))
        })
        .collect();
    s.answers(&answers);
    s.scribe("/scribe on");
    let config = s.fx.root().join("config.toml");
    std::fs::write(&config, "[catalog]\nwindow_messages = 1\n").unwrap();
    make_private(&config);
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 3);
    assert_eq!(s.record(ON).unwrap()["passes"], 5);
    assert!(!s.control()["due"].as_bool().unwrap());
}

#[test]
fn candidate_open_cap_is_enforced_even_when_the_model_ignores_it() {
    let s = Setup::new();
    // The store-wide cap, not the per-tag one.
    s.config("[tags.preferences.capture]\nmax_open = 20\n");
    s.write(ON, &[line("user", "u-1", "I always choose option zero.")]);
    s.answer(&candidate_answer(
        "I always choose option zero.",
        "u-1",
        "preferences",
    ));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let template = proposals(&s)["proposals"][0].clone();
    let full: Vec<_> = (1..=19)
        .map(|n| {
            let mut p = template.clone();
            p["id"] = json!(format!("p-{n}"));
            p["older"] = json!(format!("preferences-{n}"));
            p["candidate"]["quote"] = json!(format!("Stored preference {n}."));
            p["candidate"]["statement"] = json!(format!("Stored preference {n}."));
            // A format-14 claim has no `context` (format 24).
            p["candidate"].as_object_mut().unwrap().remove("context");
            p
        })
        .collect();
    // The stored claims replace the pass's own: p-1 .. p-19.
    support::sql(&s.fx.root(), "DELETE FROM claims;");
    seed_claims(&s.fx.root(), &full);
    let quotes = [
        "I prefer faster incremental compilation.",
        "I always choose a blue interface.",
        "Always retain descriptive error messages.",
    ];
    let turns: Vec<_> = quotes
        .iter()
        .enumerate()
        .map(|(n, quote)| line("user", &format!("cap-{n}"), quote))
        .collect();
    s.append(ON, &turns);
    let mut body = answer();
    body["candidates"] = json!(quotes.iter().enumerate().map(|(n, quote)| json!({"quote":quote, "message":format!("cap-{n}"), "tag":"preferences"})).collect::<Vec<_>>());
    s.answer(&body);
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 20);
    s.append(
        ON,
        &[line("user", "u-2", "I prefer detailed release notes.")],
    );
    s.answer(&candidate_answer(
        "I prefer detailed release notes.",
        "u-2",
        "preferences",
    ));
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 20);
    assert_eq!(s.record(ON).unwrap()["scribed_through"], "u-2");
}

#[test]
fn an_instructed_rejection_keeps_the_claim_from_being_proposed_again() {
    let s = Setup::new();
    let quote = "I always choose smaller pull requests for easier review.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 1);
    // Daniel's instruction, typed in the aide's interactive session after
    // the pass opened the claim (2_design.md §12.2).
    let aide = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61";
    let words = "reject p-1, it was a one-off";
    let at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let mut turn = line("user", "aide-1", words);
    turn["timestamp"] = json!(at);
    turn["entrypoint"] = json!("claude-desktop");
    s.write(aide, &[turn]);
    let rejected: Value = s.fx.check(
        Cli(&[
            "proposal",
            "settle",
            "p-1",
            "--reject",
            "--author",
            "opus",
            "--session",
            aide,
            "--instruction",
            words,
            "--json",
        ]),
        Json,
    );
    assert_eq!(rejected["state"], "rejected");
    assert_eq!(rejected["closed_reason"], "instructed");
    assert_eq!(s.entry_count(), 0, "no entry is written");
    // He says it again; the next pass finds the claim and proposes nothing.
    s.append(ON, &[line("user", "u-2", quote)]);
    s.answer(&candidate_answer(quote, "u-2", "preferences"));
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(s.record(ON).unwrap()["scribed_through"], "u-2");
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["proposals"][0]["closed_reason"], "instructed");
}

#[test]
fn candidate_ids_are_reserved_for_approval_and_agent_settlement_is_refused() {
    let s = Setup::new();
    let quote = "I always choose smaller pull requests for easier review.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let denied = s.fx.check(
        Cli(&[
            "proposal",
            "settle",
            "p-1",
            "--approve",
            "--author",
            "agent",
            "--json",
        ]),
        Code(1),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&denied.stderr).unwrap()["code"],
        "user_only"
    );
    let direct: Value = s.fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "agent",
            "Agent analysis with a different content.",
            "--json",
        ]),
        Json,
    );
    assert_eq!(direct["id"], "preferences-2");
    // A line he marks is handed to the agent at Stop, and the entry the
    // agent records for it too skips the id the candidate reserved.
    let id = mark(
        &s,
        "u-2",
        "A separate decision Daniel explicitly marks. #scribe preferences",
        "preferences",
        "Daniel marks a separate decision.",
        "A separate decision Daniel explicitly marks.",
    );
    assert_eq!(id, "preferences-3");
    assert!(support::exported(&s.fx.root(), "entries/preferences/preferences-3.md").is_some());
    type_settlement(&s, "#scribe approve p-1");
    let entry: Value = s.entry_json("preferences-1");
    assert_eq!(entry["content"], context_of(quote));
    type_settlement(&s, "#scribe forget preferences-1");
    assert_eq!(proposals(&s)["proposals"][0]["candidate"]["quote"], "");
    type_settlement(&s, "#scribe approve p-1");
    let forgotten: Value = s.entry_json("preferences-1");
    assert_eq!(forgotten["status"], "forgotten");
    assert_eq!(forgotten["content"], "");
}

#[test]
fn interrupted_candidate_approval_is_replayed_once_from_the_durable_proposal() {
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    // Approved journal durable; entry file not yet written at the crash point.
    approve_claim_in_db(&s.fx.root(), 1);
    s.run(&["consistency", "scan", "--json"], b"");
    s.run(&["consistency", "scan", "--json"], b"");
    let es: Value = s.fx.check(Cli(&["entry", "list", "--json"]), Json);
    assert_eq!(es["total"], 1);
    let entry: Value = s.entry_json("preferences-1");
    assert_eq!(entry["content"], context_of(quote));
    assert_eq!(entry["author"]["kind"], "user");
}

#[test]
fn approving_a_candidate_covered_by_a_later_entry_writes_no_second_entry() {
    let s = Setup::new();
    let quote = "I prefer concise commit subjects on every change.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["proposals"][0]["older"], "preferences-1");
    let added: Value = s.fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--project",
            "scribe",
            "--author",
            "opus",
            quote,
            "--json",
        ]),
        Json,
    );
    assert_eq!(added["id"], "preferences-2");
    let out = type_settlement(&s, "#scribe approve p-1");
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        reply["systemMessage"],
        "scribe: approved p-1 — already covered by preferences-2; no new entry"
    );
    assert!(support::exported(&s.fx.root(), "entries/preferences/preferences-1.md").is_none());
    let listed: Value = s.fx.check(Cli(&["entry", "list", "--json"]), Json);
    assert_eq!(listed["total"], 1);
    let proposal = &proposals(&s)["proposals"][0];
    assert_eq!(proposal["state"], "rejected");
    assert!(proposal["newer"].is_null());
    let replay = type_settlement(&s, "#scribe approve p-1");
    let again: Value = serde_json::from_slice(&replay.stdout).unwrap();
    let message = again["systemMessage"].as_str().unwrap();
    assert!(message.contains("refused"), "{message}");
    assert!(message.contains("already rejected"), "{message}");
    assert!(!message.contains("preferences-2"), "{message}");
}

#[test]
fn a_longer_refinement_of_a_rejected_candidate_still_opens() {
    let s = Setup::new();
    let quote = "I prefer concise messages during lengthy builds";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    type_settlement(&s, "#scribe reject p-1");
    let refined = "I prefer concise messages during lengthy builds in every repository";
    s.append(ON, &[line("user", "u-2", refined)]);
    s.answer(&candidate_answer(refined, "u-2", "preferences"));
    s.run(&["session", "pass", "--session", ON], b"");
    let ps = proposals(&s);
    assert_eq!(ps["total"], 2, "{ps}");
    assert_eq!(ps["proposals"][0]["state"], "rejected");
    assert_eq!(ps["proposals"][1]["state"], "open");
    assert_eq!(ps["proposals"][1]["candidate"]["quote"], refined);
}

#[test]
fn consistency_scan_reports_an_approved_candidate_closed_as_covered() {
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let added: Value = s.fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--project",
            "scribe",
            "--author",
            "opus",
            quote,
            "--json",
        ]),
        Json,
    );
    assert_eq!(added["id"], "preferences-2");
    approve_claim_in_db(&s.fx.root(), 1);
    let first = s.run(&["consistency", "scan", "--json"], b"");
    let report: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(report["covered"][0]["proposal"], "p-1");
    assert_eq!(report["covered"][0]["by"], "preferences-2");
    assert!(report["reapplied"].as_array().unwrap().is_empty());
    assert!(support::exported(&s.fx.root(), "entries/preferences/preferences-1.md").is_none());
    let settled = &proposals(&s)["proposals"][0];
    assert_eq!(settled["state"], "rejected");
    assert!(settled["newer"].is_null());
    let listed: Value = s.fx.check(Cli(&["entry", "list", "--json"]), Json);
    assert_eq!(listed["total"], 1);
    let audit = support::audit_text(&s.fx.root());
    let covered_lines = audit
        .lines()
        .filter(|line| {
            line.contains("\"command\":\"consistency scan\"")
                && line.contains("\"target\":\"p-1\"")
                && line.contains("\"status\":\"success\"")
        })
        .count();
    assert_eq!(covered_lines, 1, "{audit}");
    s.run(&["consistency", "scan", "--json"], b"");
    let again = support::audit_text(&s.fx.root());
    let covered_again = again
        .lines()
        .filter(|line| {
            line.contains("\"command\":\"consistency scan\"")
                && line.contains("\"target\":\"p-1\"")
                && line.contains("\"status\":\"success\"")
        })
        .count();
    assert_eq!(covered_again, 1, "{again}");
    assert_eq!(
        s.fx.check(Cli(&["entry", "list", "--json"]), Json)["total"],
        1
    );
}

#[test]
fn pass_closes_retired_tag_candidates_before_cap_and_suppresses_their_quotes() {
    let s = Setup::new();
    s.scribe("/scribe on");
    let cite = |msg: &str| {
        json!({
            "session": ON,
            "message": msg,
            "at": "2026-11-15T00:00:00Z",
            "prompt_sha256": "0".repeat(64),
            "line": 1,
            "cwd": "/nonexistent/work/scribe"
        })
    };
    let mut initial = Vec::new();
    let tags = [
        ("decisions", 7),
        ("task", 7),
        ("finding", 2),
        ("preferences", 2),
        ("standard", 2),
    ];
    let mut n = 0usize;
    let mut retired_counts: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for (tag, count) in tags {
        for i in 1..=count {
            n += 1;
            let quote = if n == 1 {
                String::from("I always prefer retired rule one for builds.")
            } else {
                format!("I always keep {tag} item {i} in the store.")
            };
            let is_active = matches!(tag, "preferences" | "standard");
            if !is_active {
                *retired_counts.entry(tag).or_default() += 1;
            }
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
    assert_eq!(initial.len(), 20);
    assert_eq!(retired_counts.values().sum::<usize>(), 16);
    seed_claims(&s.fx.root(), &initial);

    let new_quote = "I prefer concise summaries in every session pass.";
    s.write(ON, &[line("user", "u-new", new_quote)]);
    s.answer(&candidate_answer(new_quote, "u-new", "preferences"));
    let audit_before = s.audit_lines();
    s.pass_session(ON);

    let listed = proposals(&s);
    assert_eq!(listed["total"], 21, "{listed}");
    let list = listed["proposals"].as_array().unwrap();
    for p in &list[..16] {
        assert_eq!(p["state"], "rejected", "{p}");
        assert_eq!(p["closed_reason"], "tag_retired", "{p}");
        assert_eq!(
            p["settled_author"],
            json!({"kind": "scribe", "name": "tag-retired"}),
            "{p}"
        );
        assert!(p["settled_by"].is_null(), "{p}");
        assert!(!p["settled_at"].is_null(), "{p}");
    }
    for p in &list[16..20] {
        assert_eq!(p["state"], "open", "{p}");
        assert!(p["closed_reason"].is_null(), "{p}");
        assert!(p["settled_author"].is_null(), "{p}");
    }
    assert_eq!(list[20]["id"], "p-21");
    assert_eq!(list[20]["older"], "preferences-3");
    assert_eq!(list[20]["state"], "open");
    assert_eq!(list[20]["candidate"]["quote"], new_quote);
    assert_eq!(s.entry_count(), 0, "pass writes no entries");

    let new_audit: Vec<Value> = support::audit_lines(&s.fx.root())
        .into_iter()
        .skip(audit_before)
        .collect();
    let closed_audit_targets: Vec<&str> = new_audit
        .iter()
        .filter(|e| {
            e["command"] == "session pass"
                && e["status"] == "success"
                && e["target"].as_str().is_some_and(|t| t.starts_with("p-"))
        })
        .map(|e| e["target"].as_str().unwrap())
        .collect();
    assert_eq!(closed_audit_targets.len(), 16, "{new_audit:?}");
    for i in 1..=16 {
        assert!(closed_audit_targets.contains(&format!("p-{i}").as_str()));
    }

    // A closed candidate's quote stays in the suppression set, and a second
    // pass writes no more closes.
    let dup_quote = "I always prefer retired rule one for builds.";
    s.append(ON, &[line("user", "u-dup", dup_quote)]);
    s.answer(&candidate_answer(dup_quote, "u-dup", "preferences"));
    let audit_before_second = s.audit_lines();
    s.pass_session(ON);
    assert_eq!(
        proposals(&s)["total"],
        21,
        "closed candidate quote is still suppressed"
    );
    let second_audit: Vec<Value> = support::audit_lines(&s.fx.root())
        .into_iter()
        .skip(audit_before_second)
        .collect();
    assert_eq!(second_audit.len(), 1, "only the session pass line");
    assert_eq!(second_audit[0]["target"], format!("session:{ON}"));

    // `proposal list --state rejected` shows `tag_retired` in table and JSON.
    let rejected_json: Value = s.fx.check(
        Cli(&["proposal", "list", "--state", "rejected", "--json"]),
        Json,
    );
    assert_eq!(rejected_json["total"], 16);
    assert!(
        rejected_json["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["closed_reason"] == "tag_retired")
    );
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
fn pass_with_custom_tag_config_keeps_open_preferences_candidates() {
    let s = Setup::new();
    s.scribe("/scribe on");
    let config = s.fx.root().join("config.toml");
    std::fs::write(&config, "[tags.remind]\naction = true\n").unwrap();
    make_private(&config);

    let open_pref = json!({
        "id": "p-1",
        "kind": "candidate",
        "older": "preferences-1",
        "newer": null,
        "candidate": {
            "tag": "preferences",
            "role": "user",
            "quote": "I always prefer clear commit messages.",
            "evidence": null,
            "cite": {
                "session": ON,
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
        "reason": "Unmarked preferences statement: I always prefer clear commit messages.",
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
    seed_claims(&s.fx.root(), &[open_pref]);

    let new_quote = "I prefer concise summaries in every session pass.";
    s.write(ON, &[line("user", "u-new", new_quote)]);
    s.answer(&candidate_answer(new_quote, "u-new", "preferences"));
    s.pass_session(ON);

    let listed = proposals(&s);
    assert_eq!(listed["total"], 2, "{listed}");
    let list = listed["proposals"].as_array().unwrap();
    assert_eq!(list[0]["id"], "p-1");
    assert_eq!(list[0]["state"], "open");
    assert!(list[0]["closed_reason"].is_null());
    assert_eq!(list[1]["id"], "p-2");
    assert_eq!(list[1]["state"], "open");
    assert_eq!(list[1]["candidate"]["tag"], "preferences");
}
