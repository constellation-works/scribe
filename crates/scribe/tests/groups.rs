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
    let mut payload = json!({
        "session_id": session,
        "hook_event_name": "SessionStart",
        "source": source
    });
    if let Some(path) = path {
        payload["transcript_path"] = json!(path);
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
    assert_eq!(version(), 10);
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
