//! Session group contracts exercised through the installed command surface.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use support::{Fixture, text};

const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";
const C: &str = "33333333-3333-4333-8333-333333333333";
const D: &str = "44444444-4444-4444-8444-444444444444";
const QUOTE: &str = "open our release review";
const OPEN: &[&str] = &[
    "group",
    "open",
    "--title",
    "Release review",
    "--kind",
    "peer review",
    "--project",
    "scribe",
    "--related",
    "repo:scribe",
    "--related",
    "task:example",
];
const HANDOFF: &str = "## State\nReady\n## In flight\nReview\n## Next\nShip\n## Waiting on you\nNone\n## Do not touch\nArchive\n## Pointers\nREADME.md";

fn row(session: &str, id: &str, role: &str, body: &str) -> Value {
    json!({"sessionId":session,"uuid":id,"type":role,"entrypoint":"cli",
        "timestamp":"2090-11-20T01:00:00Z","message":{"role":role,"content":body}})
}

fn transcript_path(f: &Fixture, session: &str) -> PathBuf {
    let dir = f.home().join(".claude/projects/-test");
    f.create_private_dir_all(&dir);
    dir.join(format!("{session}.jsonl"))
}

fn transcript(f: &Fixture, session: &str, rows: &[Value]) -> PathBuf {
    let path = transcript_path(f, session);
    std::fs::write(
        &path,
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    support::make_private(&path);
    path
}

fn cleared_transcript(f: &Fixture, session: &str, launch: &str, clears: &[&str]) -> PathBuf {
    let mut sessions = vec![json!({"via":"launch","id":launch})];
    sessions.extend(clears.iter().map(|id| json!({"via":"clear","id":id})));
    transcript(
        f,
        session,
        &[json!({"type":"derived-from","sessions":sessions})],
    )
}

fn session_start(
    f: &Fixture,
    session: &str,
    path: Option<&PathBuf>,
    source: &str,
    entrypoint: &str,
) -> Value {
    session_start_with_cwd(f, session, path, source, entrypoint, None)
}

fn session_start_with_cwd(
    f: &Fixture,
    session: &str,
    path: Option<&PathBuf>,
    source: &str,
    entrypoint: &str,
    cwd: Option<&PathBuf>,
) -> Value {
    let mut payload = json!({
        "session_id": session,
        "hook_event_name": "SessionStart",
        "source": source
    });
    if let Some(path) = path {
        payload["transcript_path"] = json!(path);
    }
    if let Some(cwd) = cwd {
        payload["cwd"] = json!(cwd.to_string_lossy());
    }
    let mut cmd = f.cmd(&["hook", "session-start"]);
    cmd.env("CLAUDE_CODE_ENTRYPOINT", entrypoint)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    if out.stdout.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

fn invoke(f: &Fixture, session: &str, args: &[&str], code: i32) -> Value {
    let out = f
        .cmd(args)
        .arg("--json")
        .env("CLAUDE_CODE_SESSION_ID", session)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(code),
        "{args:?}: {}",
        text(&out.stderr)
    );
    serde_json::from_slice(if code == 0 { &out.stdout } else { &out.stderr }).unwrap()
}

fn open(f: &Fixture) -> Value {
    invoke(f, A, &[OPEN, &["--quote", QUOTE]].concat(), 0)
}

fn join(f: &Fixture, session: &str, id: &str, extra: &[&str], code: i32) -> Value {
    invoke(
        f,
        session,
        &[
            &["group", "join", id, "--as", "editor", "--author", "aide"][..],
            extra,
        ]
        .concat(),
        code,
    )
}

fn hook(f: &Fixture, args: &[&str], payload: Value, env: &[(&str, &str)]) -> std::process::Output {
    let mut child = f
        .cmd(args)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    out
}

fn stop(f: &Fixture, session: &str, path: &PathBuf, env: &[(&str, &str)]) {
    hook(
        f,
        &["hook", "stop"],
        json!({"session_id":session,"transcript_path":path}),
        env,
    );
}

#[test]
fn member_capture_is_model_free_and_leave_and_forget_erase_its_turns() {
    let f = Fixture::new();
    let opener = transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    let mut before_join = row(B, "pre", "user", "prejoinorchid");
    before_join["timestamp"] = json!("2000-01-01T00:00:00Z");
    let mut injected = row(B, "injected", "user", "agentorchid");
    injected["turnOrigin"] = json!("agent");
    let turns = [
        before_join,
        row(
            B,
            "typed",
            "user",
            "Keep the violetorchid with groupsecretvalue\n<pasted_content>pasteorchid</pasted_content>\n```\nfenceorchid\n```",
        ),
        row(B, "finance", "user", "Save my bank account financeorchid"),
        row(B, "control", "user", "/scribe status\ncontrolorchid"),
        row(B, "assistant", "assistant", "assistantorchid"),
        row(
            B,
            "synthetic",
            "user",
            "<task-notification>syntheticorchid</task-notification>",
        ),
        row(A, "copied", "user", "copiedorchid"),
        injected,
    ];
    let member = transcript(&f, B, &turns);
    f.config("[redact]\nenv = [\"GROUP_SECRET\"]\n[catalog]\nevery_turns = 1\n");
    let path = f.fake_program(
        "claude",
        "#!/bin/sh\ntouch \"$HOME/model-called\"\nexit 1\n",
    );
    let env = [
        ("GROUP_SECRET", "groupsecretvalue"),
        ("PATH", path.to_str().unwrap()),
    ];
    let audit_before = support::audit_lines(&f.root());
    stop(&f, B, &member, &env);
    let after_first = support::audit_lines(&f.root());
    assert_eq!(after_first.len(), audit_before.len() + 1, "{after_first:?}");
    let capture_line = after_first.last().unwrap();
    assert_eq!(capture_line["command"], "hook stop");
    assert_eq!(capture_line["status"], "success");
    assert_eq!(capture_line["target"], format!("session:{B}"));
    assert!(capture_line["error_code"].is_null());
    let log_text = support::audit_text(&f.root());
    assert!(!log_text.contains("violetorchid"));
    assert!(!log_text.contains("groupsecretvalue"));
    assert!(!log_text.contains("GROUP_SECRET"));

    stop(&f, B, &member, &env);
    assert_eq!(support::audit_lines(&f.root()), after_first);
    assert!(!f.home().join("model-called").exists());
    assert!(support::control(&f.root())["sessions"][B].is_null());
    assert_eq!(
        invoke(&f, B, &["session", "list", "--group", "g-1"], 0)["total"],
        0
    );
    let messages = |term| {
        invoke(
            &f,
            B,
            &["search", term, "--kind", "message", "--group", "g-1"],
            0,
        )
    };
    let hit = messages("violetorchid");
    assert_eq!(hit["total"], 1);
    assert_eq!(hit["hits"][0]["cite"], format!("session:{B}"));
    let snippet = hit["hits"][0]["snippet"].as_str().unwrap();
    assert!(snippet.contains("[redacted:GROUP_SECRET]"), "{snippet}");
    for word in [
        "prejoinorchid",
        "pasteorchid",
        "fenceorchid",
        "financeorchid",
        "controlorchid",
        "assistantorchid",
        "syntheticorchid",
        "copiedorchid",
        "agentorchid",
        "groupsecretvalue",
    ] {
        assert_eq!(messages(word)["total"], 0, "{word}");
    }

    invoke(&f, B, &["group", "leave"], 0);
    assert_eq!(
        invoke(&f, B, &["search", "violetorchid", "--kind", "message"], 0)["total"],
        0
    );
    // A second member has no catalog record either, and the existing forget
    // still erases the data and prevents the next Stop from restoring it.
    f.config("");
    join(&f, C, "g-1", &[], 0);
    let member = transcript(&f, C, &[row(C, "c", "user", "remember cobaltazalea")]);
    stop(&f, C, &member, &env);
    assert_eq!(messages("cobaltazalea")["total"], 1);
    let out = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({
            "session_id": A, "transcript_path": opener, "cwd": f.home(),
            "prompt": format!("#scribe forget {C}")
        }),
        &env,
    );
    assert!(
        text(&out.stdout).contains("forgot"),
        "{}",
        text(&out.stdout)
    );
    stop(&f, C, &member, &env);
    assert_eq!(messages("cobaltazalea")["total"], 0);
}

