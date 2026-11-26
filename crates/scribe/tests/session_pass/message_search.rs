#[test]
fn a_pass_keeps_the_windows_typed_turns_for_search_kind_message() {
    let s = Setup::new();
    let quote = "Let us keep the merge queue for agent pull requests.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            // An assistant turn no claim cites is not kept.
            line("assistant", "a-1", "Noted: the merge queue stays."),
            // Fenced code is not the user's typed words.
            line(
                "user",
                "u-2",
                "Here is the old helper:\n```\nfn merge_all() {}\n```\nIt can go.",
            ),
        ],
    );
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);
    let db = s.fx.root().join("scribe.db");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let search = |terms: &str| -> Value {
        s.fx.check(Cli(&["search", terms, "--kind", "message", "--json"]), Json)
    };
    let found = search("merge");
    assert_eq!(found["total"], 1, "{found}");
    let hit = &found["hits"][0];
    assert_eq!(hit["kind"], "message");
    assert_eq!(hit["message"], "u-1");
    assert_eq!(hit["cite"], format!("session:{ON}"));
    assert_eq!(hit["snippet"], quote);
    assert_eq!(hit["project"], "scribe");
    assert_eq!(
        search("helper")["hits"][0]["snippet"],
        "Here is the old helper: It can go."
    );
    // The default kinds are entries and sessions: no message hit.
    let default: Value = s.fx.check(Cli(&["search", "merge", "--json"]), Json);
    assert!(
        default["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["kind"] != "message"),
        "{default}"
    );
    let plain =
        s.fx.check(Cli(&["search", "merge", "--kind", "message"]), Text);
    assert!(plain.contains("session:s-1"), "{plain}");

    // A read-only copy of the store, without the -wal and -shm files, is
    // opened immutable and says so.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let copy = s.fx.elsewhere("copy");
        copy_store_preserving_modes(&s.fx, &copy);
        std::fs::remove_file(copy.join("scribe.db-wal")).unwrap();
        std::fs::remove_file(copy.join("scribe.db-shm")).unwrap();
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o500)).unwrap();
        let copy_arg = copy.display().to_string();
        let out = s.fx.check(
            Cli(&["--root", &copy_arg, "search", "merge", "--kind", "message"]),
            Success,
        );
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
        assert!(
            stderr.contains("read-only copy") || stdout.contains("read-only copy"),
            "{stdout}{stderr}"
        );
        assert!(stdout.contains("merge queue"), "{stdout}");
    }
}

