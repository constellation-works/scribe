#[test]
fn uncataloged_transcript_prefix_from_transcript_list_works_for_quoted_and_self_refresh_handoffs() {
    let since = (time::OffsetDateTime::now_utc() - time::Duration::days(1))
        .date()
        .to_string();

    // 1. Quoted handoff for an uncataloged transcript with no store record.
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Please write a handoff for this uncataloged session.";
    tr.write(&[("u-1", "user", request)]);

    let listed = fx.check(
        Cli(&[
            "transcript",
            "list",
            "--since",
            &since,
            "--min-turns",
            "1",
            "--format",
            "table",
        ]),
        Text,
    );
    let off_prefix = &OFF_SESSION[..8];
    assert!(listed.contains(off_prefix), "{listed}");
    assert!(!listed.contains(OFF_SESSION), "{listed}");

    let quoted = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &format!("session:{off_prefix}"),
            "--quote",
            request,
            &valid_handoff("Quoted via prefix.", "Continue."),
            "--json",
        ],
        Success,
    ));
    assert_eq!(quoted["related"], json!([format!("session:{OFF_SESSION}")]));
    let shown = json_of(&fx.check(&["show", "handoff-1", "--json"], Success));
    assert_eq!(shown["related"], json!([format!("session:{OFF_SESSION}")]));
    assert!(!support::has_control(&fx.root()));
    assert!(support::record_text(&fx.root(), OFF_SESSION).is_none());

    // 2. Self-refresh handoff for a fresh uncataloged transcript with no store record.
    let fx2 = Fixture::new();
    let refresh_tr = Transcript::new(&fx2, ON_SESSION);
    refresh_tr.write(&[("u-1", "user", "Let's refresh our handoff.")]);
    let listed2 = fx2.check(
        Cli(&[
            "transcript",
            "list",
            "--since",
            &since,
            "--min-turns",
            "1",
            "--format",
            "table",
        ]),
        Text,
    );
    let on_prefix = &ON_SESSION[..8];
    assert!(listed2.contains(on_prefix), "{listed2}");
    assert!(!listed2.contains(ON_SESSION), "{listed2}");

    let refresh_out = fx2.check(
        with_env(
            &[
                "entry",
                "add",
                "--tag",
                "handoff",
                "--author",
                "opus",
                "--related",
                &format!("session:{on_prefix}"),
                "--mode",
                "continue",
                "--json",
                &valid_handoff("Refreshed via prefix.", "Keep going."),
            ],
            "CLAUDE_CODE_SESSION_ID",
            ON_SESSION,
        ),
        Capture,
    );
    assert!(
        refresh_out.status.success(),
        "{}",
        text(&refresh_out.stderr)
    );
    let refreshed = json_of(&refresh_out);
    assert_eq!(refreshed["handoff_mode"], "continue");
    assert_eq!(
        refreshed["related"],
        json!([format!("session:{ON_SESSION}")])
    );

    // 3. Ambiguity across two uncataloged transcripts refuses both quoted and self-refresh writes.
    let fx3 = Fixture::new();
    let sibling = "5d1c0b7e-9999-4c39-8e61-0a9b8c7d6e5f";
    Transcript::new(&fx3, OFF_SESSION).write(&[("u-1", "user", request)]);
    Transcript::new(&fx3, sibling).write(&[("u-1", "user", "Another session sharing prefix.")]);

    let listed3 = fx3.check(
        Cli(&[
            "transcript",
            "list",
            "--since",
            &since,
            "--min-turns",
            "1",
            "--format",
            "table",
        ]),
        Text,
    );
    assert!(listed3.contains("5d1c0b7e-2 "), "{listed3}");
    assert!(listed3.contains("5d1c0b7e-9 "), "{listed3}");

    let ambig_quoted = fx3.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--related",
            &format!("session:{off_prefix}"),
            "--quote",
            request,
            "--json",
            &valid_handoff("Ambiguous quoted.", "Stop."),
        ],
        Code(2),
    );
    let err: Value = serde_json::from_slice(&ambig_quoted.stderr).unwrap();
    assert_eq!(err["code"], "id_ambiguous");
    assert!(
        err["error"].as_str().unwrap().contains(OFF_SESSION)
            && err["error"].as_str().unwrap().contains(sibling),
        "{err}"
    );
    assert_eq!(
        json_of(&fx3.check(&["list", "--json"], Success))["total"],
        0
    );

    let ambig_refresh = fx3.check(
        with_env(
            &[
                "entry",
                "add",
                "--tag",
                "handoff",
                "--author",
                "opus",
                "--related",
                &format!("session:{off_prefix}"),
                "--mode",
                "continue",
                "--json",
                &valid_handoff("Ambiguous refresh.", "Stop."),
            ],
            "CLAUDE_CODE_SESSION_ID",
            OFF_SESSION,
        ),
        Code(2),
    );
    let err: Value = serde_json::from_slice(&ambig_refresh.stderr).unwrap();
    assert_eq!(err["code"], "id_ambiguous");
    assert_eq!(
        json_of(&fx3.check(&["list", "--json"], Success))["total"],
        0
    );

    // Passing the disambiguated prefix `5d1c0b7e-2` succeeds and cites the full ID.
    let ok_refresh = fx3.check(
        with_env(
            &[
                "entry",
                "add",
                "--tag",
                "handoff",
                "--author",
                "opus",
                "--related",
                "session:5d1c0b7e-2",
                "--mode",
                "continue",
                "--json",
                &valid_handoff("Disambiguated refresh.", "Go."),
            ],
            "CLAUDE_CODE_SESSION_ID",
            OFF_SESSION,
        ),
        Capture,
    );
    assert!(ok_refresh.status.success(), "{}", text(&ok_refresh.stderr));
    assert_eq!(
        json_of(&ok_refresh)["related"],
        json!([format!("session:{OFF_SESSION}")])
    );
}