#[test]
fn group_filters_apply_before_limits_ranking_and_lookups() {
    const MEMBER: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
    let f = Fixture::new();
    f.seed();
    f.config("");
    let entry_path = f.root().join("entries/decisions/decisions-2.md");
    let entry = std::fs::read_to_string(&entry_path).unwrap();
    let entry = entry.replace(
        &format!("session: {MEMBER}, message: null"),
        &format!("session: {C}, message: null"),
    );
    assert!(entry.contains(&format!("session: {C}, message: null")));
    std::fs::write(&entry_path, entry).unwrap();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, MEMBER, "g-1", &[], 0);
    invoke(
        &f,
        MEMBER,
        &[
            "entry",
            "update",
            "decisions-2",
            "--author",
            "aide",
            "--add-related",
            &format!("session:{MEMBER}"),
        ],
        0,
    );
    let entries = invoke(
        &f,
        A,
        &[
            "entry",
            "list",
            "--tag",
            "decisions",
            "--limit",
            "1",
            "--group",
            "g-1",
        ],
        0,
    );
    assert_eq!(entries["total"], 1);
    assert_eq!(entries["entries"][0]["id"], "decisions-2");
    let sessions = invoke(
        &f,
        A,
        &["session", "list", "--limit", "1", "--group", "g-1"],
        0,
    );
    assert_eq!(sessions["total"], 1);
    assert_eq!(sessions["sessions"][0]["id"], MEMBER);
    let found = invoke(
        &f,
        A,
        &["search", "merge", "--group", "g-1", "--include-hidden"],
        0,
    );
    assert!(found["total"].as_u64().unwrap() > 1);
    assert!(found["hits"].as_array().unwrap().iter().all(|hit| {
        hit["cite"] == "decisions-2"
            || hit["cite"]
                .as_str()
                .unwrap()
                .starts_with(&format!("session:{MEMBER}"))
    }));
    assert!(
        found["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|hit| hit["cite"] != "decisions-1")
    );
    let model = f.fake_program(
        "claude",
        "#!/bin/sh\ncat > \"$HOME/ask-prompt\"\nprintf '%s\\n' '{\"type\":\"result\",\"is_error\":false,\"structured_output\":{\"status\":\"no_record\",\"answer\":\"\",\"claims\":[],\"conflicts\":[]}}'\n",
    );
    for prefix in [vec!["ask"], vec!["question", "ask"]] {
        let args = [
            prefix.as_slice(),
            &[
                "squash merge",
                "--kind",
                "entry",
                "--group",
                "g-1",
                "--json",
            ],
        ]
        .concat();
        let out = f
            .cmd(&args)
            .env("CLAUDE_CODE_SESSION_ID", A)
            .env("PATH", &model)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(result["evidence_considered"], 1);
        let prompt = std::fs::read_to_string(f.home().join("ask-prompt")).unwrap();
        assert!(prompt.contains("cite=\"decisions-2\""), "{prompt}");
        assert!(!prompt.contains("cite=\"decisions-1\""), "{prompt}");
    }
    for args in [vec!["ask"], vec!["question", "ask"]] {
        let result = invoke(
            &f,
            A,
            &[args.as_slice(), &["last session", "--group", "g-1"]].concat(),
            0,
        );
        assert_eq!(result["status"], "record");
        assert_eq!(result["records"][0]["cite"], format!("session:{MEMBER}"));
    }
    assert_eq!(
        invoke(&f, A, &["search", "merge", "--group", "g-99"], 1)["code"],
        "group_not_found"
    );
}

