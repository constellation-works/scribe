#[test]
fn prompt_hooks_audit_newly_persisted_member_turns_once_per_turn_for_each_provider() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);

    // Claude Code: UserPromptSubmit persists a plain typed turn already in the
    // transcript, prints nothing, writes one content-free audit line, and
    // neither replay nor Stop writes a second line.
    let claude_path = transcript(&f, B, &[row(B, "c-1", "user", "Keep claudepromptorchid.")]);
    let before_claude = support::audit_lines(&f.root());
    let prompt_out = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({
            "session_id": B,
            "transcript_path": claude_path,
            "cwd": f.home(),
            "prompt": "Keep claudepromptorchid."
        }),
        &[],
    );
    assert!(prompt_out.stdout.is_empty(), "{}", text(&prompt_out.stdout));
    let after_claude = support::audit_lines(&f.root());
    assert_eq!(
        after_claude.len(),
        before_claude.len() + 1,
        "{after_claude:?}"
    );
    let line = after_claude.last().unwrap();
    assert_eq!(line["command"], "hook user-prompt-submit");
    assert_eq!(line["status"], "success");
    assert_eq!(line["target"], format!("session:{B}"));
    assert!(line["error_code"].is_null());
    assert!(!support::audit_text(&f.root()).contains("claudepromptorchid"));

    // Replay UserPromptSubmit and then Stop on the unchanged transcript.
    hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({
            "session_id": B,
            "transcript_path": claude_path,
            "cwd": f.home(),
            "prompt": "Keep claudepromptorchid."
        }),
        &[],
    );
    stop(&f, B, &claude_path, &[]);
    assert_eq!(support::audit_lines(&f.root()), after_claude);
    assert_eq!(
        invoke(
            &f,
            A,
            &[
                "search",
                "claudepromptorchid",
                "--kind",
                "message",
                "--group",
                "g-1"
            ],
            0
        )["total"],
        1
    );

    // Codex: join another member session C and verify both UserPromptSubmit
    // (before vs after rollout persistence) and Stop replay idempotency.
    let join_c = f.check(
        with_env(
            &[
                "--provider",
                "codex",
                "group",
                "join",
                "g-1",
                "--as",
                "reviewer",
                "--author",
                "aide",
            ],
            "CODEX_THREAD_ID",
            C,
        ),
        Capture,
    );
    assert!(join_c.status.success(), "{}", text(&join_c.stderr));

    let dir = f.home().join(".codex/sessions/2090/11/20");
    f.create_private_dir_all(&dir);
    let codex_path = dir.join(format!("rollout-2090-11-20T01-00-00-{C}.jsonl"));
    let meta = json!({"type":"session_meta","payload":{"id":C,"source":"vscode","thread_source":"user","cwd":f.home()}});
    let write_rollout = |rows: &[Value]| {
        std::fs::write(
            &codex_path,
            rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )
        .unwrap();
        support::make_private(&codex_path);
    };
    write_rollout(std::slice::from_ref(&meta));

    // Before the rollout holds the plain turn: UserPromptSubmit writes nothing.
    let before_codex = support::audit_lines(&f.root());
    let early = hook(
        &f,
        &["--provider", "codex", "hook", "user-prompt-submit"],
        json!({
            "session_id": C,
            "transcript_path": codex_path,
            "hook_event_name": "UserPromptSubmit",
            "turn_id": "turn-1",
            "prompt": "Keep codexdeferredorchid."
        }),
        &[],
    );
    assert!(early.stdout.is_empty(), "{}", text(&early.stdout));
    assert_eq!(support::audit_lines(&f.root()), before_codex);

    // Once Stop sees the persisted turn in the rollout, Stop audits it once.
    let turn_1 = json!({"timestamp":"2090-11-20T01:00:00Z","type":"response_item","payload":{
        "type":"message","role":"user","id":"codex-m1",
        "content":[{"type":"input_text","text":"Keep codexdeferredorchid."}],
        "internal_chat_message_metadata_passthrough":{
            "turn_id":"turn-1","content_item_kinds":["user.text"],
            "content_item_metadata":[{"provenance":{"type":"user"}}]
        }
    }});
    write_rollout(&[meta.clone(), turn_1.clone()]);
    hook(
        &f,
        &["--provider", "codex", "hook", "stop"],
        json!({
            "session_id": C,
            "transcript_path": codex_path,
            "hook_event_name": "Stop",
            "turn_id": "turn-1"
        }),
        &[],
    );
    let after_codex_stop = support::audit_lines(&f.root());
    assert_eq!(
        after_codex_stop.len(),
        before_codex.len() + 1,
        "{after_codex_stop:?}"
    );
    let line = after_codex_stop.last().unwrap();
    assert_eq!(line["command"], "hook stop");
    assert_eq!(line["status"], "success");
    assert_eq!(line["target"], format!("session:{C}"));
    assert!(line["error_code"].is_null());
    assert!(!support::audit_text(&f.root()).contains("codexdeferredorchid"));

    // Next turn already present in rollout when UserPromptSubmit runs:
    // UserPromptSubmit persists and audits it; replay and Stop add no second line.
    let turn_2 = json!({"timestamp":"2090-11-20T01:05:00Z","type":"response_item","payload":{
        "type":"message","role":"user","id":"codex-m2",
        "content":[{"type":"input_text","text":"Keep codexpromptorchid."}],
        "internal_chat_message_metadata_passthrough":{
            "turn_id":"turn-2","content_item_kinds":["user.text"],
            "content_item_metadata":[{"provenance":{"type":"user"}}]
        }
    }});
    write_rollout(&[meta, turn_1, turn_2]);
    let submit_2 = hook(
        &f,
        &["--provider", "codex", "hook", "user-prompt-submit"],
        json!({
            "session_id": C,
            "transcript_path": codex_path,
            "hook_event_name": "UserPromptSubmit",
            "turn_id": "turn-2",
            "prompt": "Keep codexpromptorchid."
        }),
        &[],
    );
    assert!(submit_2.stdout.is_empty(), "{}", text(&submit_2.stdout));
    let after_codex_prompt = support::audit_lines(&f.root());
    assert_eq!(
        after_codex_prompt.len(),
        after_codex_stop.len() + 1,
        "{after_codex_prompt:?}"
    );
    let line = after_codex_prompt.last().unwrap();
    assert_eq!(line["command"], "hook user-prompt-submit");
    assert_eq!(line["status"], "success");
    assert_eq!(line["target"], format!("session:{C}"));
    assert!(line["error_code"].is_null());
    assert!(!support::audit_text(&f.root()).contains("codexpromptorchid"));

    // Replaying UserPromptSubmit and running Stop on unchanged rollout add no line.
    hook(
        &f,
        &["--provider", "codex", "hook", "user-prompt-submit"],
        json!({
            "session_id": C,
            "transcript_path": codex_path,
            "hook_event_name": "UserPromptSubmit",
            "turn_id": "turn-2",
            "prompt": "Keep codexpromptorchid."
        }),
        &[],
    );
    hook(
        &f,
        &["--provider", "codex", "hook", "stop"],
        json!({
            "session_id": C,
            "transcript_path": codex_path,
            "hook_event_name": "Stop",
            "turn_id": "turn-2"
        }),
        &[],
    );
    assert_eq!(support::audit_lines(&f.root()), after_codex_prompt);
    assert_eq!(
        invoke(
            &f,
            A,
            &[
                "search",
                "codexpromptorchid",
                "--kind",
                "message",
                "--group",
                "g-1"
            ],
            0
        )["total"],
        1
    );
}

