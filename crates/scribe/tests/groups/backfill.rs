#[test]
fn open_stores_the_openers_earlier_typed_turns_and_no_other_line() {
    let f = Fixture::new();
    let [slash, queued] = desktop_rows(A);
    let mut rows = vec![slash, queued, row_at(A, "u", "user", QUOTE, OLD)];
    rows.extend(never_stored(A, B));
    transcript(&f, A, &rows);
    open(&f);
    assert_eq!(stored_turns(&f, A), ["slash", "queued", "u"]);
    assert_eq!(all_stored(&f), 3);

    // Another open from the same session stores nothing it has not stored.
    invoke(&f, A, &[OPEN, &["--quote", QUOTE]].concat(), 0);
    assert_eq!(stored_turns(&f, A), ["slash", "queued", "u"]);
    assert_eq!(all_stored(&f), 3);

    // The turns belong to the group once the opener takes a seat; no model ran.
    join_as(&f, A, "g-1", "chief", &[], 0);
    assert_eq!(search_group(&f, "formationorchid")["total"], 1);
    assert_eq!(search_group(&f, "queuedorchid")["total"], 1);
    for term in [
        "assistantorchid",
        "agentorchid",
        "metaorchid",
        "harnessorchid",
        "peerorchid",
        "scheduledorchid",
        "copiedorchid",
    ] {
        assert_eq!(search_group(&f, term)["total"], 0, "{term}");
    }
}

#[test]
fn join_stores_the_joining_sessions_earlier_typed_turns_and_no_other_line() {
    let f = Fixture::new();
    transcript(&f, A, &[row_at(A, "u", "user", QUOTE, OLD)]);
    open(&f);
    let [slash, queued] = desktop_rows(B);
    let mut rows = vec![slash, queued];
    rows.extend(never_stored(B, A));
    rows.push(row_at(
        B,
        "typed",
        "user",
        "typedorchid before the join",
        OLD,
    ));
    transcript(&f, B, &rows);
    let before = all_stored(&f);
    join(&f, B, "g-1", &[], 0);
    assert_eq!(stored_turns(&f, B), ["slash", "queued", "typed"]);
    // A's own earlier turn came with its open.
    assert_eq!(all_stored(&f), before + 3);
    assert_eq!(search_group(&f, "formationorchid")["total"], 1);
    assert_eq!(search_group(&f, "typedorchid")["total"], 1);
    for term in [
        "assistantorchid",
        "metaorchid",
        "copiedorchid",
        "scheduledorchid",
    ] {
        assert_eq!(search_group(&f, term)["total"], 0, "{term}");
    }
}

#[test]
fn a_capture_after_the_backfill_stores_no_turn_twice_and_a_retried_join_nothing() {
    let f = Fixture::new();
    let mut rows = vec![
        row_at(A, "u", "user", QUOTE, OLD),
        row_at(A, "early", "user", "earlyorchid typed before the join", OLD),
    ];
    let path = transcript(&f, A, &rows);
    open(&f);
    join_as(&f, A, "g-1", "chief", &[], 0);
    assert_eq!(stored_turns(&f, A), ["u", "early"]);

    // A retried join is a no-op.
    join_as(&f, A, "g-1", "chief", &[], 0);
    assert_eq!(stored_turns(&f, A), ["u", "early"]);

    let fake = f.fake_program("claude", "#!/bin/sh\nexit 1\n");
    let env = [("PATH", fake.to_str().unwrap())];
    stop(&f, A, &path, &env);
    assert_eq!(stored_turns(&f, A), ["u", "early"]);

    // A later turn is stored once, however many hooks read it, even when it is
    // dated before the join.
    rows.push(row_at(
        A,
        "late",
        "user",
        "lateorchid typed after the join",
        OLD,
    ));
    transcript(&f, A, &rows);
    stop(&f, A, &path, &env);
    stop(&f, A, &path, &env);
    assert_eq!(stored_turns(&f, A), ["u", "early", "late"]);
    assert_eq!(search_group(&f, "lateorchid")["total"], 1);
    assert_eq!(search_group(&f, "earlyorchid")["total"], 1);
}