#[test]
fn leaving_preserves_a_turn_independently_written_by_a_pass() {
    const MEMBER: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
    let f = Fixture::new();
    f.seed();
    f.config("");
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, MEMBER, "g-1", &[], 0);
    let member = transcript(
        &f,
        MEMBER,
        &[
            row(MEMBER, "kept", "user", "preserve copperlilac"),
            row(MEMBER, "erased", "user", "erase copperlilac"),
        ],
    );
    stop(&f, MEMBER, &member, &[]);
    let search = |extra: &[&str]| {
        let args = [&["search", "copperlilac", "--kind", "message"][..], extra].concat();
        invoke(&f, A, &args, 0)
    };
    assert_eq!(search(&["--group", "g-1"])["total"], 2);
    scribe_core::Store::open(f.root())
        .write_messages(
            &[scribe_core::message::NewMessage {
                session: MEMBER.parse().unwrap(),
                message: "kept".into(),
                role: scribe_core::Speaker::User,
                at: time::macros::datetime!(2090-11-20 1:00 UTC),
                text: "preserve copperlilac".into(),
            }],
            &scribe_core::Redactor::default(),
        )
        .unwrap();
    invoke(&f, MEMBER, &["group", "leave"], 0);
    assert_eq!(search(&["--group", "g-1"])["total"], 0);
    let independent = search(&[]);
    assert_eq!(independent["total"], 1);
    assert_eq!(independent["hits"][0]["message"], "kept");
}

#[test]
fn opener_closes_with_one_bounded_summary_and_the_project_brief_shows_it() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    let private = "ghp_abcdefghijklmnopqrstuvwxyzABCDEFGHIJ";
    let member = transcript(
        &f,
        B,
        &[row(
            B,
            "u",
            "user",
            &format!(
                "{QUOTE} sprucechecklist {private} {}",
                "longword ".repeat(100)
            ),
        )],
    );
    stop(&f, B, &member, &[]);
    let handoff = invoke(
        &f,
        B,
        &[
            "entry",
            "add",
            HANDOFF,
            "--tag",
            "handoff",
            "--author",
            "aide",
            "--related",
            &format!("session:{B}"),
            "--quote",
            QUOTE,
        ],
        0,
    );
    let path = f.fake_program("claude", &format!(r#"#!/bin/sh
echo call >> "$HOME/model-calls"
cat > "$HOME/model-prompt"
printf '%s\n' '{{"type":"result","is_error":false,"structured_output":{{"summary":"Review complete. {private}"}}}}'
"#));
    let close = |session| {
        f.cmd(&["group", "close", "g-1", "--json"])
            .env("CLAUDE_CODE_SESSION_ID", session)
            .env("PATH", &path)
            .output()
            .unwrap()
    };
    let denied = close(B);
    assert_eq!(denied.status.code(), Some(1));
    assert!(text(&denied.stderr).contains("only the opening session"));
    assert!(!f.home().join("model-calls").exists());
    let after_denied: Vec<_> = support::audit_lines(&f.root())
        .into_iter()
        .filter(|line| line["command"] == "group close")
        .collect();
    assert_eq!(after_denied.len(), 1, "{after_denied:?}");
    assert_eq!(after_denied[0]["status"], "failure");
    assert_eq!(after_denied[0]["target"], "g-1");
    assert_eq!(after_denied[0]["error_code"], "group_membership");
    assert_eq!(after_denied[0]["instruction_session"], B);

    for _ in 0..2 {
        let out = close(A);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let calls = std::fs::read_to_string(f.home().join("model-calls")).unwrap();
    assert_eq!(calls.lines().count(), 1);
    let close_successes: Vec<_> = support::audit_lines(&f.root())
        .into_iter()
        .filter(|line| line["command"] == "group close" && line["status"] == "success")
        .collect();
    assert_eq!(close_successes.len(), 1, "{close_successes:?}");
    assert_eq!(close_successes[0]["target"], "g-1");
    assert!(close_successes[0]["error_code"].is_null());
    assert!(close_successes[0]["author"].is_null());
    assert_eq!(close_successes[0]["instruction_session"], A);
    assert!(close_successes[0]["instruction_message"].is_null());
    let audit_text = support::audit_text(&f.root());
    assert!(!audit_text.contains("Review complete."));
    assert!(!audit_text.contains("sprucechecklist"));
    assert!(!audit_text.contains(private));
    let prompt = std::fs::read_to_string(f.home().join("model-prompt")).unwrap();
    assert!(prompt.contains(handoff["id"].as_str().unwrap()), "{prompt}");
    assert!(prompt.contains("sprucechecklist") && prompt.contains("Review"));
    assert!(!prompt.contains(private));
    assert!(
        prompt.len() < 2500,
        "unexpectedly long prompt: {}",
        prompt.len()
    );
    let shown = invoke(&f, A, &["group", "show", "g-1"], 0);
    assert!(!shown["closed"].is_null());
    let summary = shown["summary"].as_str().unwrap();
    assert!(summary.contains("Review complete."));
    assert!(!summary.contains(private));
    assert!(summary.contains("task:example"));
    let report = invoke(&f, A, &["brief", "--project", "scribe"], 0);
    assert_eq!(report["last_group"]["id"], "g-1");
    assert_eq!(report["last_group"]["summary"], summary);
    assert!(text(&f.ok(&["brief", "--project", "scribe"]).stdout).contains(summary));
    let member = transcript(&f, B, &[row(B, "after", "user", "aftercloseorchid")]);
    stop(&f, B, &member, &[]);
    assert_eq!(
        invoke(
            &f,
            A,
            &[
                "search",
                "aftercloseorchid",
                "--kind",
                "message",
                "--group",
                "g-1"
            ],
            0
        )["total"],
        0
    );
    let left = invoke(&f, B, &["group", "leave"], 0);
    assert!(left["summary"].is_null());
    assert!(!left["closed"].is_null());
}

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

    let path = f.fake_program(
        "claude",
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"result\",\"is_error\":false,\"structured_output\":{\"summary\":\"Retained group summary.\"}}'\n",
    );
    let closed = f
        .cmd(&["group", "close", "g-1", "--json"])
        .env("CLAUDE_CODE_SESSION_ID", A)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(closed.status.code(), Some(0), "{}", text(&closed.stderr));
    let group = invoke(&f, A, &["group", "show", "g-1"], 0);
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
    let out = f
        .cmd(&args)
        .env("CLAUDE_CODE_SESSION_ID", A)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("g-1 is still open"));
    let mut other = args;
    let project = other.iter().position(|s| *s == "--project").unwrap();
    other[project + 1] = "other";
    let out = f
        .cmd(&other)
        .env("CLAUDE_CODE_SESSION_ID", A)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
    assert_eq!(
        invoke(&f, A, &["brief", "--project", "scribe"], 0)["last_group"]["id"],
        "g-2"
    );
}

