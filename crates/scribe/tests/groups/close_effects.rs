#[test]
fn session_start_includes_last_group_when_it_is_the_only_brief_content() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    let member = transcript(
        &f,
        B,
        &[row(B, "u", "user", "Review the release checklist.")],
    );
    stop(&f, B, &member, &[]);

    f.check(
        with_env(
            &["group", "close", "g-1", "--summary", "-", "--json"],
            "CLAUDE_CODE_SESSION_ID",
            A,
        ),
        Success.stdin(b"Retained group summary."),
    );
    let group = show_group(&f, A, "g-1");
    let summary = group["summary"].as_str().unwrap();
    assert!(summary.contains("Retained group summary."), "{summary}");

    let standalone = invoke(&f, A, &["brief", "--project", "scribe"], 0);
    assert!(standalone["sessions"].as_array().unwrap().is_empty());
    assert_eq!(standalone["claim_count"], 0);
    assert_eq!(standalone["claims"], json!([]));
    assert!(standalone["memory"]["profile"].is_null());
    assert_eq!(standalone["memory"]["now"], json!([]));
    assert_eq!(standalone["memory"]["milestones"], json!([]));
    assert_eq!(standalone["last_group"]["summary"], summary);

    let project = f.elsewhere("scribe");
    f.create_private_dir_all(&project);
    let reply = session_start_with_cwd(&f, C, None, "startup", "claude-desktop", Some(&project));
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("## Last group"), "{context}");
    assert!(context.contains(summary), "{context}");

    let empty_project = f.elsewhere("empty-project");
    f.create_private_dir_all(&empty_project);
    let empty = session_start_with_cwd(
        &f,
        D,
        None,
        "startup",
        "claude-desktop",
        Some(&empty_project),
    );
    assert!(
        empty["hookSpecificOutput"]["additionalContext"].is_null(),
        "{empty}"
    );
}

#[test]
fn opening_flags_only_other_open_groups_in_the_same_project() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    let args = [OPEN, &["--quote", QUOTE]].concat();
    let out = f.check(with_env(&args, "CLAUDE_CODE_SESSION_ID", A), Capture);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("g-1 is still open"));
    let mut other = args;
    let project = other.iter().position(|s| *s == "--project").unwrap();
    other[project + 1] = "other";
    let out = f.check(with_env(&other, "CLAUDE_CODE_SESSION_ID", A), Capture);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
    assert_eq!(
        invoke(&f, A, &["brief", "--project", "scribe"], 0)["last_group"]["id"],
        "g-2"
    );
}

#[test]
fn close_imports_forgotten_residue_erased_and_keeps_only_the_success_audit() {
    let f = Fixture::new();
    let opener = transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    let forgotten = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({
            "session_id": A, "transcript_path": opener, "cwd": f.home(),
            "prompt": format!("#scribe forget {B}")
        }),
        &[],
    );
    assert!(text(&forgotten.stdout).contains("forgot"));

    // Re-create residue a previous build could have left after the forget.
    let archive = f.root().join("pre-sqlite-stale/sessions/2026/11");
    f.create_private_dir_all(&archive);
    let residue = archive.join(format!("{B}.md"));
    std::fs::write(
        &residue,
        format!("---\nid: {B}\n---\nforgotten session residue\n"),
    )
    .unwrap();
    support::make_private(&residue);

    f.check(
        with_env(
            &[
                "group",
                "close",
                "g-1",
                "--summary",
                "Review complete.",
                "--json",
            ],
            "CLAUDE_CODE_SESSION_ID",
            A,
        ),
        Success,
    );
    // The close's write imported the residue, erased as it was imported.
    assert!(!residue.exists());
    assert!(!f.root().join("pre-sqlite-stale").exists());
    let leftover: i64 = rusqlite::Connection::open(f.root().join(scribe_core::DB_FILE))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM archive_records WHERE instr(record, 'forgotten session residue') > 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(leftover, 0);

    let record = show_group(&f, A, "g-1");
    assert!(record["closed"].is_string(), "{record}");
    assert!(
        record["summary"]
            .as_str()
            .unwrap()
            .starts_with("Review complete."),
        "{record}"
    );
    let lines = group_close_audits(&f);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["status"], "success");
    assert_eq!(lines[0]["target"], "g-1");
    assert_eq!(lines[0]["instruction_session"], A);
    assert!(lines[0]["error_code"].is_null());
}