#[test]
fn an_empty_default_search_names_the_stored_messages_that_match() {
    let s = Setup::new();
    // The word is only in a turn he typed: no entry or session holds it.
    let quote = "Keep the quartzlark rollout note out of the release.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);
    let json = |out: &Output| -> Value { serde_json::from_slice(&out.stdout).unwrap() };

    // The default search finds no entry or session, and its last stderr line
    // names the messages and the command that lists them.
    let found = s.fx.check(Cli(&["search", "quartzlark"]), Success);
    assert!(found.stdout.is_empty(), "{}", text(&found.stdout));
    let stderr = text(&found.stderr);
    let hint = stderr.lines().last().unwrap();
    let command = hint
        .strip_prefix("1 stored message(s) match, and no entry or session does: ")
        .unwrap()
        .strip_suffix(" lists them")
        .unwrap();
    assert!(command.starts_with("scribe --root="), "{hint}");
    assert!(
        command.ends_with("search --kind message -- quartzlark"),
        "{hint}"
    );
    #[cfg(unix)]
    assert_message_hint_replays(
        &s.fx,
        hint,
        "1 stored message(s) match, and no entry or session does: ",
        "quartzlark",
    );
    let listed = json(&s.fx.check(
        Cli(&["search", "quartzlark", "--kind", "message", "--json"]),
        Success,
    ));
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["hits"][0]["message"], "u-1", "{listed}");
    let default_json = json(
        &s.fx
            .check(Cli(&["search", "quartzlark", "--json"]), Success),
    );
    assert_eq!(default_json["hits"], json!([]), "{default_json}");
    assert_eq!(default_json["message_matches"], 1, "{default_json}");

    // The count takes the same filters, so a project with no such message
    // gets no hint.
    let other = s.fx.check(
        Cli(&["search", "quartzlark", "--project", "orbit"]),
        Success,
    );
    assert_eq!(
        text(&other.stderr),
        "no records match \"quartzlark\" --project orbit\n"
    );

    // With no matching message, the output and the JSON are unchanged.
    let none = s.fx.check(Cli(&["search", "zzznomatch"]), Success);
    assert_eq!(text(&none.stderr), "no records match \"zzznomatch\"\n");
    let none_json = json(
        &s.fx
            .check(Cli(&["search", "zzznomatch", "--json"]), Success),
    );
    assert!(none_json.get("message_matches").is_none(), "{none_json}");

    // An entry that holds the word answers the default search, so no hint.
    s.fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "Quartzlark is the rollout code name.",
        ]),
        Success,
    );
    let answered = json(
        &s.fx
            .check(Cli(&["search", "quartzlark", "--json"]), Success),
    );
    assert_eq!(answered["total"], 1, "{answered}");
    assert!(answered.get("message_matches").is_none(), "{answered}");
    assert!(
        !text(&s.fx.check(Cli(&["search", "quartzlark"]), Success).stderr)
            .contains("stored message")
    );
}

#[test]
fn a_message_hint_beside_a_hidden_match_names_only_the_visible_entries_and_sessions() {
    let s = Setup::new();
    // The word is in a turn he typed, and in a retired entry: the default
    // search finds no visible entry or session, so the hint names the message.
    let quote = "Keep the quartzlark rollout note out of the release.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);
    let added: Value = s.fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "Quartzlark was the old rollout code name.",
            "--json",
        ]),
        Json,
    );
    let id = added["id"].as_str().unwrap();
    s.fx.check(
        Cli(&[
            "entry",
            "retire",
            id,
            "--author",
            "opus",
            "--reason",
            "the code name changed",
        ]),
        Success,
    );

    let found = s.fx.check(Cli(&["search", "quartzlark"]), Success);
    assert!(found.stdout.is_empty(), "{}", text(&found.stdout));
    let stderr = text(&found.stderr);
    let hidden = "1 hidden record(s) also match: superseded or retired entries, or inactive sessions; --include-hidden shows them";
    assert!(stderr.lines().any(|l| l == hidden), "{stderr}");
    let hint = stderr.lines().last().unwrap();
    let command = hint
        .strip_prefix("1 stored message(s) match, and no visible entry or session does: ")
        .unwrap()
        .strip_suffix(" lists them")
        .unwrap();
    assert!(command.starts_with("scribe --root="), "{hint}");
    assert!(
        command.ends_with("search --kind message -- quartzlark"),
        "{hint}"
    );
    #[cfg(unix)]
    assert_message_hint_replays(
        &s.fx,
        hint,
        "1 stored message(s) match, and no visible entry or session does: ",
        "quartzlark",
    );
    assert!(!stderr.contains("and no entry or session does"), "{stderr}");

    // The count is the same JSON key, and it names the visible message.
    let json: Value = s.fx.check(Cli(&["search", "quartzlark", "--json"]), Json);
    assert_eq!(json["total"], 0, "{json}");
    assert_eq!(json["hits"], json!([]), "{json}");
    assert_eq!(json["message_matches"], 1, "{json}");

    // Showing the hidden entry answers the search, so the hint goes.
    let shown = s.fx.check(
        Cli(&["search", "quartzlark", "--include-hidden", "--json"]),
        Success,
    );
    let shown_json: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown_json["total"], 1, "{shown_json}");
    assert!(shown_json.get("message_matches").is_none(), "{shown_json}");
    assert!(
        !text(&shown.stderr).contains("stored message"),
        "{}",
        text(&shown.stderr)
    );
}