#[test]
fn close_cannot_publish_a_summary_after_a_member_erases_its_sources() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    let member = transcript(&f, B, &[row(B, "u", "user", "retained snapshot azalea")]);
    stop(&f, B, &member, &[]);
    let mut leave = f.cmd(&["group", "leave"]);
    leave.env("CLAUDE_CODE_SESSION_ID", B);
    let hold = support::Hold::new(&f.home(), move || {
        let out = leave.output().unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
    });
    let path = f.fake_program("claude", &format!(
        "#!/bin/sh\ncat > \"$HOME/model-prompt\"\n{}\nprintf '%s\\n' '{{\"type\":\"result\",\"is_error\":false,\"structured_output\":{{\"summary\":\"retained snapshot azalea\"}}}}'\n",
        hold.script()
    ));
    let out = f
        .cmd(&["group", "close", "g-1", "--json"])
        .env("CLAUDE_CODE_SESSION_ID", A)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("group evidence changed"),
        "{}",
        text(&out.stderr)
    );
    hold.join();
    let record = invoke(&f, A, &["group", "show", "g-1"], 0);
    assert!(record["closed"].is_null() && record["summary"].is_null());
    let close_lines: Vec<_> = support::audit_lines(&f.root())
        .into_iter()
        .filter(|line| line["command"] == "group close")
        .collect();
    assert_eq!(close_lines.len(), 1, "{close_lines:?}");
    assert_eq!(close_lines[0]["status"], "failure");
    assert_eq!(close_lines[0]["target"], "g-1");
    assert_eq!(close_lines[0]["error_code"], "group_membership");
    assert_eq!(close_lines[0]["instruction_session"], A);
}