#[test]
fn a_restored_seat_keeps_its_new_join_time_as_the_cutoff_after_a_backfilled_join() {
    let f = Fixture::new();
    let mut a_rows = vec![
        row_at(A, "u", "user", QUOTE, OLD),
        row_at(
            A,
            "before",
            "user",
            "beforeorchid typed before the join",
            OLD,
        ),
    ];
    let a_path = transcript(&f, A, &a_rows);
    transcript(&f, B, &[row_at(B, "b", "user", "joining now", OLD)]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    assert_eq!(stored_turns(&f, A), ["u", "before"]);
    let fake = f.fake_program("claude", "#!/bin/sh\nexit 1\n");
    let env = [("PATH", fake.to_str().unwrap())];

    join(&f, B, "g-1", &["--replace"], 0);
    a_rows.push(row_at(
        A,
        "during",
        "user",
        "duringorchid typed while displaced",
        OLD,
    ));
    transcript(&f, A, &a_rows);

    // B leaves: A holds the seat again from now, and nothing it typed
    // meanwhile is stored, even by a hook that ran after the restore.
    invoke(&f, B, &["group", "leave"], 0);
    stop(&f, A, &a_path, &env);
    assert_eq!(stored_turns(&f, A), ["u", "before"]);
    assert_eq!(search_group(&f, "duringorchid")["total"], 0);
    a_rows.push(row(
        A,
        "after",
        "user",
        "afterorchid typed after the restore",
    ));
    transcript(&f, A, &a_rows);
    stop(&f, A, &a_path, &env);
    assert_eq!(stored_turns(&f, A), ["u", "before", "after"]);
}

/// An explicit backfill keeps the restarted window of a restored holder: what
/// it typed while a later member held its seat was never authorized, so it is
/// not stored, while turns from before the displacement stay and turns typed
/// after the restore are stored.
#[test]
fn backfill_keeps_a_restored_holders_displaced_turns_out() {
    let f = Fixture::new();
    let mut a_rows = vec![
        row_at(A, "u", "user", QUOTE, OLD),
        row_at(
            A,
            "before",
            "user",
            "beforeorchid typed before the join",
            OLD,
        ),
    ];
    transcript(&f, A, &a_rows);
    transcript(&f, B, &[row_at(B, "b", "user", "joining now", OLD)]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    assert_eq!(stored_turns(&f, A), ["u", "before"]);

    join(&f, B, "g-1", &["--replace"], 0);
    a_rows.push(row_at(
        A,
        "during",
        "user",
        "duringorchid typed while displaced",
        OLD,
    ));
    transcript(&f, A, &a_rows);
    invoke(&f, B, &["group", "leave"], 0);
    a_rows.push(row(
        A,
        "after",
        "user",
        "afterorchid typed after the restore",
    ));
    transcript(&f, A, &a_rows);

    let result = invoke(&f, A, &["group", "backfill", "g-1"], 0);
    assert_eq!(result["stored"], 1, "{result}");
    assert_eq!(result["skipped"], 3, "{result}");
    assert_eq!(stored_turns(&f, A), ["u", "before", "after"]);
    assert_eq!(search_group(&f, "duringorchid")["total"], 0);
    assert_eq!(search_group(&f, "afterorchid")["total"], 1);
    let repeat = invoke(&f, A, &["group", "backfill", "g-1"], 0);
    assert_eq!(repeat["stored"], 0);
    assert_eq!(stored_turns(&f, A), ["u", "before", "after"]);
}

/// A version 17 store cannot say whether a holder was restored: the old
/// `remove_seat_member` reset `joined` but wrote no mark. The upgrade marks a
/// holder whose seat lost a later membership (a vanished `position`), so an
/// explicit backfill after the upgrade still keeps the displaced interval out.
#[test]
fn backfill_after_a_version_17_upgrade_keeps_an_already_restored_holders_cutoff() {
    let f = Fixture::new();
    let mut a_rows = vec![
        row_at(A, "u", "user", QUOTE, OLD),
        row_at(
            A,
            "before",
            "user",
            "beforeorchid typed before the join",
            OLD,
        ),
    ];
    transcript(&f, A, &a_rows);
    transcript(&f, B, &[row_at(B, "b", "user", "joining now", OLD)]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    join(&f, B, "g-1", &["--replace"], 0);
    a_rows.push(row_at(
        A,
        "during",
        "user",
        "duringorchid typed while displaced",
        OLD,
    ));
    transcript(&f, A, &a_rows);
    invoke(&f, B, &["group", "leave"], 0);
    a_rows.push(row(
        A,
        "after",
        "user",
        "afterorchid typed after the restore",
    ));
    transcript(&f, A, &a_rows);
    // Stage the store as a version 17 build left it: the restore reset
    // `joined`, there is no `restarted` column and `during` was never stored.
    support::sql(
        &f.root(),
        "ALTER TABLE group_members DROP COLUMN restarted; PRAGMA user_version = 17;",
    );
    assert_eq!(stored_turns(&f, A), ["u", "before"]);

    let result = invoke(&f, A, &["group", "backfill", "g-1"], 0);
    assert_eq!(result["stored"], 1, "{result}");
    assert_eq!(stored_turns(&f, A), ["u", "before", "after"]);
    assert_eq!(search_group(&f, "duringorchid")["total"], 0);
    let restarted: i64 = rusqlite::Connection::open(f.root().join("scribe.db"))
        .unwrap()
        .query_row(
            "SELECT restarted FROM group_members WHERE session = ?1",
            [A],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(restarted, 1);
}

/// The upgrade marks only a holder that can have been restored. One whose seat
/// never lost a later membership keeps backfilling its whole transcript.
#[test]
fn backfill_after_a_version_17_upgrade_leaves_a_never_displaced_holder_unmarked() {
    let f = Fixture::new();
    transcript(
        &f,
        A,
        &[
            row_at(A, "u", "user", QUOTE, OLD),
            row_at(
                A,
                "before",
                "user",
                "beforeorchid typed before the join",
                OLD,
            ),
        ],
    );
    open(&f);
    join(&f, A, "g-1", &[], 0);
    support::sql(
        &f.root(),
        "DELETE FROM messages;
         ALTER TABLE group_members DROP COLUMN restarted; PRAGMA user_version = 17;",
    );
    let result = invoke(&f, A, &["group", "backfill", "g-1"], 0);
    assert_eq!(result["stored"], 2, "{result}");
    assert_eq!(stored_turns(&f, A), ["u", "before"]);
}

#[test]
fn a_replace_or_clear_successor_backfills_only_its_own_transcript() {
    let f = Fixture::new();
    transcript(
        &f,
        A,
        &[
            row_at(A, "u", "user", QUOTE, OLD),
            row_at(
                A,
                "a1",
                "user",
                "predecessororchid typed by the holder",
                OLD,
            ),
        ],
    );
    open(&f);
    join(&f, A, "g-1", &[], 0);

    // A `--replace` successor: its own turns, never the holder's, and a line
    // of the holder's copied into its transcript is not its own.
    transcript(
        &f,
        B,
        &[
            row_at(A, "copied", "user", "copiedorchid from the holder", OLD),
            row_at(B, "b1", "user", "replaceorchid typed by the successor", OLD),
        ],
    );
    join(&f, B, "g-1", &["--replace"], 0);
    assert_eq!(stored_turns(&f, B), ["b1"]);
    assert_eq!(stored_turns(&f, A), ["u", "a1"]);

    // A clear successor of B: the same, with no `--replace`.
    let mut rows = vec![json!({"type":"derived-from","sessions":[
        {"via":"launch","id":D},{"via":"clear","id":B}]})];
    rows.push(row_at(
        B,
        "copied-b",
        "user",
        "copiedbyorchid from the predecessor",
        OLD,
    ));
    rows.push(row_at(
        C,
        "c1",
        "user",
        "clearorchid typed by the successor",
        OLD,
    ));
    transcript(&f, C, &rows);
    join(&f, C, "g-1", &[], 0);
    assert_eq!(stored_turns(&f, C), ["c1"]);
    assert_eq!(stored_turns(&f, B), ["b1"]);
    assert_eq!(search_group(&f, "copiedorchid")["total"], 0);
    assert_eq!(search_group(&f, "copiedbyorchid")["total"], 0);
}

#[test]
fn a_join_without_a_readable_transcript_still_joins_and_stores_nothing() {
    let f = Fixture::new();
    transcript(&f, A, &[row_at(A, "u", "user", QUOTE, OLD)]);
    open(&f);
    let path = transcript(&f, B, &[row_at(B, "b1", "user", "refusedorchid", OLD)]);
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(f.home().join("elsewhere"), &path).unwrap();
    join(&f, B, "g-1", &[], 0);
    assert!(stored_turns(&f, B).is_empty());
    assert_eq!(
        show_group(&f, B, "g-1")["seats"][0]["transcripts"],
        json!([B])
    );
}

#[test]
fn an_unrelated_duplicate_transcript_does_not_block_a_join_or_its_retry() {
    let f = Fixture::new();
    transcript(&f, A, &[row_at(A, "u", "user", QUOTE, OLD)]);
    open(&f);
    let other = transcript(&f, C, &[row_at(C, "c1", "user", "unrelated", OLD)]);
    duplicate_transcript(&f, C, &other);
    transcript(&f, B, &[row_at(B, "b1", "user", "ownorchid", OLD)]);
    join(&f, B, "g-1", &[], 0);
    assert_eq!(stored_turns(&f, B), ["b1"]);
    // The retry holds the seat already; the unrelated duplicate still does not matter.
    join(&f, B, "g-1", &[], 0);
    assert_eq!(stored_turns(&f, B), ["b1"]);
}

#[test]
fn the_callers_own_duplicate_transcript_still_refuses_the_join() {
    let f = Fixture::new();
    transcript(&f, A, &[row_at(A, "u", "user", QUOTE, OLD)]);
    open(&f);
    let own = transcript(&f, B, &[row_at(B, "b1", "user", "ownorchid", OLD)]);
    duplicate_transcript(&f, B, &own);
    let err = join(&f, B, "g-1", &[], 2);
    assert!(
        err.to_string().contains("ambiguous duplicate transcript"),
        "{err}"
    );
    assert!(stored_turns(&f, B).is_empty());
}

#[test]
fn an_idle_holder_yields_its_seat_despite_an_unrelated_duplicate() {
    let f = Fixture::new();
    transcript(&f, A, &[row_at(A, "u", "user", QUOTE, OLD)]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    let other = transcript(&f, C, &[row_at(C, "c1", "user", "unrelated", OLD)]);
    duplicate_transcript(&f, C, &other);
    transcript(&f, B, &[row_at(B, "b1", "user", "takeoverorchid", OLD)]);
    let group = join(&f, B, "g-1", &[], 0);
    assert_eq!(group["seats"][0]["transcripts"], json!([A, B]));
    assert_eq!(stored_turns(&f, B), ["b1"]);
}

#[test]
fn a_live_holder_keeps_its_seat_despite_an_unrelated_duplicate() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    let other = transcript(&f, C, &[row_at(C, "c1", "user", "unrelated", OLD)]);
    duplicate_transcript(&f, C, &other);
    transcript(&f, B, &[row_at(B, "b1", "user", "refusedorchid", OLD)]);
    assert_eq!(join(&f, B, "g-1", &[], 1)["code"], "seat_occupied");
    assert!(stored_turns(&f, B).is_empty());
}

#[test]
fn an_idle_holder_with_unverifiable_evidence_keeps_its_seat_despite_an_unrelated_duplicate() {
    let f = Fixture::new();
    let path = transcript(&f, A, &[row_at(A, "u", "user", QUOTE, OLD)]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(f.home().join("elsewhere"), &path).unwrap();
    let other = transcript(&f, C, &[row_at(C, "c1", "user", "unrelated", OLD)]);
    duplicate_transcript(&f, C, &other);
    transcript(&f, B, &[row_at(B, "b1", "user", "refusedorchid", OLD)]);
    assert_eq!(join(&f, B, "g-1", &[], 1)["code"], "seat_occupied");
    assert!(stored_turns(&f, B).is_empty());
}

#[test]
fn an_idle_holders_own_duplicate_still_refuses_the_takeover() {
    let f = Fixture::new();
    let holder = transcript(&f, A, &[row_at(A, "u", "user", QUOTE, OLD)]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    duplicate_transcript(&f, A, &holder);
    transcript(&f, B, &[row_at(B, "b1", "user", "refusedorchid", OLD)]);
    let err = join(&f, B, "g-1", &[], 2);
    assert!(
        err.to_string().contains("ambiguous duplicate transcript"),
        "{err}"
    );
    assert!(stored_turns(&f, B).is_empty());
}

#[test]
fn backfill_existing_holders_is_idempotent_and_keeps_capture_cursor() {
    let f = Fixture::new();
    transcript(&f, A, &[row_at(A, "u", "user", QUOTE, OLD)]);
    open(&f);
    // Join without a transcript, as old versions did without earlier turns.
    join(&f, B, "g-1", &[], 0);
    join_as(&f, C, "g-1", "missing", &[], 0);
    let [slash, queued] = desktop_rows(B);
    let mut rows = vec![slash, queued];
    rows.extend(never_stored(B, A));
    transcript(&f, B, &rows);
    let db = rusqlite::Connection::open(f.root().join("scribe.db")).unwrap();
    let cursor = || {
        db.query_row(
            "SELECT turn_cursor FROM group_members WHERE session = ?1",
            [B],
            |r| r.get::<_, Option<String>>(0),
        )
        .unwrap()
    };
    db.execute(
        "UPDATE group_members SET turn_cursor = 'later-cursor' WHERE session = ?1",
        [B],
    )
    .unwrap();
    let before = cursor();
    let result = invoke(&f, A, &["group", "backfill", "g-1"], 0);
    assert_eq!(result["stored"], 2);
    assert_eq!(result["members"][0]["seat"], "editor");
    assert_eq!(result["members"][0]["session"], B);
    assert_eq!(result["members"][1]["transcript_unavailable"], true);
    assert_eq!(stored_turns(&f, B), ["slash", "queued"]);
    assert_eq!(cursor(), before);
    let repeat = invoke(&f, A, &["group", "backfill", "g-1"], 0);
    assert_eq!(repeat["stored"], 0);
    assert_eq!(repeat["skipped"], 2);
    assert_eq!(cursor(), before);
}

#[test]
fn backfill_reports_terminal_and_refuses_closed_groups() {
    let f = Fixture::new();
    transcript(&f, A, &[row_at(A, "u", "user", QUOTE, OLD)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    let out = f.check(Cli(&["group", "backfill", "g-1"]), Capture);
    assert!(out.status.success());
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains(&B[..8])
            && stdout.contains("transcript unavailable")
            && stdout.contains("Total:"),
        "{stdout}"
    );
    assert!(close_as(&f, A).status.success());
    let error = invoke(&f, A, &["group", "backfill", "g-1"], 1);
    assert_eq!(error["code"], "group_membership");
}