#[test]
fn a_message_search_hint_keeps_its_root_filters_and_dash_leading_terms() {
    let s = Setup::new();
    let quote = "Keep the quartzlark rollout note out of the release.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);

    // Move the populated fixture store to a nondefault root with spaces so
    // the suggested command must preserve both the selected root and quote it.
    let root = s.fx.elsewhere("selected store");
    std::fs::rename(s.fx.root(), &root).unwrap();
    let root_arg = root.display().to_string();
    let search = s.fx.check(
        Cli(&[
            "--root",
            &root_arg,
            "search",
            "--project",
            "scribe",
            "--since",
            "2026-11-03",
            "--until",
            "2026-11-03",
            "--",
            "-quartzlark",
        ]),
        Success,
    );
    let stderr = text(&search.stderr);
    let hint = stderr.lines().last().unwrap();
    let command = hint
        .strip_prefix("1 stored message(s) match, and no entry or session does: ")
        .unwrap()
        .strip_suffix(" lists them")
        .unwrap();
    assert!(command.starts_with("scribe --root="), "{hint}");
    assert!(
        command.ends_with(
            "search --kind message --project scribe --since 2026-11-03 --until 2026-11-03 -- -quartzlark"
        ),
        "{hint}"
    );

    // Execute the hinted command with JSON inserted before its terminator.
    let listed: Value = s.fx.check(
        Cli(&[
            "--root",
            &root_arg,
            "search",
            "--kind",
            "message",
            "--project",
            "scribe",
            "--since",
            "2026-11-03",
            "--until",
            "2026-11-03",
            "--json",
            "--",
            "-quartzlark",
        ]),
        Json,
    );
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["hits"][0]["message"], "u-1", "{listed}");

    // On Unix, run the actual shell command from the hint, adding JSON before
    // its terminator so the output can be checked.
    #[cfg(unix)]
    {
        let stderr = text(&search.stderr);
        let line = stderr.lines().last().unwrap();
        let command = json_before_terminator(
            line.strip_prefix("1 stored message(s) match, and no entry or session does: ")
                .unwrap()
                .strip_suffix(" lists them")
                .unwrap(),
            "-quartzlark",
        );
        let out = replay_hint(&s.fx, &command, None, true);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let shell_listed: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(shell_listed["total"], 1, "{shell_listed}");
        assert_eq!(shell_listed["hits"][0]["message"], "u-1", "{shell_listed}");
    }
}

#[cfg(unix)]
#[test]
fn a_message_hint_replays_when_the_temp_dir_holds_a_separator() {
    const DEFAULT: &str = "1 stored message(s) match, and no entry or session does: ";
    // Every path in this fixture holds the literal " -- " that a replay must
    // not take for its terminator, whatever TMPDIR is.
    let s = Setup::in_fixture(Fixture::with_dir_prefix("scratch -- "));
    let quote = "Keep the quartzlark rollout note out of the release.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);

    // The default root is printed as one quoted word that holds the separator.
    let found = s.fx.check(Cli(&["search", "quartzlark"]), Success);
    let stderr = text(&found.stderr);
    let hint = stderr.lines().last().unwrap();
    assert!(
        hint.contains("--root='") && hint.contains("scratch -- "),
        "{hint}"
    );
    assert_message_hint_replays(&s.fx, hint, DEFAULT, "quartzlark");

    // The explicit root is printed the same way.
    let root = s.fx.elsewhere("selected store");
    std::fs::rename(s.fx.root(), &root).unwrap();
    let root_arg = root.display().to_string();
    let search = s.fx.check(
        Cli(&[
            "--root",
            &root_arg,
            "search",
            "--project",
            "scribe",
            "--since",
            "2026-11-03",
            "--until",
            "2026-11-03",
            "--",
            "-quartzlark",
        ]),
        Success,
    );
    let stderr = text(&search.stderr);
    let hint = stderr.lines().last().unwrap();
    assert!(hint.contains("scratch -- "), "{hint}");
    assert_message_hint_replays(&s.fx, hint, DEFAULT, "-quartzlark");
}