#[test]
fn codex_group_capture_uses_host_typed_blocks_and_ask_cites_the_stored_turn() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    let out = f
        .cmd(&[
            "--provider",
            "codex",
            "group",
            "join",
            "g-1",
            "--as",
            "editor",
            "--author",
            "aide",
        ])
        .env("CODEX_THREAD_ID", B)
        .output()
        .unwrap();
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
    let model = f.fake_program("claude", &format!(
        "#!/bin/sh\ncat > \"$HOME/ask-prompt\"\nprintf '%s\\n' '{{\"type\":\"result\",\"is_error\":false,\"structured_output\":{{\"status\":\"answered\",\"answer\":\"Label the queue.\",\"claims\":[{{\"text\":\"label it\",\"quote\":\"Always label the saffronqueue.\",\"cite\":\"session:{B}#message:codex-typed\"}}],\"conflicts\":[]}}}}'\n"
    ));
    let out = f
        .cmd(&[
            "ask",
            "how should we handle saffronqueue?",
            "--group",
            "g-1",
            "--json",
        ])
        .env("PATH", &model)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["status"], "answered", "{result}");
    assert_eq!(
        result["claims"][0]["quote"],
        "Always label the saffronqueue."
    );
    let prompt = std::fs::read_to_string(f.home().join("ask-prompt")).unwrap();
    assert!(!prompt.contains("Exclude skill"));
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
    for table in ["sessions", "messages", "claims", "entries"] {
        let count: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
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
fn seat_chains_replacement_handoffs_and_mistaken_joins() {
    let f = Fixture::new();
    for id in [A, B] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    open(&f);
    open(&f);
    join(&f, A, "g-1", &["--role", "copy editor"], 0);
    join(&f, A, "g-1", &[], 0);
    assert_eq!(
        join(&f, A, "g-2", &["--replace"], 1)["code"],
        "group_membership"
    );
    assert_eq!(join(&f, B, "g-1", &[], 1)["code"], "seat_occupied");
    let handoff = |session| {
        invoke(
            &f,
            session,
            &[
                "entry",
                "add",
                &format!("{HANDOFF}\nSession {session}"),
                "--tag",
                "handoff",
                "--author",
                "aide",
                "--related",
                &format!("session:{session}"),
                "--quote",
                QUOTE,
            ],
            0,
        )["id"]
            .clone()
    };
    let first = handoff(A);
    let group = join(&f, B, "g-1", &["--replace"], 0);
    assert_eq!(
        group["seats"],
        json!([{"name":"editor","role":"copy editor","transcripts":[A,B],"newest_handoff":first}])
    );
    let second = handoff(B);
    let before = support::audit_lines(&f.root());
    assert!(before.iter().any(|r| r["command"] == "group join"
        && r["author"] == "aide"
        && r["error_code"] == "seat_occupied"));
    let shown = invoke(&f, A, &["group", "show", "g-1"], 0);
    let list = invoke(&f, A, &["group", "list"], 0);
    assert_eq!(list["items"][1], shown);
    assert_eq!(shown["seats"][0]["newest_handoff"], second);
    assert_eq!(support::audit_lines(&f.root()), before);
    // Historical entries may have subsecond timestamps. Pick by the same
    // instant and numeric id ordering as session show, including across seats' holders.
    support::sql(&f.root(), "UPDATE entries SET created = CASE n
        WHEN 1 THEN '2026-11-22T19:00:00.000002Z' ELSE '2026-11-22T19:00:00.000001Z' END WHERE tag='handoff';");
    assert_eq!(
        invoke(&f, A, &["group", "show", "g-1"], 0)["seats"][0]["newest_handoff"],
        first
    );
    let after = invoke(&f, B, &["group", "leave"], 0);
    assert_eq!(after["seats"][0]["transcripts"], json!([A]));
    assert_eq!(after["seats"][0]["newest_handoff"], first);
    join(&f, B, "g-2", &[], 0);
    assert_eq!(invoke(&f, A, &["group", "leave"], 0)["seats"], json!([]));
    assert_eq!(
        invoke(&f, C, &["group", "leave"], 1)["code"],
        "group_membership"
    );
}

#[test]
fn joins_after_an_ended_transcript_and_keeps_unverifiable_seats_occupied() {
    let f = Fixture::new();
    let mut message = row(A, "u", "user", QUOTE);
    message["timestamp"] = json!("2000-01-01T00:00:00Z");
    transcript(&f, A, &[message]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    let group = join(&f, B, "g-1", &[], 0);
    assert_eq!(group["seats"][0]["transcripts"], json!([A, B]));
    assert_eq!(join(&f, C, "g-1", &[], 1)["code"], "seat_occupied");
    let export = f.elsewhere("dump");
    f.ok(&["export", export.to_str().unwrap(), "--as", "jsonl"]);
    let members = std::fs::read_to_string(export.join("group_members.jsonl")).unwrap();
    assert_eq!(members.lines().count(), 2);
    assert!(export.join("groups.jsonl").is_file() && export.join("group_seats.jsonl").is_file());
}

#[test]
fn old_schema_reads_empty_without_migration_then_upgrades_on_open() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    f.ok(&["store", "migrate"]);
    support::remove_group_capture_schema(&f.root());
    support::sql(
        &f.root(),
        "DROP TABLE group_members; DROP TABLE group_seats; DROP TABLE groups; PRAGMA user_version=8;",
    );
    assert_eq!(invoke(&f, A, &["group", "list"], 0)["total"], 0);
    let conn = rusqlite::Connection::open(f.root().join("scribe.db")).unwrap();
    let version = || {
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(version(), 8);
    open(&f);
    assert_eq!(version(), 14);
}

#[test]
fn existing_members_start_group_capture_at_the_schema_upgrade() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, B, "g-1", &[], 0);
    support::remove_group_capture_schema(&f.root());
    support::sql(&f.root(), "PRAGMA user_version = 10");
    let old = invoke(&f, A, &["group", "show", "g-1"], 0);
    assert!(old["summary"].is_null());
    assert_eq!(old["seats"][0]["transcripts"], json!([B]));
    let report = invoke(&f, A, &["brief", "--project", "scribe"], 0);
    assert_eq!(report["last_group"]["id"], "g-1");

    open(&f);
    let mut old_turn = row(B, "old", "user", "beforeupgradejuniper");
    old_turn["timestamp"] = json!("2000-01-01T00:00:00Z");
    let path = transcript(
        &f,
        B,
        &[old_turn, row(B, "new", "user", "afterupgradejuniper")],
    );
    stop(&f, B, &path, &[]);
    let messages = |word| {
        invoke(
            &f,
            A,
            &["search", word, "--kind", "message", "--group", "g-1"],
            0,
        )
    };
    assert_eq!(messages("beforeupgradejuniper")["total"], 0);
    assert_eq!(messages("afterupgradejuniper")["total"], 1);
}

#[test]
fn desktop_clear_inherits_group_and_seat_without_typing_or_rerunning() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &["--role", "reviewer"], 0);

    // Non-interactive, resume, compact, and mismatched transcripts do not inherit.
    let child_b = cleared_transcript(&f, B, C, &[A]);
    for (source, entrypoint) in [
        ("startup", "sdk-cli"),
        ("resume", "claude-desktop"),
        ("compact", "claude-desktop"),
    ] {
        session_start(&f, B, Some(&child_b), source, entrypoint);
    }
    let mismatched = transcript_path(&f, C);
    std::fs::copy(&child_b, &mismatched).unwrap();
    session_start(&f, B, Some(&mismatched), "startup", "claude-desktop");
    assert_eq!(
        invoke(&f, A, &["group", "show", "g-1"], 0)["seats"][0]["transcripts"],
        json!([A])
    );

    // Desktop clear (`source: startup`) appends B to A's seat with nothing typed.
    let before = support::audit_lines(&f.root()).len();
    let reply = session_start(&f, B, Some(&child_b), "startup", "claude-desktop");
    assert!(reply.get("systemMessage").is_none(), "{reply}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("editor") && context.contains("reviewer") && context.contains("g-1"),
        "{context}"
    );
    assert!(!context.contains("scribe group join"), "{context}");
    assert_eq!(
        invoke(&f, B, &["group", "show", "g-1"], 0)["seats"][0]["transcripts"],
        json!([A, B])
    );
    let audit = support::audit_lines(&f.root());
    assert_eq!(audit.len(), before + 1);
    assert_eq!(audit[before]["command"], "hook session-start");
    assert_eq!(audit[before]["target"], "g-1");
    assert!(audit[before]["error_code"].is_null());
    assert_eq!(invoke(&f, B, &["session", "list"], 0)["total"], 0);

    // Repeating SessionStart for B is idempotent and writes no second audit line.
    let repeat = session_start(&f, B, Some(&child_b), "startup", "claude-desktop");
    assert_eq!(support::audit_lines(&f.root()).len(), before + 1);
    assert!(
        !repeat["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("scribe group join")
    );

    // A second clear (C cleared from B, still carrying A as launch ancestor) appends C too.
    let child_c = cleared_transcript(&f, C, A, &[B]);
    session_start(&f, C, Some(&child_c), "startup", "claude-desktop");
    assert_eq!(
        invoke(&f, C, &["group", "show", "g-1"], 0)["seats"][0]["transcripts"],
        json!([A, B, C])
    );
}

