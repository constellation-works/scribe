#[test]
fn a_unique_prefix_names_a_session_and_an_exact_id_wins() {
    let fx = Fixture::seeded();
    let shown = ok_json(&fx, &["session", "show", "7c2b"]);
    assert_eq!(shown["id"], SESSION);
    let segment = ok_json(&fx, &["session", "show", "session:7c2b9d14#2"]);
    assert_eq!(segment["id"], SESSION);
    assert_eq!(segment["log"].as_array().unwrap().len(), 1);
    // A second session whose id extends the first's.
    let longer = format!("{SESSION}-b");
    copy_session(&fx, &longer);
    assert_eq!(ok_json(&fx, &["session", "show", SESSION])["id"], SESSION);
    assert_eq!(ok_json(&fx, &["session", "show", &longer])["id"], longer);
    let err = usage_error(&fx, &["session", "show", "7c2b"]);
    assert_eq!(err["code"], "id_ambiguous");
    let message = err["error"].as_str().unwrap();
    assert!(
        message.contains(SESSION) && message.contains(&longer),
        "{message}"
    );
    // Fewer than four characters is never a prefix.
    let short = fx.check(&["session", "show", "0f8", "--json"], Capture);
    assert_eq!(short.status.code(), Some(2));
    assert_eq!(json(&short.stderr)["code"], "usage_error");
}

#[test]
fn an_exact_entry_id_wins_over_a_longer_one_it_prefixes() {
    let fx = Fixture::seeded();
    support::seed_records(
        &fx.root(),
        &[&support::entry_text(10, "a tenth preference")],
    );
    assert_eq!(
        ok_json(&fx, &["entry", "show", "preferences-1"])["id"],
        "preferences-1"
    );
    assert_eq!(
        ok_json(&fx, &["entry", "show", "preferences-10"])["id"],
        "preferences-10"
    );
    assert_eq!(ok_json(&fx, &["entry", "show", "ritu"])["id"], "rituals-1");
    let err = usage_error(&fx, &["entry", "show", "pref"]);
    assert_eq!(err["code"], "id_ambiguous");
    let message = err["error"].as_str().unwrap();
    for id in ["preferences-1", "preferences-10"] {
        assert!(message.contains(id), "{message}");
    }
}

#[test]
fn session_writes_take_a_prefix_and_audit_the_full_id() {
    let fx = Fixture::seeded();
    fx.check(
        &[
            "session",
            "deactivate",
            "7c2b9d14",
            "--author",
            "opus",
            "--reason",
            "gone",
        ],
        Success,
    );
    let shown = ok_json(&fx, &["session", "show", SESSION]);
    assert_eq!(shown["active"], false);
    let lines = support::audit_lines(&fx.root());
    let line = lines.last().unwrap();
    assert_eq!(line["target"], format!("session:{SESSION}"));
    fx.check(
        &["session", "activate", "7c2b", "--author", "opus"],
        Success,
    );
    assert_eq!(ok_json(&fx, &["session", "show", SESSION])["active"], true);
    copy_session(&fx, "7c2b0000-0000-4000-8000-000000000000");
    let err = usage_error(
        &fx,
        &[
            "session",
            "deactivate",
            "7c2b",
            "--author",
            "opus",
            "--reason",
            "gone",
        ],
    );
    assert_eq!(err["code"], "id_ambiguous");
    let pass = usage_error(&fx, &["session", "pass", "--session", "7c2b"]);
    assert_eq!(pass["code"], "id_ambiguous");
}

#[test]
fn the_record_noun_is_gone_and_search_is_a_top_level_verb() {
    // `scribe search` replaced `record search` with no alias.
    let fx = Fixture::seeded();
    for args in [&["record", "search", "merge"][..], &["record"]] {
        let out = fx.check(args, Capture);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    fx.check(&["search", "merge agent PRs"], Success);
}

#[test]
fn a_table_prints_unique_prefixes_and_piped_output_aliases() {
    let fx = Fixture::seeded();
    let table = |fx: &Fixture| {
        text(
            &fx.check(&["session", "list", "--format", "table"], Success)
                .stdout,
        )
    };
    // Unique at eight characters, the first group of a UUID.
    let listed = table(&fx);
    assert!(listed.contains("7c2b9d14 "), "{listed}");
    assert!(!listed.contains(SESSION), "{listed}");
    // A printed prefix is accepted back.
    assert_eq!(
        ok_json(&fx, &["session", "show", "7c2b9d14"])["id"],
        SESSION
    );
    // Piped output uses an alias that resolves; JSON keeps the provider id.
    let plain = text(
        &fx.check(&["session", "list", "--format", "auto"], Success)
            .stdout,
    );
    assert!(
        plain.lines().any(|line| line.starts_with("s-3\t")),
        "{plain}"
    );
    assert_eq!(ok_json(&fx, &["session", "show", "s-3"])["id"], SESSION);
    let listed_json = ok_json(&fx, &["session", "list"]);
    assert!(ids(&listed_json, "sessions", "id").contains(&SESSION.to_owned()));
    // A session sharing the first eight characters (a UUIDv7's timestamp)
    // lengthens both, past the shared run, and each is accepted back.
    let sibling = "7c2b9d14-ffff-4a3b-b1c6-2f9e8d7a6b02";
    copy_session(&fx, sibling);
    let listed = table(&fx);
    assert!(listed.contains("7c2b9d14-8 "), "{listed}");
    assert!(listed.contains("7c2b9d14-f "), "{listed}");
    assert_eq!(
        ok_json(&fx, &["session", "show", "7c2b9d14-8"])["id"],
        SESSION
    );
    assert_eq!(
        ok_json(&fx, &["session", "show", "7c2b9d14-f"])["id"],
        sibling
    );
}

#[test]
fn cursor_only_codex_sessions_disambiguate_cataloged_session_prefixes() {
    let fx = Fixture::seeded();
    let cursor_only = "7c2b9d14-ffff-4a3b-b1c6-2f9e8d7a6b02";
    // The first store write migrates the checked-in document fixture into
    // SQLite, where cursor-only ids are persisted independently of sessions.
    fx.check(
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            "fixture migration",
        ],
        Success,
    );
    fx.check(
        &["session", "activate", SESSION, "--author", "opus"],
        Success,
    );
    support::sql(
        &fx.root(),
        &format!(
            "INSERT INTO codex_cursors (session, message) VALUES ('{cursor_only}', 'cursor-1')"
        ),
    );

    let table = text(
        &fx.check(&["session", "list", "--format", "table"], Success)
            .stdout,
    );
    assert!(table.contains("7c2b9d14-8 "), "{table}");

    let err = usage_error(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--related",
            "session:7c2b9d14",
            "Cache the manifest between runs",
        ],
    );
    assert_eq!(err["code"], "id_ambiguous");
    let message = err["error"].as_str().unwrap();
    assert!(
        message.contains(SESSION) && message.contains(cursor_only),
        "{message}"
    );
}

