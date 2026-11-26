use crate::support;
use crate::support::command::Exit::Code;
use crate::support::command::Exit::{Capture, Success};
use crate::support::command::Input::Cli;
use crate::support::command::with_env;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, text};
use serde_json::{Value, json};
use std::path::PathBuf;

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["group"], expect)
    }
}

pub(super) const A: &str = "11111111-1111-4111-8111-111111111111";

pub(super) const B: &str = "22222222-2222-4222-8222-222222222222";

pub(super) const C: &str = "33333333-3333-4333-8333-333333333333";

pub(super) const D: &str = "44444444-4444-4444-8444-444444444444";

pub(super) const QUOTE: &str = "open our release review";

pub(super) const OPEN: &[&str] = &[
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

pub(super) const HANDOFF: &str = "## State\nReady\n## In flight\nReview\n## Next\nShip\n## Waiting on you\nNone\n## Do not touch\nArchive\n## Pointers\nREADME.md";

pub(super) fn row(session: &str, id: &str, role: &str, body: &str) -> Value {
    json!({"sessionId":session,"uuid":id,"type":role,"entrypoint":"cli",
        "timestamp":"2090-11-20T01:00:00Z","message":{"role":role,"content":body}})
}

pub(super) fn transcript_path(f: &Fixture, session: &str) -> PathBuf {
    let dir = f.home().join(".claude/projects/-test");
    f.create_private_dir_all(&dir);
    dir.join(format!("{session}.jsonl"))
}

pub(super) fn transcript(f: &Fixture, session: &str, rows: &[Value]) -> PathBuf {
    let path = transcript_path(f, session);
    std::fs::write(
        &path,
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    support::make_private(&path);
    path
}

pub(super) fn cleared_transcript(
    f: &Fixture,
    session: &str,
    launch: &str,
    clears: &[&str],
) -> PathBuf {
    let mut sessions = vec![json!({"via":"launch","id":launch})];
    sessions.extend(clears.iter().map(|id| json!({"via":"clear","id":id})));
    transcript(
        f,
        session,
        &[json!({"type":"derived-from","sessions":sessions})],
    )
}

pub(super) fn session_start(
    f: &Fixture,
    session: &str,
    path: Option<&PathBuf>,
    source: &str,
    entrypoint: &str,
) -> Value {
    session_start_with_cwd(f, session, path, source, entrypoint, None)
}

pub(super) fn session_start_with_cwd(
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
    let out = f.check(
        with_env(
            &["hook", "session-start"],
            "CLAUDE_CODE_ENTRYPOINT",
            entrypoint,
        ),
        Success.stdin(payload.to_string().as_bytes()),
    );
    if out.stdout.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

pub(super) fn close_plugin(f: &Fixture, tool: &str, input: Value) -> Value {
    close_plugin_with_crew(f, tool, input, Some("haiku"))
}

pub(super) fn close_plugin_with_crew(
    f: &Fixture,
    tool: &str,
    input: Value,
    crew: Option<&str>,
) -> Value {
    let mut config = json!({"root": f.root(), "transcripts": f.home().join(".claude/projects")});
    if let Some(crew) = crew {
        config["group_summary_crew"] = json!(crew);
    }
    let envelope = json!({"schema_version": 1, "tool": format!("scribe.{tool}"), "input": input,
        "context": {"workspace_root": "/nonexistent", "agent": "claude", "model": null,
          "config": config,
          "task_id": null, "job_run_id": null}});
    let out = f.check(
        Cli(&["plugin", "call"]),
        Capture.stdin(envelope.to_string().as_bytes()),
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[track_caller]
pub(super) fn invoke(f: &Fixture, session: &str, args: &[&str], code: i32) -> Value {
    let out = f.check(
        f.cmd(args)
            .arg("--json")
            .env("CLAUDE_CODE_SESSION_ID", session),
        Code(code),
    );
    serde_json::from_slice(if code == 0 { &out.stdout } else { &out.stderr }).unwrap()
}

#[track_caller]
pub(super) fn show_group(f: &Fixture, session: &str, id: &str) -> Value {
    invoke(f, session, &["group", "show", id], 0)
}

#[track_caller]
pub(super) fn open(f: &Fixture) -> Value {
    invoke(f, A, &[OPEN, &["--quote", QUOTE]].concat(), 0)
}

#[track_caller]
pub(super) fn join(f: &Fixture, session: &str, id: &str, extra: &[&str], code: i32) -> Value {
    join_as(f, session, id, "editor", extra, code)
}

#[track_caller]
pub(super) fn join_as(
    f: &Fixture,
    session: &str,
    id: &str,
    seat: &str,
    extra: &[&str],
    code: i32,
) -> Value {
    invoke(
        f,
        session,
        &[
            &["group", "join", id, "--as", seat, "--author", "aide"][..],
            extra,
        ]
        .concat(),
        code,
    )
}

pub(super) fn hook(
    f: &Fixture,
    args: &[&str],
    payload: Value,
    env: &[(&str, &str)],
) -> std::process::Output {
    f.check(
        f.cmd(args).envs(env.iter().copied()),
        Success.stdin(payload.to_string().as_bytes()),
    )
}

pub(super) fn stop(f: &Fixture, session: &str, path: &PathBuf, env: &[(&str, &str)]) {
    hook(
        f,
        &["hook", "stop"],
        json!({"session_id":session,"transcript_path":path}),
        env,
    );
}

/// A fixture whose opener `A` holds seat `chief`.
pub(super) fn seated_group() -> Fixture {
    let f = Fixture::new();
    for id in [A, B, C, D] {
        transcript(&f, id, &[row(id, "u", "user", QUOTE)]);
    }
    open(&f);
    join_as(&f, A, "g-1", "chief", &[], 0);
    f
}

pub(super) fn close_as(f: &Fixture, session: &str) -> std::process::Output {
    close_with(f, session, &[])
}

/// Close `g-1` as `session` with extra arguments, such as `--quote`.
pub(super) fn close_with(f: &Fixture, session: &str, extra: &[&str]) -> std::process::Output {
    let args = [
        &["group", "close", "g-1"][..],
        extra,
        &["--summary", "Review complete.", "--json"][..],
    ]
    .concat();
    f.check(with_env(&args, "CLAUDE_CODE_SESSION_ID", session), Capture)
}

/// The caller is refused with `group_membership`: the group
/// unchanged and exactly one more failure audit.
pub(super) fn assert_close_refused(f: &Fixture, session: &str) -> Value {
    assert_close_refused_with(f, session, &[], "group_membership")
}

/// The close with `extra` arguments is refused with `code`: the
/// group unchanged and exactly one more failure audit naming `session`. Returns
/// the refusal so a caller can check its message.
pub(super) fn assert_close_refused_with(
    f: &Fixture,
    session: &str,
    extra: &[&str],
    code: &str,
) -> Value {
    let group = show_group(f, A, "g-1");
    let audits = group_close_audits(f).len();
    let out = close_with(f, session, extra);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{session} {extra:?}: {}",
        text(&out.stderr)
    );
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], code, "{session} {extra:?}: {error}");
    assert_eq!(show_group(f, A, "g-1"), group, "{session} {extra:?}");
    let after = group_close_audits(f);
    assert_eq!(after.len(), audits + 1, "{session} {extra:?}: {after:?}");
    let last = after.last().unwrap();
    assert_eq!(last["status"], "failure");
    assert_eq!(last["error_code"], code);
    assert_eq!(last["instruction_session"], session);
    error
}

/// Restores owner access to a path a test made unreadable, even on a panic.
#[cfg(unix)]
pub(super) struct RestoreMode(PathBuf, u32);

#[cfg(unix)]
impl RestoreMode {
    pub(super) fn lock(path: &std::path::Path, restore: u32) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
        Self(path.to_path_buf(), restore)
    }
}

#[cfg(unix)]
impl Drop for RestoreMode {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(self.1));
    }
}