#[test]
fn desktop_clear_with_continue_handoff_prepends_handoff_and_inherits_seat() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &["--role", "reviewer"], 0);
    invoke(
        &f,
        A,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "aide",
            "--related",
            &format!("session:{A}"),
            "--mode",
            "continue",
            "--quote",
            QUOTE,
            &format!("{HANDOFF}\nCarry on."),
        ],
        0,
    );
    let child_b = cleared_transcript(&f, B, C, &[A]);
    let reply = session_start(&f, B, Some(&child_b), "startup", "claude-desktop");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.starts_with("## State\nReady"), "{context}");
    assert!(
        context.contains("editor") && context.contains("g-1"),
        "{context}"
    );
    assert_eq!(
        invoke(&f, B, &["group", "show", "g-1"], 0)["seats"][0]["transcripts"],
        json!([A, B])
    );

    let store = scribe_core::Store::open(f.root());
    let parent = A.parse().unwrap();
    let successor = B.parse().unwrap();
    store
        .update_control(|state| {
            scribe_core::control::switch(
                state,
                &parent,
                &scribe_core::control::Switch::On,
                time::OffsetDateTime::now_utc(),
            )
        })
        .unwrap();
    let now = time::OffsetDateTime::now_utc();
    store
        .link_session(
            &successor,
            &scribe_core::SessionLink {
                kind: scribe_core::LinkType::Continues,
                session: parent,
            },
            true,
            now,
        )
        .unwrap();
    let archive = f.root().join("pre-sqlite-session-forget-test");
    f.ok(&["export", archive.to_str().unwrap()]);
    let archived_handoff = archive.join("entries/handoff/handoff-1.md");
    assert!(archived_handoff.is_file());
    let before = std::fs::read(&child_b).unwrap();
    let reply = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id": B, "transcript_path": child_b, "cwd": f.home(),
            "prompt": format!("#scribe forget {A}")}),
        &[],
    );
    assert!(text(&reply.stdout).contains("provider transcript was left untouched"));
    assert_eq!(std::fs::read(&child_b).unwrap(), before);
    assert!(!archived_handoff.exists());
    assert_eq!(
        invoke(&f, B, &["entry", "show", "handoff-1"], 1)["code"],
        "entry_not_found"
    );
    let group = invoke(&f, B, &["group", "show", "g-1"], 0);
    let seat = &group["seats"][0];
    assert_eq!(seat["transcripts"], json!([B]), "{group}");
    assert_eq!(seat["role"], "reviewer");
    assert!(seat["handoff"].is_null(), "{group}");
    assert!(
        store
            .control()
            .unwrap()
            .get(&successor)
            .unwrap()
            .pending_links
            .is_empty()
    );
    let ask = invoke(&f, B, &["ask", "last session", "--group", "g-1"], 0);
    assert!(!ask.to_string().contains("Carry on."), "{ask}");
    let brief = invoke(&f, B, &["brief", "--project", "scribe"], 0);
    assert!(!brief.to_string().contains("Carry on."), "{brief}");
}