#[test]
fn claude_member_capture_preserves_provenance_and_record_boundaries_across_appends() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);

    let mut first_claim = row(B, "claim", "user", "untrusted claim");
    first_claim["entrypoint"] = json!("sdk-cli");
    first_claim.as_object_mut().unwrap().remove("timestamp");
    let mut first_message = row(B, "replay", "user", "agent turn");
    first_message["source"] = json!("agent");
    let path = transcript(
        &f,
        B,
        &[
            first_claim,
            first_message,
            row(B, "original", "user", "originalorchid"),
        ],
    );
    let prompt = || {
        hook(
            &f,
            &["hook", "user-prompt-submit"],
            json!({"session_id": B, "transcript_path": path, "cwd": f.home(), "prompt": "Continue."}),
            &[],
        );
    };
    prompt();
    let before = support::audit_lines(&f.root());
    let mut append = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    for line in [
        row(C, "copied", "user", "copiedorchid"),
        row(B, "replay", "user", "replayorchid"),
        row(B, "claim", "user", "claimedorchid"),
    ] {
        writeln!(append, "{line}").unwrap();
    }
    let next = row(B, "next", "user", "appendedorchid").to_string();
    let split = next.len() / 2;
    append.write_all(&next.as_bytes()[..split]).unwrap();
    prompt();
    assert_eq!(support::audit_lines(&f.root()), before);

    append.write_all(&next.as_bytes()[split..]).unwrap();
    // Stop can capture a complete final record before its newline arrives.
    stop(&f, B, &path, &[]);
    let after = support::audit_lines(&f.root());
    assert_eq!(after.len(), before.len() + 1);
    writeln!(append, "\n{}", row(B, "last", "user", "lastorchid")).unwrap();
    prompt();
    let after_prompt = support::audit_lines(&f.root());
    assert_eq!(after_prompt.len(), after.len() + 1);
    prompt();
    stop(&f, B, &path, &[]);
    assert_eq!(support::audit_lines(&f.root()), after_prompt);

    let total = |term| {
        invoke(
            &f,
            B,
            &["search", term, "--kind", "message", "--group", "g-1"],
            0,
        )["total"]
            .as_u64()
            .unwrap()
    };
    for term in ["copiedorchid", "replayorchid", "claimedorchid"] {
        assert_eq!(total(term), 0, "{term}");
    }
    for term in ["originalorchid", "appendedorchid", "lastorchid"] {
        assert_eq!(total(term), 1, "{term}");
    }

    #[cfg(unix)]
    {
        let outside = f.elsewhere("outside.jsonl");
        writeln!(append, "{}", row(B, "outside", "user", "outsideorchid")).unwrap();
        drop(append);
        std::fs::rename(&path, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        prompt();
        assert_eq!(support::audit_lines(&f.root()), after_prompt);
        assert_eq!(total("outsideorchid"), 0);
    }
}

