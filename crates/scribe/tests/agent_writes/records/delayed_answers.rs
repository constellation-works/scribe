#[test]
fn delayed_older_answer_uses_the_retained_choice_for_session_notice() {
    let fx = Fixture::new();
    let earlier = decision_transcript_in(
        &fx,
        DECISION_SESSION,
        "2026-11-10T01:00:00Z",
        &[("user", "Choose A")],
    );
    let later = decision_transcript_in(
        &fx,
        LATER_SESSION,
        "2026-11-11T02:00:00Z",
        &[("user", "Choose B")],
    );
    let required_session = format!("session:{LATER_SESSION}");
    let added = add_record(
        &fx,
        "open-item",
        &[
            "--asked-by",
            "0-chief",
            "--blocked",
            "Build",
            "--answer-in",
            &required_session,
            "Choose",
        ],
    );
    assert!(added.status.success(), "{added:?}");
    let answer = |words, session| {
        settle(
            &fx,
            "answer",
            "open-item-1",
            &["--quote", words, "--related", session],
        )
    };
    let accepted_outside = answer("Choose A", &earlier);
    assert!(accepted_outside.status.success(), "{accepted_outside:?}");
    assert!(
        text(&accepted_outside.stderr).contains("must also type it in that session"),
        "{accepted_outside:?}"
    );
    let newer = answer("Choose B", &later);
    assert!(newer.status.success(), "{newer:?}");
    assert!(
        !text(&newer.stderr).contains("must also type it in that session"),
        "{newer:?}"
    );
    let expected = json(&newer.stdout)["item"]["answer"].clone();
    assert_eq!(expected["words"], "Choose B");
    assert_eq!(expected["at"], "2026-11-11T02:00:00Z");
    for (words, session) in [("Choose A", &earlier), ("Choose B", &later)] {
        let retry = answer(words, session);
        assert!(retry.status.success(), "{retry:?}");
        assert!(text(&retry.stderr).contains("nothing changed"), "{retry:?}");
        assert!(
            !text(&retry.stderr).contains("must also type it in that session"),
            "{retry:?}"
        );
        assert_eq!(json(&retry.stdout)["item"]["answer"], expected);
    }
}

#[test]
fn delayed_answer_does_not_replace_a_later_repetition_of_the_choice() {
    let fx = Fixture::new();
    let first_a = "2026-11-10T01:00:00Z";
    let delayed_b = "2026-11-10T02:00:00Z";
    let later_a = "2026-11-10T03:00:00Z";
    let original = decision_transcript_in(&fx, DECISION_SESSION, first_a, &[("user", "Choose A")]);
    let later = decision_transcript_in(&fx, LATER_SESSION, delayed_b, &[("user", "Choose B")]);
    assert!(
        add_record(
            &fx,
            "open-item",
            &["--asked-by", "0-chief", "--blocked", "Build", "Choose"]
        )
        .status
        .success()
    );
    let answer = |words, session| {
        settle(
            &fx,
            "answer",
            "open-item-1",
            &["--quote", words, "--related", session],
        )
    };
    assert!(answer("Choose A", &original).status.success());
    // Daniel types A again in the original session, after B was typed.
    decision_transcript_timed(
        &fx,
        DECISION_SESSION,
        &[(first_a, "user", "Choose A"), (later_a, "user", "Choose A")],
    );
    let repeated = answer("Choose A", &original);
    assert!(repeated.status.success(), "{repeated:?}");
    let expected = json(&repeated.stdout)["item"]["answer"].clone();
    assert_eq!(expected["words"], "Choose A");
    assert_eq!(expected["at"], later_a);
    // B reaches the store late: it is older than the repeated A.
    let delayed = answer("Choose B", &later);
    assert!(delayed.status.success(), "{delayed:?}");
    assert!(
        text(&delayed.stderr).contains("nothing changed"),
        "{delayed:?}"
    );
    assert_eq!(json(&delayed.stdout)["item"]["answer"], expected);
    // Replaying each stored typed turn changes nothing.
    for (words, session) in [("Choose A", &original), ("Choose B", &later)] {
        let retry = answer(words, session);
        assert!(retry.status.success(), "{retry:?}");
        assert!(text(&retry.stderr).contains("nothing changed"), "{retry:?}");
        assert_eq!(json(&retry.stdout)["item"]["answer"], expected);
    }
}