#[test]
fn missing_start_transcript_recovers_at_first_prompt_once() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &["--role", "reviewer"], 0);
    invoke(
        &f,
        A,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "aide",
            "--related",
            &format!("session:{A}"),
            "--mode",
            "continue",
            "--quote",
            QUOTE,
            &format!("{HANDOFF}\nCarry on."),
        ],
        0,
    );
    let path = transcript_path(&f, B);
    let start = session_start(&f, B, Some(&path), "startup", "claude-desktop");
    let context = start["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("transcript not written yet"), "{start}");
    assert!(!context.contains("no predecessor line"), "{start}");
    transcript(
        &f,
        B,
        &[
            json!({"type":"tools-used"}),
            json!({"type":"derived-from","sessions":[{"id":C,"via":"launch"},{"id":A,"via":"clear"}]}),
        ],
    );
    let payload = json!({"session_id":B,"transcript_path":path,"cwd":f.home(),"prompt":"continue from handoff"});
    let result = hook(
        &f,
        &["hook", "user-prompt-submit"],
        payload.clone(),
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    let reply: Value = serde_json::from_slice(&result.stdout).unwrap();
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.starts_with("## State\nReady") && context.contains("inherited from"),
        "{reply}"
    );
    assert_eq!(
        invoke(&f, B, &["group", "show", "g-1"], 0)["seats"][0]["transcripts"],
        json!([A, B])
    );
    let again = hook(
        &f,
        &["hook", "user-prompt-submit"],
        payload,
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    );
    assert!(again.stdout.is_empty(), "{}", text(&again.stdout));
}

#[test]
fn missing_or_non_member_predecessor_joins_nothing_and_tells_session_to_rerun_join() {
    let f = Fixture::new();
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    let before = support::audit_lines(&f.root());

    // Missing derived-from line (and missing transcript file).
    let no_ancestry = transcript(&f, B, &[row(B, "u", "user", "fresh")]);
    for (source, path) in [
        ("startup", Some(&no_ancestry)),
        ("clear", Some(&no_ancestry)),
        ("startup", None),
    ] {
        let reply = session_start(&f, B, path, source, "claude-desktop");
        let context = reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or_default();
        assert!(context.contains("scribe group join"), "{reply}");
    }

    // Predecessor line naming a session in no group.
    let non_member = cleared_transcript(&f, C, A, &[D]);
    let reply = session_start(&f, C, Some(&non_member), "startup", "claude-desktop");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(context.contains("scribe group join"), "{reply}");

    // Predecessor line naming an earlier holder already replaced on the seat.
    join(&f, D, "g-1", &["--replace"], 0);
    let replaced_before = support::audit_lines(&f.root());
    let stale_child = cleared_transcript(&f, "55555555-5555-4555-8555-555555555555", C, &[A]);
    let stale_reply = session_start(
        &f,
        "55555555-5555-4555-8555-555555555555",
        Some(&stale_child),
        "startup",
        "claude-desktop",
    );
    assert!(
        stale_reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or_default()
            .contains("scribe group join"),
        "{stale_reply}"
    );
    assert_eq!(support::audit_lines(&f.root()), replaced_before);
    assert_eq!(
        invoke(&f, A, &["group", "show", "g-1"], 0)["seats"][0]["transcripts"],
        json!([A, D])
    );
    assert!(before.len() < replaced_before.len());
}

#[test]
fn group_join_accepts_cleared_predecessor_without_replace_and_refuses_others() {
    let f = Fixture::new();
    // Recent timestamp (year 2090): A has NOT ended under the 24-hour rule.
    transcript(&f, A, &[row(A, "u", "user", QUOTE)]);
    open(&f);
    join(&f, A, "g-1", &["--role", "reviewer"], 0);

    // B's transcript names A as its cleared predecessor -> join succeeds without --replace.
    cleared_transcript(&f, B, C, &[A]);
    let joined = join(&f, B, "g-1", &[], 0);
    assert_eq!(joined["seats"][0]["role"], "reviewer");
    assert_eq!(joined["seats"][0]["transcripts"], json!([A, B]));

    // C has no predecessor line -> refused with seat_occupied without --replace.
    transcript(&f, C, &[row(C, "u", "user", "unrelated")]);
    assert_eq!(join(&f, C, "g-1", &[], 1)["code"], "seat_occupied");

    // D's predecessor is A (no longer the seat's newest transcript B) -> refused with seat_occupied.
    cleared_transcript(&f, D, C, &[A]);
    assert_eq!(join(&f, D, "g-1", &[], 1)["code"], "seat_occupied");
}

