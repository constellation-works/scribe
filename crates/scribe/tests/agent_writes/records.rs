//! Decision and open-item entry writes through the CLI.

use super::{Fixture, json, support, text};
use crate::support::command::Exit::{Capture, Success};
use crate::support::command::Input::Cli;

const DECISION_SESSION: &str = "8dc9df2b-5808-47cf-8be4-190c9c6e0711";
const LATER_SESSION: &str = "1b7e5a3c-9f2d-4c8e-b6a1-2d3e4f5a6b7c";
const DOC_LINK: &str = "doc:scribe/docs/design/scribe/4_decisions.md#stable-api";

fn decision_transcript(fx: &Fixture, turns: &[(&str, &str)]) -> String {
    decision_transcript_in(fx, DECISION_SESSION, "2026-11-10T01:00:00Z", turns)
}

/// Writes one session's transcript with every turn at `at`; returns its reference.
fn decision_transcript_in(fx: &Fixture, session: &str, at: &str, turns: &[(&str, &str)]) -> String {
    let timed: Vec<(&str, &str, &str)> = turns
        .iter()
        .map(|(role, words)| (at, *role, *words))
        .collect();
    decision_transcript_timed(fx, session, &timed)
}

/// Writes one session's transcript, each turn at its own `(at, role, words)`;
/// returns its reference.
fn decision_transcript_timed(fx: &Fixture, session: &str, turns: &[(&str, &str, &str)]) -> String {
    let dir = fx.home().join(".claude/projects/-work-scribe");
    fx.create_private_dir_all(&dir);
    let path = dir.join(format!("{session}.jsonl"));
    let rows: String = turns
        .iter()
        .enumerate()
        .map(|(i, (at, role, words))| {
            format!(
                "{}\n",
                serde_json::json!({
                    "type": role, "uuid": format!("test-{i}"), "cwd": "/work/scribe",
                    "timestamp": at, "sessionId": session,
                    "entrypoint": "cli", "message": {"role": role, "content": words}
                })
            )
        })
        .collect();
    std::fs::write(&path, rows).unwrap();
    support::make_private(&path);
    format!("session:{session}")
}

fn add_record(fx: &Fixture, tag: &str, args: &[&str]) -> std::process::Output {
    fx.check(
        Cli(&[
            &["entry", "add", "--tag", tag, "--author", "opus", "--json"][..],
            args,
        ]
        .concat()),
        Capture,
    )
}

