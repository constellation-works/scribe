#[test]
fn a_pass_claim_carries_a_context_and_subject_and_a_line_or_an_empty_subject_opens_nothing() {
    let s = Setup::new();
    let quote = "ok that makes sense. I prefer that session records keep the whole history in an append-only log.";
    let vague = "I prefer we keep it like this from now on.";
    let long = "I prefer squash merges on every agent branch we open.";
    let terse = "I prefer the short form of every release note.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            line("user", "u-2", vague),
            line("user", "u-3", long),
            line("user", "u-4", terse),
        ],
    );
    // The design's example (2_design.md §15.3): what was being discussed
    // and what the remark settles, standing alone.
    let context = "The user and the agent were choosing how scribe stores session records. The user rejected a single 2,500-character outcome because rewrites lose the session's start, and asked for an append-only log.";
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": quote, "message": "u-1", "tag": "preferences",
            "context": context, "subject": "session records"},
        // The window does not name what this turn is about.
        {"quote": vague, "message": "u-2", "tag": "preferences",
            "context": "The user and the agent were reviewing it. The user prefers to keep it like this.", "subject": " "},
        // A subject past 80 characters is not a short noun phrase.
        {"quote": long, "message": "u-3", "tag": "preferences",
            "context": "The user and the agent were merging. The user prefers squash merges on agent branches.",
            "subject": "x".repeat(81)},
        // A single line does not stand alone (2_design.md §15.3).
        {"quote": terse, "message": "u-4", "tag": "preferences",
            "context": "The user prefers short release notes.", "subject": "release notes"}
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    let candidate = &listed["proposals"][0]["candidate"];
    assert_eq!(candidate["quote"], quote);
    assert_eq!(candidate["context"], context);
    assert_eq!(candidate["statement"], Value::Null);
    assert_eq!(candidate["subject"], "session records");
    // The request asked for both, within their bounds.
    let prompt = s.prompts().join("\n");
    assert!(prompt.contains("context (a paragraph of"), "{prompt}");

    // `proposal list` and `proposal show` read the context first and
    // Daniel's words after it; `proposal show --json` carries it.
    let plain = s.fx.check(Cli(&["proposal", "list"]), Text);
    assert!(
        plain.contains(&format!("\t{context} | The user's words: \"{quote}\"\t")),
        "{plain}"
    );
    let shown = s.fx.check(Cli(&["proposal", "show", "p-1"]), Text);
    assert!(
        shown.contains(&format!("\t{context} | The user's words: \"{quote}\"\t")),
        "{shown}"
    );
    let shown: Value =
        s.fx.check(Cli(&["proposal", "show", "p-1", "--json"]), Json);
    assert_eq!(shown["candidate"]["context"], context);

    // Approval writes scribe's context as content and his words as quote;
    // the entry stays his.
    type_settlement(&s, "#scribe approve p-1");
    let entry: Value = s.entry_json("preferences-1");
    assert_eq!(entry["content"], context);
    assert_eq!(entry["quote"], quote);
    assert_eq!(
        entry["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    assert_eq!(entry["author"]["kind"], "user");
    assert_eq!(entry["content_from"]["kind"], "message");
    assert_eq!(entry["content_from"]["role"], "user");
    assert_eq!(entry["content_from"]["message"], "u-1");
    assert_eq!(entry["cite"]["message"], "u-1");
    let plain = s.fx.check(Cli(&["entry", "show", "preferences-1"]), Text);
    assert!(
        plain
            .lines()
            .any(|l| l == format!("stated_by\tscribe:session-pass\t{quote}")),
        "{plain}"
    );
}

#[test]
fn a_candidate_from_before_statements_approves_with_its_quote_as_content() {
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    // Rewrite it as a format-13 store holds it: no statement, no subject.
    support::sql(
        &s.fx.root(),
        "UPDATE claims SET c_context = NULL, c_statement = NULL, c_subject = NULL WHERE n = 1;",
    );
    assert_eq!(
        proposals(&s)["proposals"][0]["candidate"]["statement"],
        Value::Null
    );
    type_settlement(&s, "#scribe approve p-1");
    let entry: Value = s.entry_json("preferences-1");
    assert_eq!(entry["content"], quote);
    assert_eq!(entry["quote"], Value::Null);
    assert_eq!(entry["stated_by"], Value::Null);
    assert_eq!(entry["author"]["kind"], "user");
}

#[test]
fn a_format_23_claim_reads_with_no_context_shows_its_statement_as_an_older_claim_and_approves_it() {
    // 2_design.md §15.3: a claim opened before format 24 is not rewritten.
    // It reads with `context: null`, `proposal show` puts its statement in
    // the context's place labelled `older claim`, and approval keeps the
    // statement as content; the next write persists format 24.
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    let statement = "The user prefers a short explanation beside each changed behavior.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let doc = support::proposals_doc(&s.fx.root());
    assert_eq!(doc["format"], 31);
    assert!(doc["proposals"][0]["candidate"]["context"].is_string());
    let conn = rusqlite::Connection::open(s.fx.root().join("scribe.db")).unwrap();
    conn.execute(
        "UPDATE claims SET c_context = NULL, c_statement = ?1 WHERE n = 1",
        [statement],
    )
    .unwrap();
    drop(conn);

    let shown: Value =
        s.fx.check(Cli(&["proposal", "show", "p-1", "--json"]), Json);
    assert_eq!(shown["candidate"]["context"], Value::Null);
    assert_eq!(shown["candidate"]["statement"], statement);
    let plain = s.fx.check(Cli(&["proposal", "show", "p-1"]), Text);
    assert!(
        plain.contains(&format!(
            "\tolder claim: {statement} | The user's words: \"{quote}\"\t"
        )),
        "{plain}"
    );
    let table =
        s.fx.check(Cli(&["proposal", "show", "p-1", "--format", "table"]), Text);
    assert!(
        table.contains(&format!("older claim: {statement}")),
        "{table}"
    );

    type_settlement(&s, "#scribe approve p-1");
    let entry: Value = s.entry_json("preferences-1");
    assert_eq!(entry["content"], statement);
    assert_eq!(entry["quote"], quote);
    assert_eq!(
        entry["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    let doc = support::proposals_doc(&s.fx.root());
    assert_eq!(doc["format"], 31);
    assert_eq!(doc["proposals"][0]["candidate"]["context"], Value::Null);
    assert_eq!(doc["proposals"][0]["candidate"]["statement"], statement);
}

#[test]
fn a_context_needing_redaction_is_kept_redacted_and_a_quote_needing_it_is_omitted() {
    let s = Setup::new();
    let clean = "I prefer that the deploy job reads its token from the vault.";
    let leaky = "I prefer to paste sensitive-value-123 into the deploy job directly.";
    s.write(
        ON,
        &[line("user", "u-1", clean), line("user", "u-2", leaky)],
    );
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": clean, "message": "u-1", "tag": "preferences",
            "statement": "Daniel prefers that the deploy job reads sensitive-value-123 from the vault, never from the job file.",
            "subject": "the deploy job's sensitive-value-123"},
        {"quote": leaky, "message": "u-2", "tag": "preferences",
            "statement": "Daniel prefers pasting the deploy token into the job.",
            "subject": "the deploy job"}
    ]);
    s.answer(&body);
    s.config("[redact]\nenv = [\"TEST_SECRET\"]\n");
    s.scribe("/scribe on");
    let out = s.run_pass(
        &["session", "pass", "--session", ON, "--json"],
        &[("TEST_SECRET", "sensitive-value-123")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    let candidate = &listed["proposals"][0]["candidate"];
    assert_eq!(candidate["quote"], clean);
    assert_eq!(
        candidate["context"],
        context_of(
            "Daniel prefers that the deploy job reads [redacted:TEST_SECRET] from the vault, never from the job file."
        )
    );
    assert_eq!(
        candidate["subject"],
        "the deploy job's [redacted:TEST_SECRET]"
    );
    let stored = support::exported(&s.fx.root(), "proposals.json").unwrap();
    assert!(!stored.contains("sensitive-value-123"), "{stored}");
}

#[test]
fn a_context_and_subject_at_their_caps_stay_when_redaction_lengthens_them() {
    // The caps hold the model's words; the mask `[redacted:TEST_SECRET]`
    // is longer than the secret it replaces, and the masked value is kept.
    const SECRET: &str = "sv-12345678";
    let s = Setup::new();
    let quote = "I prefer that the deploy job reads its token from the vault.";
    s.write(ON, &[line("user", "u-1", quote)]);
    let head = format!(
        "The user and the agent were setting up the deploy job. The user prefers that it reads {SECRET} from the vault"
    );
    let context = format!("{head}{}.", " and never from the job file".repeat(30));
    let context: String = context.chars().take(599).chain(['.']).collect();
    assert_eq!(context.chars().count(), 600);
    let subject = format!("the deploy job's {SECRET} {}", "x".repeat(80));
    let subject: String = subject.chars().take(80).collect();
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": quote, "message": "u-1", "tag": "preferences",
            "context": context, "subject": subject}
    ]);
    s.answer(&body);
    s.config("[redact]\nenv = [\"TEST_SECRET\"]\n");
    s.scribe("/scribe on");
    let out = s.run_pass(
        &["session", "pass", "--session", ON, "--json"],
        &[("TEST_SECRET", SECRET)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    let candidate = &listed["proposals"][0]["candidate"];
    let masked_context = context.replace(SECRET, "[redacted:TEST_SECRET]");
    let masked_subject = subject.replace(SECRET, "[redacted:TEST_SECRET]");
    assert!(masked_context.chars().count() > 600);
    assert!(masked_subject.chars().count() > 80);
    assert_eq!(candidate["context"], masked_context.as_str());
    assert_eq!(candidate["subject"], masked_subject.as_str());
    let stored = support::exported(&s.fx.root(), "proposals.json").unwrap();
    assert!(!stored.contains(SECRET), "{stored}");

    // The longer, masked context still approves and reads back.
    let mut payload = s.payload("UserPromptSubmit");
    payload["prompt"] = json!("#scribe approve p-1");
    let out = s.hook_with_env("user-prompt-submit", &payload, "TEST_SECRET", SECRET);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let entry: Value = s.entry_json("preferences-1");
    assert_eq!(entry["content"], masked_context.as_str());
    assert_eq!(entry["quote"], quote);
}

#[test]
fn a_claim_whose_context_an_entry_already_says_opens_nothing() {
    let s = Setup::new();
    let quote = "ok fine, I prefer that we keep the changelog out of task branches.";
    s.write(ON, &[line("user", "u-1", quote)]);
    let context = context_of("Daniel prefers that task branches never touch the changelog.");
    let mut body = answer();
    body["candidates"] = json!([{"quote": quote, "message": "u-1", "tag": "preferences",
        "context": context, "subject": "task branches"}]);
    s.answer(&body);
    let recorded = format!("Recorded earlier: {context}");
    s.fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--project",
            "scribe",
            "--author",
            "opus",
            &recorded,
        ]),
        Success,
    );
    s.scribe("/scribe on");
    s.pass_session(ON);
    assert_eq!(proposals(&s)["total"], 0, "the context is already recorded");
}

// --- ideas the pass writes itself (2_design.md §11.6) -----------------------