/// Count of membership rows; a replaced session keeps its row, so the
/// count only grows by real joins.
fn member_rows(f: &Fixture) -> i64 {
    rusqlite::Connection::open_with_flags(
        f.root().join("scribe.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row("SELECT COUNT(*) FROM group_members", [], |r| r.get(0))
    .unwrap()
}

#[test]
fn restarting_a_replaced_session_names_no_seat_and_writes_nothing() {
    let f = Fixture::new();
    for id in [A, B, C] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    open(&f);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    join(&f, B, "g-1", &["--replace"], 0);
    join(&f, C, "g-2", &[], 0);
    let before = support::audit_lines(&f.root());
    assert_eq!(member_rows(&f), 3);

    // A restarts on its own transcript with no predecessor line: B holds
    // the seat now, so A must not be reported as holding it.
    let a_path = transcript_path(&f, A);
    let reply = session_start(&f, A, Some(&a_path), "startup", "claude-desktop");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(!context.contains("holds seat"), "{reply}");
    assert!(context.contains("was replaced"), "{reply}");
    assert!(!context.contains("scribe group join"), "{reply}");

    // A's cleared predecessor C holds an open seat in g-2. A has a historical
    // row, so it must not be inserted again (the session is unique).
    let cleared = cleared_transcript(&f, A, D, &[C]);
    let reply = session_start(&f, A, Some(&cleared), "startup", "claude-desktop");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(!context.contains("holds seat"), "{reply}");
    assert!(context.contains("was replaced"), "{reply}");
    assert!(!context.contains("scribe group join"), "{reply}");
    assert_eq!(member_rows(&f), 3);
    assert_eq!(support::audit_lines(&f.root()), before);
    assert_eq!(
        invoke(&f, A, &["group", "show", "g-1"], 0)["seats"][0]["transcripts"],
        json!([A, B])
    );
    assert_eq!(
        invoke(&f, A, &["group", "show", "g-2"], 0)["seats"][0]["transcripts"],
        json!([C])
    );

    // B restarting still identifies itself as the holder, with no write.
    let b_path = transcript_path(&f, B);
    let reply = session_start(&f, B, Some(&b_path), "startup", "claude-desktop");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(context.contains("holds seat editor"), "{reply}");
    assert_eq!(member_rows(&f), 3);
    assert_eq!(support::audit_lines(&f.root()), before);
}

#[test]
fn replaced_member_stores_no_new_turns_and_its_history_stays_readable() {
    let f = Fixture::new();
    let a_before = [
        row(A, "u", "user", QUOTE),
        row(A, "before", "user", "beforeorchid typed by A"),
    ];
    let a_path = transcript(&f, A, &a_before);
    let b_path = transcript(&f, B, &[row(B, "u", "user", "joining now")]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    let fake = f.fake_program("claude", "#!/bin/sh\nexit 1\n");
    let env = [("PATH", fake.to_str().unwrap())];
    stop(&f, A, &a_path, &env);

    join(&f, B, "g-1", &["--replace"], 0);
    // A keeps typing on its transcript after B holds the seat.
    let mut a_after = a_before.to_vec();
    a_after.push(row(A, "after", "user", "afterreplaceorchid typed by A"));
    transcript(&f, A, &a_after);
    let before_replaced_stop = support::audit_lines(&f.root());
    stop(&f, A, &a_path, &env);
    assert_eq!(support::audit_lines(&f.root()), before_replaced_stop);
    transcript(
        &f,
        B,
        &[
            row(B, "u", "user", "joining now"),
            row(B, "b", "user", "bcaptureorchid typed by B"),
        ],
    );
    stop(&f, B, &b_path, &env);

    let search = |session: &str, term: &str| {
        invoke(
            &f,
            session,
            &["search", term, "--kind", "message", "--group", "g-1"],
            0,
        )
    };
    assert_eq!(search(B, "afterreplaceorchid")["total"], 0);
    let history = search(B, "beforeorchid");
    assert_eq!(history["total"], 1);
    assert_eq!(history["hits"][0]["cite"], format!("session:{A}"));
    let captured = search(B, "bcaptureorchid");
    assert_eq!(captured["total"], 1);
    assert_eq!(captured["hits"][0]["cite"], format!("session:{B}"));
}

#[test]
fn a_restored_seat_keeps_turns_typed_during_displacement_out_of_the_group() {
    let f = Fixture::new();
    let a_before = [
        row(A, "u", "user", QUOTE),
        row(A, "before", "user", "beforeorchid typed by A"),
    ];
    let a_path = transcript(&f, A, &a_before);
    transcript(&f, B, &[row(B, "u", "user", "joining now")]);
    open(&f);
    join(&f, A, "g-1", &[], 0);
    let fake = f.fake_program("claude", "#!/bin/sh\nexit 1\n");
    let env = [("PATH", fake.to_str().unwrap())];
    stop(&f, A, &a_path, &env);

    join(&f, B, "g-1", &["--replace"], 0);
    let mut a_turns = a_before.to_vec();
    a_turns.push(row(A, "during", "user", "duringdisplaceorchid typed by A"));
    transcript(&f, A, &a_turns);
    stop(&f, A, &a_path, &env);

    // B leaves: A holds the seat again and its next hooks must not backfill.
    invoke(&f, B, &["group", "leave"], 0);
    stop(&f, A, &a_path, &env);
    a_turns.push(row(A, "after", "user", "afterrestoreorchid typed by A"));
    transcript(&f, A, &a_turns);
    stop(&f, A, &a_path, &env);

    let search = |term: &str| {
        invoke(
            &f,
            A,
            &["search", term, "--kind", "message", "--group", "g-1"],
            0,
        )
    };
    assert_eq!(search("duringdisplaceorchid")["total"], 0);
    assert_eq!(search("beforeorchid")["total"], 1);
    assert_eq!(search("afterrestoreorchid")["total"], 1);
}

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
    let join_c = f
        .cmd(&[
            "--provider",
            "codex",
            "group",
            "join",
            "g-1",
            "--as",
            "reviewer",
            "--author",
            "aide",
        ])
        .env("CODEX_THREAD_ID", C)
        .output()
        .unwrap();
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