#[test]
fn a_text_refused_typed_form_keeps_its_first_copy_verdict_across_captures() {
    // The slash command's args are an agent wrapper, so its text refuses it while
    // its line's claims read as Daniel's. An edited copy with the same uuid,
    // captured later, must not become his turn: a full replay of both copies
    // stores nothing, and the incremental captures must match it.
    let refused = || {
        json!({"sessionId": B, "uuid": "typed", "type": "user",
            "entrypoint": "claude-desktop", "origin": {"kind": "human"},
            "turnOrigin": "human", "timestamp": "2090-11-20T01:00:00Z",
            "message": {"role": "user", "content": "<command-message>formation</command-message>\n<command-name>/formation</command-name>\n<command-args><scheduled-prompt>refusedorchid</scheduled-prompt></command-args>"}})
    };
    let edited = row(B, "typed", "user", "editedorchid");

    let incremental = Fixture::new();
    transcript(&incremental, A, &[row(A, "u", "user", QUOTE)]);
    open(&incremental);
    join(&incremental, B, "g-1", &[], 0);
    let path = transcript(&incremental, B, &[refused()]);
    let prompt = || {
        hook(
            &incremental,
            &["hook", "user-prompt-submit"],
            json!({"session_id": B, "transcript_path": path, "cwd": incremental.home(), "prompt": "Continue."}),
            &[],
        );
    };
    prompt();
    let mut append = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(append, "{edited}").unwrap();
    drop(append);
    prompt();

    let replay = Fixture::new();
    transcript(&replay, A, &[row(A, "u", "user", QUOTE)]);
    open(&replay);
    join(&replay, B, "g-1", &[], 0);
    let path = transcript(&replay, B, &[refused(), edited]);
    hook(
        &replay,
        &["hook", "user-prompt-submit"],
        json!({"session_id": B, "transcript_path": path, "cwd": replay.home(), "prompt": "Continue."}),
        &[],
    );

    let replayed = stored_turns(&replay, B);
    assert!(replayed.is_empty(), "{replayed:?}");
    assert_eq!(stored_turns(&incremental, B), replayed);
}

