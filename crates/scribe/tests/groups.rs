//! Session group contracts exercised through the installed command surface.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;
use serde_json::{Value, json};
use support::{Fixture, text};

const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";
const C: &str = "33333333-3333-4333-8333-333333333333";
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

fn transcript(f: &Fixture, session: &str, rows: &[Value]) {
    let dir = f.home().join(".claude/projects/-test");
    f.create_private_dir_all(&dir);
    let path = dir.join(format!("{session}.jsonl"));
    std::fs::write(
        &path,
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    support::make_private(&path);
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