#[cfg(unix)]
#[test]
fn a_message_search_hint_replays_unusual_roots_without_terminal_controls() {
    for root_name in ["-selected", "control-\u{1}", "newline-\n", "c1-\u{85}"] {
        let s = Setup::new();
        let quote = "Keep the quartzlark rollout note out of the release.";
        s.write(ON, &[line("user", "u-1", quote)]);
        s.answer(&candidate_answer(quote, "u-1", "preferences"));
        s.scribe("/scribe on");
        s.pass_session(ON);

        let root = s.fx.elsewhere(root_name);
        std::fs::rename(s.fx.root(), &root).unwrap();
        let parent = root.parent().unwrap();
        let search = search_relative_root(&s.fx, parent, root_name, "-quartzlark");
        assert!(search.status.success(), "{}", text(&search.stderr));
        let stderr = text(&search.stderr);
        let hint = stderr
            .lines()
            .last()
            .unwrap()
            .strip_prefix("1 stored message(s) match, and no entry or session does: ")
            .unwrap()
            .strip_suffix(" lists them")
            .unwrap();
        assert!(!hint.chars().any(char::is_control), "{hint:?}");
        let command = json_before_terminator(hint, "-quartzlark");
        let replay = replay_hint(&s.fx, &command, Some(parent), true);
        assert!(replay.status.success(), "{}", text(&replay.stderr));
        let listed: Value = serde_json::from_slice(&replay.stdout).unwrap();
        assert_eq!(listed["total"], 1, "{root_name:?}: {listed}");
        assert_eq!(listed["hits"][0]["message"], "u-1", "{listed}");
    }
}

#[test]
fn a_message_search_hint_replays_a_project_with_an_apostrophe() {
    let json = |out: &Output| -> Value { serde_json::from_slice(&out.stdout).unwrap() };
    let s = Setup::new();
    // A project is the cwd's basename, so this transcript names it with an
    // apostrophe, which a POSIX shell reads as an opening quote.
    let project = "editor's-project";
    let quote = "Keep the quartzlark rollout note out of the release.";
    s.write(
        ON,
        &[
            json!({"type": "user", "uuid": "u-1", "timestamp": "2026-11-03T06:00:00Z",
                 "cwd": format!("/nonexistent/work/{project}"),
                 "message": {"role": "user", "content": quote}}),
        ],
    );
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.pass_session(ON);

    let search = s.fx.check(
        Cli(&["search", "quartzlark", "--project", project]),
        Success,
    );
    let stderr = text(&search.stderr);
    let hint = stderr.lines().last().unwrap();
    assert!(
        hint.contains(r"--project 'editor'\''s-project'"),
        "the project is one shell word: {hint}"
    );

    // The direct argv search is the reference the replayed hint must match.
    let direct = json(&s.fx.check(
        Cli(&[
            "search",
            "quartzlark",
            "--kind",
            "message",
            "--project",
            project,
            "--json",
        ]),
        Success,
    ));
    assert_eq!(direct["total"], 1, "{direct}");
    assert_eq!(direct["hits"][0]["message"], "u-1", "{direct}");

    // On Unix, replay the hinted command in sh, adding JSON before its
    // terminator, and compare the hits with the direct argv search.
    #[cfg(unix)]
    {
        let command = json_before_terminator(
            hint.strip_prefix("1 stored message(s) match, and no entry or session does: ")
                .unwrap()
                .strip_suffix(" lists them")
                .unwrap(),
            "quartzlark",
        );
        let out = replay_hint(&s.fx, &command, None, true);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{command}\n{}",
            text(&out.stderr)
        );
        let replayed: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(replayed["total"], direct["total"], "{replayed}");
        assert_eq!(replayed["hits"], direct["hits"], "{command}");
    }
}