#[test]
fn claude_member_capture_replays_after_rotation_and_waits_for_a_missing_cursor() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    let cursor = row(B, "cursor", "user", "cursororchid");
    let path = transcript(&f, B, std::slice::from_ref(&cursor));
    stop(&f, B, &path, &[]);
    let before = support::audit_lines(&f.root());

    std::fs::rename(&path, path.with_extension("old")).unwrap();
    transcript(
        &f,
        B,
        &[row(B, "before-cursor", "user", "missingcursororchid")],
    );
    stop(&f, B, &path, &[]);
    assert_eq!(support::audit_lines(&f.root()), before);

    let mut append = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(append, "{cursor}").unwrap();
    writeln!(
        append,
        "{}",
        row(B, "after-cursor", "user", "rotatedorchid")
    )
    .unwrap();
    drop(append);
    stop(&f, B, &path, &[]);
    let after = support::audit_lines(&f.root());
    assert_eq!(after.len(), before.len() + 1);
    stop(&f, B, &path, &[]);
    assert_eq!(support::audit_lines(&f.root()), after);

    let total = |term| {
        invoke(
            &f,
            B,
            &["search", term, "--kind", "message", "--group", "g-1"],
            0,
        )["total"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(total("missingcursororchid"), 0);
    assert_eq!(total("cursororchid"), 1);
    assert_eq!(total("rotatedorchid"), 1);

    support::sql(
        &f.root(),
        "ALTER TABLE group_members DROP COLUMN restarted; ALTER TABLE entries DROP COLUMN decision; ALTER TABLE entries DROP COLUMN item;
         DROP TABLE group_claude_seen; DROP TABLE group_claude_progress; PRAGMA user_version = 14;",
    );
    let mut append = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(append, "{}", row(B, "migration", "user", "migrationorchid")).unwrap();
    stop(&f, B, &path, &[]);
    assert_eq!(total("migrationorchid"), 1);
    assert_eq!(support::audit_lines(&f.root()).len(), after.len() + 1);
}

#[test]
fn typed_project_group_refuses_configured_secret_without_persisting_or_echoing_it() {
    const SECRET: &str = "orchid-secret-token";
    let f = Fixture::new();
    f.config("[redact]\nenv = [\"GROUP_SECRET\"]\n");
    let path = transcript(&f, A, &[row(A, "u", "user", "ordinary turn")]);
    for slug in [SECRET, "prefix-orchid-secret-token-suffix"] {
        let out = hook(
            &f,
            &["hook", "user-prompt-submit"],
            json!({"session_id": A, "transcript_path": path, "cwd": f.home(),
                   "prompt": format!("/scribe group {slug}")}),
            &[("GROUP_SECRET", SECRET)],
        );
        let reply = text(&out.stdout);
        assert!(
            reply.contains("group slug contains sensitive text"),
            "{reply}"
        );
        assert!(!reply.contains(SECRET));
        assert!(!text(&out.stderr).contains(SECRET));
    }
    let list = invoke(&f, A, &["group", "list"], 0);
    assert_eq!(list["items"].as_array().unwrap().len(), 0);
    let audit = support::audit_text(&f.root());
    assert!(!audit.contains(SECRET), "{audit}");
    assert!(
        support::audit_lines(&f.root())
            .iter()
            .any(|line| line["error_code"] == "invalid_text")
    );
}

#[test]
fn automatic_project_group_seat_avoids_taken_names_and_survives_a_retry() {
    let f = Fixture::new();
    let prompt = "/scribe group seat-collision";
    let transcript = transcript(&f, A, &[row(A, "typed", "user", prompt)]);
    hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id": A, "transcript_path": transcript, "cwd": f.home(), "prompt": prompt}),
        &[],
    );
    // E shares A's eight-character prefix; an explicit seat already uses E's full id.
    const E: &str = "11111111-eeee-4eee-8eee-eeeeeeeeeeee";
    let squatter = join_as(&f, B, "seat-collision", E, &[], 0);
    assert!(
        squatter["seats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == E)
    );

    let args = ["group", "join", "seat-collision", "--author", "aide"];
    let joined = invoke(&f, E, &args, 0);
    let seats: Vec<(&str, Vec<&str>)> = joined["seats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap(),
                s["transcripts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t.as_str().unwrap())
                    .collect(),
            )
        })
        .collect();
    let automatic = format!("{E}-2");
    assert!(seats.contains(&("11111111", vec![A])), "{seats:?}");
    assert!(seats.contains(&(E, vec![B])), "{seats:?}");
    assert!(seats.contains(&(automatic.as_str(), vec![E])), "{seats:?}");

    // The short name frees up; a retry keeps the existing automatic seat.
    invoke(&f, A, &["group", "leave"], 0);
    let retry = invoke(&f, E, &args, 0);
    let held: Vec<&str> = retry["seats"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["transcripts"].as_array().unwrap().contains(&json!(E)))
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(held, [automatic.as_str()]);
    assert!(retry["seats"].as_array().unwrap().len() == 2);
}