#[test]
fn close_audit_insert_failure_rolls_back_and_records_the_failure() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    let conn = rusqlite::Connection::open(f.root().join(scribe_core::DB_FILE)).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER abort_group_close_audit BEFORE INSERT ON audit
         WHEN NEW.command = 'group close' AND NEW.status = 'success'
         BEGIN
             SELECT RAISE(ABORT, 'injected audit failure');
         END;",
    )
    .unwrap();
    drop(conn);

    let error = close_with_summary_error(&f);
    assert_eq!(error["code"], "store_database", "{error}");
    let record = show_group(&f, A, "g-1");
    assert!(record["closed"].is_null() && record["summary"].is_null());
    let lines = group_close_audits(&f);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["status"], "failure");
    assert_eq!(lines[0]["target"], "g-1");
    assert_eq!(lines[0]["instruction_session"], A);
    assert_eq!(lines[0]["error_code"], "store_database");
}

#[test]
fn close_cannot_publish_a_summary_after_a_member_erases_its_sources() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    let member = transcript(&f, B, &[row(B, "u", "user", "retained snapshot azalea")]);
    stop(&f, B, &member, &[]);
    let prepared = invoke(&f, A, &["group", "evidence", "g-1"], 0);
    let digest = prepared["evidence_sha256"].as_str().unwrap();
    invoke(&f, B, &["group", "leave"], 0);
    let reply = close_plugin(
        &f,
        "group-close-apply",
        json!({"group":"g-1", "session":A, "provider":"claude-code", "summary":"retained snapshot azalea", "evidence_sha256":digest}),
    );
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error"]["code"], "group_evidence_changed");
    let record = show_group(&f, A, "g-1");
    assert!(record["closed"].is_null() && record["summary"].is_null());
    let close_lines = group_close_audits(&f);
    assert_eq!(close_lines.len(), 1, "{close_lines:?}");
    assert_eq!(close_lines[0]["status"], "failure");
    assert_eq!(close_lines[0]["target"], "g-1");
    assert_eq!(close_lines[0]["error_code"], "group_evidence_changed");
    assert_eq!(close_lines[0]["instruction_session"], A);
}

#[test]
fn codex_group_capture_uses_host_typed_blocks_and_ask_cites_the_stored_turn() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    let out = f.check(
        with_env(
            &[
                "--provider",
                "codex",
                "group",
                "join",
                "g-1",
                "--as",
                "editor",
                "--author",
                "aide",
            ],
            "CODEX_THREAD_ID",
            B,
        ),
        Capture,
    );
    assert!(out.status.success(), "{}", text(&out.stderr));

    let dir = f.home().join(".codex/sessions/2090/11/20");
    f.create_private_dir_all(&dir);
    let path = dir.join(format!("rollout-2090-11-20T01-00-00-{B}.jsonl"));
    let rows = [
        json!({"type":"session_meta","payload":{"id":B,"source":"vscode","thread_source":"user","cwd":f.home()}}),
        json!({"timestamp":"2090-11-20T01:00:00Z","type":"response_item","payload":{
            "type":"message","role":"user","id":"codex-typed",
            "content":[{"type":"input_text","text":"Always label the saffronqueue."},
                       {"type":"input_text","text":"Exclude skill saffronqueue."}],
            "internal_chat_message_metadata_passthrough":{
                "turn_id":"turn-1","content_item_kinds":["user.text","skill"],
                "content_item_metadata":[{"provenance":{"type":"user"}},{"provenance":{"type":"skill"}}]
            }
        }}),
    ];
    std::fs::write(
        &path,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    support::make_private(&path);
    let audit_before = support::audit_lines(&f.root());
    hook(
        &f,
        &["--provider", "codex", "hook", "stop"],
        json!({
            "session_id": B, "transcript_path": path, "hook_event_name":"Stop", "turn_id":"turn-1"
        }),
        &[],
    );
    let after_first = support::audit_lines(&f.root());
    assert_eq!(after_first.len(), audit_before.len() + 1, "{after_first:?}");
    let capture_line = after_first.last().unwrap();
    assert_eq!(capture_line["command"], "hook stop");
    assert_eq!(capture_line["status"], "success");
    assert_eq!(capture_line["target"], format!("session:{B}"));
    assert!(capture_line["error_code"].is_null());
    assert!(!support::audit_text(&f.root()).contains("saffronqueue"));

    // Replaying the unchanged Stop payload writes no second success line.
    hook(
        &f,
        &["--provider", "codex", "hook", "stop"],
        json!({
            "session_id": B, "transcript_path": path, "hook_event_name":"Stop", "turn_id":"turn-1"
        }),
        &[],
    );
    assert_eq!(support::audit_lines(&f.root()), after_first);
}