#[test]
fn an_answer_typed_outside_the_named_session_is_recorded_with_a_notice() {
    let fx = Fixture::new();
    let session = decision_transcript(&fx, &[("user", "yes, restart it")]);
    let elsewhere = "session:5f0c3c1e-2b7a-4d7e-9a43-0c6f1d2e3b4a";
    let added = add_record(
        &fx,
        "open-item",
        &[
            "--asked-by",
            "orbit-0-capt",
            "--blocked",
            "nothing",
            "--answer-in",
            elsewhere,
            "Restart the dashboard",
        ],
    );
    assert!(added.status.success(), "{added:?}");
    let out = settle(
        &fx,
        "answer",
        "open-item-1",
        &["--quote", "yes, restart it", "--related", &session],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(
        text(&out.stderr).contains("must also type it in that session"),
        "{out:?}"
    );
    assert_eq!(json(&out.stdout)["state"], "answered");
}

#[test]
fn an_answer_takes_the_latest_matching_turn_across_named_sessions_in_either_order() {
    let fx = Fixture::new();
    let earlier = decision_transcript_in(
        &fx,
        DECISION_SESSION,
        "2026-11-10T01:00:00Z",
        &[("user", "yes, restart it")],
    );
    let later = decision_transcript_in(
        &fx,
        LATER_SESSION,
        "2026-11-11T02:00:00Z",
        &[("user", "yes, restart it")],
    );
    // The latest typed turn is in LATER_SESSION, so the answer belongs there
    // whichever session is named first; the notice fires only when the item
    // names another session.
    let mut item = 0;
    for (answer_in, notice) in [(&later, false), (&earlier, true)] {
        for order in [[&earlier, &later], [&later, &earlier]] {
            item += 1;
            let added = add_record(
                &fx,
                "open-item",
                &[
                    "--asked-by",
                    "orbit-0-capt",
                    "--blocked",
                    "nothing",
                    "--answer-in",
                    answer_in,
                    "Restart the dashboard",
                ],
            );
            assert!(added.status.success(), "{added:?}");
            let out = settle(
                &fx,
                "answer",
                &format!("open-item-{item}"),
                &[
                    "--quote",
                    "yes, restart it",
                    "--related",
                    order[0],
                    "--related",
                    order[1],
                ],
            );
            assert!(out.status.success(), "{out:?}");
            let body = json(&out.stdout);
            let answer = &body["item"]["answer"];
            assert_eq!(answer["session"], LATER_SESSION, "{order:?}");
            assert_eq!(answer["at"], "2026-11-11T02:00:00Z", "{order:?}");
            assert_eq!(
                text(&out.stderr).contains("must also type it in that session"),
                notice,
                "{order:?} {out:?}"
            );
        }
    }
}

#[test]
fn the_brief_overflow_list_reaches_an_answered_item_beyond_ten_open_ones() {
    // 2_design.md §18.5: the brief shows ten lines in all, and its overflow hint
    // names `--state open --state answered`, so that list must reach the rest.
    let fx = Fixture::new();
    let session = decision_transcript(&fx, &[("user", "1")]);
    for n in 1..=11 {
        let question = format!("Restart the dashboard {n}");
        assert!(
            add_record(
                &fx,
                "open-item",
                &[
                    "--asked-by",
                    "0-chief",
                    "--blocked",
                    "Local checks",
                    &question
                ],
            )
            .status
            .success()
        );
    }
    // open-item-11 is answered; the other ten stay open.
    let answered = settle(
        &fx,
        "answer",
        "open-item-11",
        &["--quote", "1", "--related", &session],
    );
    assert!(answered.status.success(), "{answered:?}");

    let open = states(&fx, &["open"]);
    assert_eq!(open.len(), 10, "{open:?}");
    assert!(!open.iter().any(|s| s.ends_with(" answered")), "{open:?}");

    let both = states(&fx, &["open", "answered"]);
    assert_eq!(both.len(), 11, "{both:?}");
    assert!(
        both.iter().any(|s| s == "open-item-11 answered"),
        "{both:?}"
    );
}
