//! Session group contracts exercised through the installed command surface.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "support/groups.rs"]
mod subject;
mod support;

use crate::support::command::Exit::Code;
use crate::support::command::Input::Cli;
use crate::support::command::Text;
use crate::support::command::with_env;
use scribe_core::SessionVerbs;
use scribe_core::testing::StoreTestReads;
use serde_json::{Value, json};
use std::io::Write;
use subject::*;
use support::command::Exit::{Capture, Success};
use support::{Fixture, text};

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
    let f = Fixture::seeded();
    f.config("");
    let entry = support::exported(&f.root(), "entries/decisions/decisions-2.md").unwrap();
    let entry = entry.replace(
        &format!("  session: {MEMBER}\n  message: null"),
        &format!("  session: {C}\n  message: null"),
    );
    assert!(entry.contains(&format!("  session: {C}\n  message: null")));
    support::seed_records(&f.root(), &[&entry]);
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
        &["entry", "list", "--limit", "1", "--group", "g-1"],
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
    assert_eq!(
        invoke(&f, A, &["search", "merge", "--group", "g-99"], 1)["code"],
        "group_not_found"
    );
}

#[test]
fn leaving_preserves_a_turn_independently_written_by_a_pass() {
    const MEMBER: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
    let f = Fixture::seeded();
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

include!("groups/close.rs");
include!("groups/close_effects.rs");
include!("groups/seats.rs");
include!("groups/clear_recovery.rs");
include!("groups/turn_capture.rs");
include!("groups/group_add.rs");
include!("groups/backfill.rs");