pub(super) fn close_with_summary_error(f: &Fixture) -> Value {
    let out = f.check(
        with_env(
            &[
                "group",
                "close",
                "g-1",
                "--summary",
                "Retained group summary.",
                "--json",
            ],
            "CLAUDE_CODE_SESSION_ID",
            A,
        ),
        Code(1),
    );
    serde_json::from_slice(&out.stderr).unwrap()
}

pub(super) fn group_close_audits(f: &Fixture) -> Vec<Value> {
    support::audit_lines(&f.root())
        .into_iter()
        .filter(|line| line["command"] == "group close")
        .collect()
}

pub(super) fn setup_missing_start(f: &Fixture) -> PathBuf {
    transcript(f, A, &[row(A, "u", "user", QUOTE)]);
    open(f);
    join(f, A, "g-1", &["--role", "reviewer"], 0);
    invoke(
        f,
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
    let path = transcript_path(f, B);
    let start = session_start(f, B, Some(&path), "startup", "claude-desktop");
    let context = start["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("transcript not written yet"), "{start}");
    assert!(!context.contains("no predecessor line"), "{start}");
    path
}

pub(super) fn write_cleared_b_transcript(f: &Fixture) -> PathBuf {
    transcript(
        f,
        B,
        &[
            json!({"type":"tools-used"}),
            json!({"type":"derived-from","sessions":[{"id":C,"via":"launch"},{"id":A,"via":"clear"}]}),
        ],
    )
}

/// Count of pending deferred ancestry checks.
pub(super) fn deferred_rows(f: &Fixture) -> i64 {
    rusqlite::Connection::open_with_flags(
        f.root().join("scribe.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row("SELECT COUNT(*) FROM deferred_ancestry", [], |r| r.get(0))
    .unwrap()
}

/// Count of membership rows; a replaced session keeps its row, so the
/// count only grows by real joins.
pub(super) fn member_rows(f: &Fixture) -> i64 {
    rusqlite::Connection::open_with_flags(
        f.root().join("scribe.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row("SELECT COUNT(*) FROM group_members", [], |r| r.get(0))
    .unwrap()
}

pub(super) const OLD: &str = "2020-01-01T00:00:00Z";

pub(super) fn row_at(session: &str, id: &str, role: &str, body: &str, at: &str) -> Value {
    let mut row = row(session, id, role, body);
    row["timestamp"] = json!(at);
    row
}

/// A line of `session` typed in the desktop app: the shape of `/formation` and
/// of a message typed while a turn runs.
pub(super) fn desktop_rows(session: &str) -> [Value; 2] {
    [
        json!({"sessionId": session, "uuid": "slash", "type": "user",
            "entrypoint": "claude-desktop", "origin": {"kind": "human"},
            "promptSource": "sdk", "turnOrigin": "human", "timestamp": OLD,
            "message": {"role": "user", "content": "<command-message>formation</command-message>\n<command-name>/formation</command-name>\n<command-args>duo scribe formationorchid</command-args>"}}),
        json!({"sessionId": session, "uuid": "queued", "type": "attachment",
            "entrypoint": "claude-desktop", "timestamp": OLD,
            "attachment": {"type": "queued_command", "prompt": "queuedorchid typed mid turn",
                "origin": {"kind": "human"}, "humanTurn": true}}),
    ]
}

/// Lines that are never a member's own typed turn.
pub(super) fn never_stored(session: &str, other: &str) -> Vec<Value> {
    let mut injected = row_at(session, "agent", "user", "agentorchid supplied", OLD);
    injected["source"] = json!("agent");
    let mut meta = row_at(session, "meta", "user", "metaorchid harness note", OLD);
    meta["isMeta"] = json!(true);
    vec![
        row_at(session, "reply", "assistant", "assistantorchid reply", OLD),
        injected,
        meta,
        row_at(
            session,
            "stdout",
            "user",
            "<local-command-stdout>harnessorchid</local-command-stdout>",
            OLD,
        ),
        row_at(
            session,
            "peer",
            "user",
            "<teammate-message teammate_id=\"x\">peerorchid</teammate-message>",
            OLD,
        ),
        row_at(
            session,
            "cron",
            "user",
            "<scheduled-prompt>scheduledorchid</scheduled-prompt>",
            OLD,
        ),
        row_at(
            other,
            "copy",
            "user",
            "copiedorchid from another session",
            OLD,
        ),
    ]
}

pub(super) fn stored_turns(f: &Fixture, session: &str) -> Vec<String> {
    let db = rusqlite::Connection::open_with_flags(
        f.root().join("scribe.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut query = db
        .prepare("SELECT message FROM messages WHERE session = ?1 ORDER BY id")
        .unwrap();
    query
        .query_map([session], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

pub(super) fn all_stored(f: &Fixture) -> i64 {
    rusqlite::Connection::open_with_flags(
        f.root().join("scribe.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
    .unwrap()
}

pub(super) fn search_group(f: &Fixture, term: &str) -> Value {
    invoke(
        f,
        A,
        &["search", term, "--kind", "message", "--group", "g-1"],
        0,
    )
}

/// A second copy of `session`'s transcript in another project directory.
pub(super) fn duplicate_transcript(f: &Fixture, session: &str, original: &PathBuf) {
    let dir = f.home().join(".claude/projects/-other");
    f.create_private_dir_all(&dir);
    let copy = dir.join(format!("{session}.jsonl"));
    std::fs::copy(original, &copy).unwrap();
    support::make_private(&copy);
}