#[test]
fn consent_belongs_to_the_callers_own_typed_turn() {
    let f = Fixture::new();
    let list = invoke(&f, A, &["group", "list"], 0);
    assert_eq!(
        list,
        json!({"items":[],"total":0,"truncated":false,"warnings":[]})
    );
    assert!(!f.root().exists());
    let mut injected = row(A, "injected", "user", "agent supplied instructions");
    injected["source"] = json!("agent");
    transcript(
        &f,
        A,
        &[
            row(A, "a", "assistant", "the assistant requested this"),
            injected,
            row(B, "copy", "user", "words copied from the parent"),
            row(
                A,
                "u",
                "user",
                "open our release review <pasted_content>private pasted block</pasted_content>",
            ),
        ],
    );
    assert_eq!(invoke(&f, A, OPEN, 1)["code"], "quote_required");
    for quote in [
        "the assistant requested this",
        "agent supplied instructions",
        "words copied from the parent",
        "private pasted block",
    ] {
        assert_eq!(
            invoke(&f, A, &[OPEN, &["--quote", quote]].concat(), 1)["code"],
            "quote_not_found"
        );
    }
    assert_eq!(
        invoke(&f, B, &[OPEN, &["--quote", QUOTE]].concat(), 1)["code"],
        "quote_not_found"
    );
    let group = open(&f);
    assert_eq!(group["id"], "g-1");
    assert_eq!(group["opener"], A);
    assert_eq!(group["project"], "scribe");
    assert_eq!(group["title"], "Release review");
    assert_eq!(group["kind"], "peer review");
    assert_eq!(group["related"], json!(["repo:scribe", "task:example"]));
    assert!(group["opened"].as_str().unwrap().ends_with('Z'));
    assert!(group["closed"].is_null());
    assert_eq!(group["seats"], json!([]));
    let db = rusqlite::Connection::open(f.root().join("scribe.db")).unwrap();
    // Opening stores the opener's own typed turn, under a header-only session
    // row, and nothing a pass would write.
    for (table, expected) in [
        ("sessions", 1),
        ("messages", 1),
        ("claims", 0),
        ("entries", 0),
    ] {
        let count: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, expected, "{table}");
    }
    let audit = support::audit_lines(&f.root());
    let last = audit.last().unwrap();
    assert_eq!(last["target"], "g-1");
    assert_eq!(last["instruction_session"], A);
    assert_eq!(last["instruction_message"], "u");
    assert!(!serde_json::to_string(&audit).unwrap().contains(QUOTE));
    assert_eq!(invoke(&f, A, &["session", "list"], 0)["total"], 0);
}

#[test]
fn a_slash_command_he_typed_opens_a_group_from_its_args() {
    let f = Fixture::new();
    let formation = |extra: Value| {
        let mut row = json!({"sessionId": A, "uuid": "slash", "type": "user",
            "entrypoint": "claude-desktop", "origin": {"kind": "human"},
            "promptSource": "sdk", "turnOrigin": "human",
            "timestamp": "2090-11-20T01:00:00Z",
            "message": {"role": "user", "content": "<command-message>formation</command-message>\n<command-name>/formation</command-name>\n<command-args>duo scribe, open our release review</command-args>"}});
        row.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        row
    };
    transcript(&f, A, &[formation(json!({"isMeta": true}))]);
    assert_eq!(
        invoke(&f, A, &[OPEN, &["--quote", QUOTE]].concat(), 1)["code"],
        "quote_not_found"
    );
    transcript(&f, A, &[formation(json!({}))]);
    assert_eq!(open(&f)["opener"], A);
}

#[test]
fn a_message_he_typed_mid_turn_opens_a_group() {
    let f = Fixture::new();
    let queued = |human_turn: bool| {
        json!({"sessionId": A, "uuid": "queued", "type": "attachment",
            "entrypoint": "claude-desktop", "timestamp": "2090-11-20T01:00:00Z",
            "attachment": {"type": "queued_command",
                "prompt": "can you create the scribe group open our release review",
                "origin": {"kind": "human"}, "humanTurn": human_turn}})
    };
    transcript(&f, A, &[queued(false)]);
    assert_eq!(
        invoke(&f, A, &[OPEN, &["--quote", QUOTE]].concat(), 1)["code"],
        "quote_not_found"
    );
    transcript(&f, A, &[queued(true)]);
    assert_eq!(open(&f)["opener"], A);
}