#[test]
fn a_session_reference_given_as_a_prefix_is_stored_in_full() {
    let fx = Fixture::seeded();
    let added = ok_json(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--related",
            "session:7c2b9d14",
            "Cache the manifest between runs",
        ],
    );
    let related: Vec<&str> = added["related"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert_eq!(related, [format!("session:{SESSION}")]);
    // The same prefix filters a list.
    let listed = ok_json(&fx, &["entry", "list", "--related", "session:7c2b9d14"]);
    assert_eq!(listed["total"], 1);
    // An ambiguous prefix is refused, names the sessions and writes nothing.
    let longer = format!("{SESSION}-b");
    copy_session(&fx, &longer);
    let before = ok_json(&fx, &["entry", "list"])["total"].clone();
    let err = usage_error(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--related",
            "session:7c2b9d14",
            "Another idea about caching",
        ],
    );
    assert_eq!(err["code"], "id_ambiguous");
    let message = err["error"].as_str().unwrap();
    assert!(
        message.contains(SESSION) && message.contains(&longer),
        "{message}"
    );
    assert_eq!(ok_json(&fx, &["entry", "list"])["total"], before);
}

#[test]
fn literal_session_references_remain_filterable_and_removable_after_cataloging() {
    let fx = Fixture::seeded();
    let literal = ok_json(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--related",
            "session:abcd",
            "Keep this reference until the session is known",
        ],
    );
    let id = literal["id"].as_str().unwrap();
    let expected = support::session_id_outcome("related_literal");
    assert_eq!(expected["exit"], 0); // ok_json observed exit 0
    assert!(expected["code"].is_null());
    assert_eq!(literal["related"][0], expected["value"]);

    // Make a longer matching session known only after the reference was
    // written. This is the ordering that turns resolution into a regression.
    copy_session(&fx, "abcd1234");
    let filtered = ok_json(&fx, &["entry", "list", "--related", "session:abcd"]);
    assert!(ids(&filtered, "entries", "id").contains(&id.to_owned()));

    let removed = ok_json(
        &fx,
        &[
            "entry",
            "update",
            id,
            "--author",
            "opus",
            "--remove-related",
            "session:abcd",
        ],
    );
    assert!(removed["related"].as_array().unwrap().is_empty());
    assert!(
        !ids(
            &ok_json(&fx, &["entry", "list", "--related", "session:abcd"]),
            "entries",
            "id"
        )
        .contains(&id.to_owned())
    );

    // Fully canonical references keep the established add, filter, and
    // removal behavior.
    let canonical = ok_json(
        &fx,
        &[
            "entry",
            "update",
            id,
            "--author",
            "opus",
            "--add-related",
            "session:abcd1234",
        ],
    );
    assert_eq!(canonical["related"][0], "session:abcd1234");
    assert!(
        ids(
            &ok_json(&fx, &["entry", "list", "--related", "session:abcd1234"]),
            "entries",
            "id"
        )
        .contains(&id.to_owned())
    );
    let removed = ok_json(
        &fx,
        &[
            "entry",
            "update",
            id,
            "--author",
            "opus",
            "--remove-related",
            "session:abcd1234",
        ],
    );
    assert!(removed["related"].as_array().unwrap().is_empty());
}

#[test]
fn session_id_matrix_rows_have_boundary_cases() {
    let rows: Vec<Value> =
        serde_json::from_str(include_str!("../../../../scripts/session-id-outcomes.json")).unwrap();
    let ids: Vec<&str> = rows.iter().map(|row| row["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        [
            "related_literal",
            "manual_link",
            "session_pass",
            "instruction_missing",
            "instruction_prefix",
            "instruction_ambiguous"
        ]
    );
}