fn assert_record_refusal(out: std::process::Output, code: &str) {
    assert!(!out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert_eq!(json(&out.stderr)["code"], code, "{}", text(&out.stderr));
}

#[test]
fn user_decisions_check_both_quotes_and_allow_short_whole_turns() {
    let fx = Fixture::new();
    let session = decision_transcript(
        &fx,
        &[
            ("user", "yes"),
            ("user", "The compatibility guarantee is public."),
            ("user", "agreed, with changes"),
            ("user", "ok\n```text\nmore typed text\n```"),
            ("assistant", "The assistant would choose differently."),
            ("user", "why"),
        ],
    );
    let decide = |quote, reason| {
        add_record(
            &fx,
            "dec",
            &[
                "--decided-by",
                "user",
                "--related",
                &session,
                "--related",
                "task:ORB-42",
                "--related",
                DOC_LINK,
                "--quote",
                quote,
                "--reason-quote",
                reason,
                "Keep the stable API",
            ],
        )
    };

    for (quote, reason) in [
        ("agreed", "The compatibility guarantee"),
        ("ok", "The compatibility guarantee"),
        ("YES", "The compatibility guarantee"),
        (
            "The assistant would choose differently.",
            "The compatibility guarantee",
        ),
        ("yes", "compat"),
        ("yes", "An invented rationale"),
        ("yes", ""),
    ] {
        assert_record_refusal(decide(quote, reason), "quote_not_found");
    }
    let absent = add_record(
        &fx,
        "decision",
        &[
            "--decided-by",
            "user",
            "--related",
            "repo:scribe",
            "--related",
            "session:missing-transcript",
            "--quote",
            "yes",
            "A decision",
        ],
    );
    assert_record_refusal(absent, "quote_not_found");

    for reason in ["The compatibility guarantee", "why"] {
        let out = decide("yes", reason);
        assert!(out.status.success(), "{out:?}");
        let body = json(&out.stdout);
        let id = body["id"].as_str().unwrap();
        assert_eq!(body["author"]["kind"], "agent");
        assert_eq!(body["decision"]["decided_by"], "user");
        assert_eq!(body["quote"], "yes");
        assert_eq!(body["decision"]["reason_quote"], reason);
        assert_eq!(
            json(&fx.check(&["show", id, "--json"], Success).stdout),
            body
        );
    }
    assert_eq!(
        json(
            &fx.check(&["list", "--related", DOC_LINK, "--json"], Success)
                .stdout
        )["total"],
        2
    );
}

#[cfg(unix)]
#[test]
fn decision_stops_before_unreadable_later_transcript() {
    use std::os::unix::fs::PermissionsExt;

    let fx = Fixture::new();
    let first = decision_transcript(&fx, &[("user", "yes"), ("user", "because")]);
    let later = decision_transcript_in(&fx, LATER_SESSION, "2026-11-11T02:00:00Z", &[]);
    let path = fx
        .home()
        .join(".claude/projects/-work-scribe")
        .join(format!("{LATER_SESSION}.jsonl"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Privileged test runners may still read mode-000 files.
    if std::fs::File::open(&path).is_ok() {
        return;
    }
    let decide = |reason| {
        add_record(
            &fx,
            "decision",
            &[
                "--decided-by",
                "user",
                "--related",
                &first,
                "--related",
                &later,
                "--related",
                "repo:scribe",
                "--quote",
                "yes",
                "--reason-quote",
                reason,
                "A decision",
            ],
        )
    };
    let complete = decide("because");
    assert!(complete.status.success(), "{complete:?}");
    assert_record_refusal(decide("missing"), "io_error");
}

#[test]
fn decision_creation_refuses_bad_fields_and_enforces_the_combined_size() {
    let fx = Fixture::new();
    let session = decision_transcript(&fx, &[("user", "yes")]);
    for (args, code) in [
        (&["A decision"][..], "decided_by_required"),
        (
            &["--decided-by", "0-chief", "A decision"],
            "rationale_required",
        ),
        (
            &["--decided-by", "0-chief", "--rationale", " ", "A decision"],
            "rationale_required",
        ),
        (
            &[
                "--decided-by",
                "0-chief",
                "--rationale",
                "Why",
                "A decision",
            ],
            "record_required",
        ),
        (&["--decided-by", "user", "A decision"], "quote_required"),
        (
            &["--decided-by", "user", "--quote", "yes", "A decision"],
            "quote_required",
        ),
        (
            &[
                "--decided-by",
                "user",
                "--rationale",
                "Why",
                "--quote",
                "yes",
                "--related",
                &session,
                "A decision",
            ],
            "rationale_not_allowed",
        ),
        (
            &[
                "--decided-by",
                "0-chief",
                "--quote",
                "yes",
                "--rationale",
                "Why",
                "A decision",
            ],
            "decision_quote_not_allowed",
        ),
        (
            &[
                "--decided-by",
                "0-chief",
                "--reason-quote",
                "yes",
                "--rationale",
                "Why",
                "A decision",
            ],
            "decision_quote_not_allowed",
        ),
        (
            &[
                "--decided-by",
                "0-chief",
                "--asked-by",
                "orbit-0-capt",
                "A decision",
            ],
            "record_fields",
        ),
        (
            &["--decided-by", "0-chief", "--spoken", "A decision"],
            "not_memory_tag",
        ),
        (&["--decided-by", "scribe", "A decision"], "reserved_author"),
        (&[""], "invalid_text"),
    ] {
        assert_record_refusal(add_record(&fx, "decision", args), code);
    }
    let decide = |extra: &[&str]| {
        let args = [
            &[
                "--decided-by",
                "0-chief",
                "--rationale",
                "Keep worker builds repeatable",
                "--related",
                "pr:example/scribe#1",
                "Keep the stable API",
            ][..],
            extra,
        ]
        .concat();
        add_record(&fx, "decision", &args)
    };
    for (args, code) in [
        (&["--option", "Only one"][..], "invalid_options"),
        (&["--option", "One", "--option", " "], "invalid_options"),
        (
            &[
                "--option", "A", "--option", "B", "--option", "C", "--option", "D", "--option",
                "E", "--option", "F", "--option", "G",
            ],
            "invalid_options",
        ),
        (&["--recommended", "1"], "invalid_recommendation"),
        (
            &["--option", "A", "--option", "B", "--recommended", "0"],
            "invalid_recommendation",
        ),
        (
            &["--option", "A", "--option", "B", "--recommended", "3"],
            "invalid_recommendation",
        ),
    ] {
        assert_record_refusal(decide(args), code);
    }
    assert_record_refusal(
        add_record(&fx, "preferences", &["--option", "A", "A preference"]),
        "record_fields",
    );
    let content = "é".repeat(2_993);
    let decision = |reason| {
        add_record(
            &fx,
            "decision",
            &[
                "--decided-by",
                "user",
                "--related",
                &session,
                "--related",
                "repo:scribe",
                "--quote",
                "yes",
                "--reason-quote",
                reason,
                "--option",
                "A",
                "--option",
                "B",
                &content,
            ],
        )
    };
    decision_transcript(&fx, &[("user", "yes"), ("user", "ok"), ("user", "why")]);
    assert_record_refusal(decision("why"), "decision_too_long");
    assert!(decision("ok").status.success());
    assert_record_refusal(
        fx.check(
            &[
                "update",
                "decision-1",
                "--author",
                "opus",
                "--json",
                "--content",
                &format!("{content}x"),
            ],
            Capture,
        ),
        "decision_too_long",
    );
    assert_record_refusal(
        fx.check(
            &[
                "update",
                "decision-1",
                "--author",
                "opus",
                "--json",
                "--remove-related",
                &session,
            ],
            Capture,
        ),
        "decision_session",
    );
    assert_record_refusal(
        fx.check(
            &[
                "update",
                "decision-1",
                "--author",
                "opus",
                "--json",
                "--remove-related",
                "repo:scribe",
            ],
            Capture,
        ),
        "record_required",
    );
}

#[test]
fn decision_reversal_checks_links_and_commits_the_pair() {
    let fx = Fixture::new();
    let session = decision_transcript(&fx, &[("user", "1")]);
    let decide = |extra: &[&str]| {
        add_record(
            &fx,
            "decision",
            &[
                &[
                    "--decided-by",
                    "0-chief",
                    "--rationale",
                    "Preserve compatibility",
                    "--related",
                    "repo:scribe",
                    "Keep the stable API",
                ][..],
                extra,
            ]
            .concat(),
        )
    };
    let decide_as_user = |extra: &[&str]| {
        add_record(
            &fx,
            "decisions",
            &[
                &[
                    "--decided-by",
                    "user",
                    "--quote",
                    "1",
                    "--related",
                    &session,
                    "--related",
                    "task:ORB-42",
                    "Adopt the versioned API",
                ][..],
                extra,
            ]
            .concat(),
        )
    };
    assert!(decide(&[]).status.success());
    assert!(decide_as_user(&[]).status.success());
    assert!(
        add_record(
            &fx,
            "open-item",
            &[
                "--asked-by",
                "0-chief",
                "--blocked",
                "API publication",
                "Which API ships?",
            ]
        )
        .status
        .success()
    );
    for id in ["decision-9", "open-item-1"] {
        assert_record_refusal(decide(&["--supersedes", id]), "invalid_supersedes");
    }
    for id in ["open-item-9", "decision-1"] {
        assert_record_refusal(decide(&["--settles", id]), "invalid_settles");
    }
    assert_record_refusal(
        decide(&["--supersedes", "decision-2"]),
        "supersede_user_decision",
    );

    // Fail the second SQL write deterministically, after the new row's insert.
    support::sql(
        &fx.root(),
        "CREATE TRIGGER refuse_reversal BEFORE UPDATE OF status ON entries
         WHEN OLD.id = 'decision-1' BEGIN SELECT RAISE(ABORT, 'test refusal'); END;",
    );
    let failed = decide(&["--supersedes", "decision-1", "--settles", "open-item-1"]);
    assert!(!failed.status.success(), "{failed:?}");
    assert_eq!(
        json(&fx.check(&["show", "decision-1", "--json"], Success).stdout)["status"],
        "active"
    );
    assert_eq!(
        json(
            &fx.check(&["list", "--tag", "decision", "--json"], Success)
                .stdout
        )["total"],
        2
    );
    support::sql(&fx.root(), "DROP TRIGGER refuse_reversal");

    let new = decide(&["--supersedes", "decision-1", "--settles", "open-item-1"]);
    assert!(new.status.success(), "{new:?}");
    let new = json(&new.stdout);
    assert_eq!(new["id"], "decision-3");
    assert_eq!(new["decision"]["supersedes"], "decision-1");
    assert_eq!(new["decision"]["settles"], "open-item-1");
    assert_eq!(
        json(&fx.check(&["show", "open-item-1", "--json"], Success).stdout)["item"]["answer"],
        serde_json::Value::Null
    );
    let old = json(&fx.check(&["show", "decision-1", "--json"], Success).stdout);
    assert_eq!(old["status"], "superseded");
    assert_eq!(old["superseded_by"], "decision-3");
    assert_eq!(old["content"], "Keep the stable API");
    assert_record_refusal(
        decide(&["--supersedes", "decision-1"]),
        "invalid_supersedes",
    );
    assert!(
        decide_as_user(&["--supersedes", "decision-2"])
            .status
            .success()
    );
    let audit = support::audit_text(&fx.root());
    assert!(!audit.contains("Preserve compatibility"));
    assert!(!audit.contains("Keep the stable API"));
}

include!("records/open_items.rs");
include!("records/delayed_answers.rs");
