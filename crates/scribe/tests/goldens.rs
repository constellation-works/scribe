//! Golden tests of the public surface, captured from the built binary as it
//! ships (STD-01 R24): every command's `--help`, and the plain, table and
//! JSON forms of every list and show, against the fixture store
//! (`tests/fixtures/store`), whose timestamps are fixed.
//!
//! A mismatch fails. After an intentional surface change, regenerate with
//! `make goldens UPDATE=1` (which sets `UPDATE_GOLDENS=1`) and review the
//! diff in the PR as a contract change.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "support/goldens.rs"]
mod subject;
mod support;

use crate::support::command::with_env;
use crate::support::command::{Json, Text};
use scribe_core::TagVerbs;
use scribe_core::testing::StoreTestReads;
use scribe_core::{EntryVerbs, ProposalVerbs};
use serde_json::{Value, json};
use std::path::PathBuf;
use subject::*;
use support::command::Exit::{Capture, Success};
use support::{Fixture, text};

#[test]
fn help_matches_goldens() {
    let fx = Fixture::new();
    let failures: Vec<String> = HELP
        .iter()
        .filter_map(|(name, args)| {
            let out = fx.check(args, Success);
            compare_golden(&format!("help/{name}.txt"), &text(&out.stdout))
        })
        .collect();
    report(&failures);
}

#[test]
fn no_help_page_names_a_person() {
    // Help speaks of "the user"; scribe ships to anyone (`[user] name`
    // names them in prompts, never in help).
    let fx = Fixture::new();
    let named: Vec<&str> = HELP
        .iter()
        .filter(|(_, args)| fx.check(args, Text).contains("Daniel"))
        .map(|(name, _)| *name)
        .collect();
    assert!(named.is_empty(), "help names a person: {named:?}");
}

#[test]
fn every_help_page_has_a_golden() {
    // Each subcommand path in the tree is pinned, so a new verb cannot ship
    // without its help golden.
    let fx = Fixture::new();
    let root = fx.check(&["--help"], Text);
    let nouns = commands_in(&root);
    let mut pages = vec![String::from("root")];
    for noun in &nouns {
        pages.push(noun.clone());
        let help = fx.check(&[noun.as_str(), "--help"], Text);
        pages.extend(
            commands_in(&help)
                .into_iter()
                .map(|verb| format!("{noun}-{verb}")),
        );
    }
    let pinned: Vec<String> = HELP.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(pages, pinned);
}

#[test]
fn command_summaries_are_at_most_70_chars() {
    const MAX_SUMMARY_CHARS: usize = 70;
    let fx = Fixture::new();
    let mut queue: Vec<Vec<String>> = vec![Vec::new()];
    let mut violations = Vec::new();
    while let Some(prefix) = queue.pop() {
        let mut args: Vec<&str> = prefix.iter().map(String::as_str).collect();
        args.push("--help");
        let help = fx.check(&args, Text);
        for (name, summary) in command_rows_in(&help) {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{} {name}", prefix.join(" "))
            };
            let len = summary.chars().count();
            if summary.is_empty() || len > MAX_SUMMARY_CHARS {
                violations.push(format!(
                    "`scribe {path}` summary is {len} chars (expected 1..={MAX_SUMMARY_CHARS}): {summary}"
                ));
            }
            if name != "help" {
                let mut child = prefix.clone();
                child.push(name);
                queue.push(child);
            }
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn output_matches_goldens() {
    let fx = Fixture::seeded();
    let mut failures = Vec::new();
    for (name, args) in OUTPUT {
        for (suffix, mode) in MODES {
            let out = fx.check(&[*args, *mode].concat(), Success);
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

#[test]
fn group_roster_matches_goldens() {
    let fx = Fixture::new();
    support::seed_records(&fx.root(), &[]);
    support::sql(
        &fx.root(),
        r#"INSERT INTO groups (id, title, kind, project, opened, closed, opener, related, summary)
            VALUES (1,'Release review','review','scribe','2026-11-22T09:00:00Z','2026-11-22T10:00:00Z','opener','["repo:scribe"]',
            'Review complete.' || char(10) || 'For work missing from the handoffs, see repo:scribe.');
        INSERT INTO group_seats VALUES (1,'editor','reviewer');
        INSERT INTO group_members (position, group_id, seat, session, author, joined)
            VALUES (1,1,'editor','member','aide','2026-11-22T09:05:00Z');
        -- An open group has no summary and no closing time: its cells are `-` (STD-01 R14).
        INSERT INTO groups (id, title, kind, project, opened, closed, opener, related, summary)
            VALUES (2,'Docs sweep','sweep','scribe','2026-11-22T11:00:00Z',NULL,'opener','[]',NULL);"#,
    );
    let mut failures = Vec::new();
    for (name, args) in [
        ("group-list", vec!["group", "list"]),
        ("group-show", vec!["group", "show", "g-1"]),
    ] {
        for (suffix, mode) in MODES {
            let out = fx.check(&[args.as_slice(), *mode].concat(), Success);
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    support::sql(
        &fx.root(),
        "INSERT INTO group_seats VALUES (2,'reviewer','member'); INSERT INTO group_members (position, group_id, seat, session, author, joined) VALUES (2,2,'reviewer','member2','aide','2026-11-22T11:05:00Z');",
    );
    for (suffix, mode) in MODES {
        let out = fx.check(
            &[&["group", "backfill", "g-2"][..], *mode].concat(),
            Success,
        );
        failures.extend(compare_golden(
            &format!("output/group-backfill.{suffix}"),
            &text(&out.stdout),
        ));
    }
    report(&failures);
}

#[test]
fn mark_list_matches_goldens() {
    // One given-up marker, one pending, and one settled that is not listed.
    let fx = Fixture::new();
    support::seed_records(&fx.root(), &[]);
    support::sql(
        &fx.root(),
        r#"INSERT INTO marks (position, session, message, prompt_sha256, marker, outcome, entry, code, attempts, at) VALUES
            (0,'5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f','u-1','aa',1,'refused',NULL,'model_failed',3,'2026-11-23T23:11:16Z'),
            (1,'5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f','u-2','bb',1,'pending',NULL,NULL,1,'2026-11-23T23:12:00Z'),
            (2,'5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f','u-3','cc',1,'none',NULL,NULL,1,'2026-11-23T23:13:00Z');"#,
    );
    let mut failures = Vec::new();
    for (suffix, mode) in MODES {
        let out = fx.check(&[&["mark", "list"][..], *mode].concat(), Success);
        failures.extend(compare_golden(
            &format!("output/mark-list.{suffix}"),
            &text(&out.stdout),
        ));
    }
    report(&failures);
}

#[test]
fn session_forget_matches_golden() {
    const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
    let fx = Fixture::new();
    let project = fx.home().join(".claude/projects/-work-scribe");
    std::fs::create_dir_all(&project).unwrap();
    let transcript = project.join(format!("{SESSION}.jsonl"));
    std::fs::write(&transcript, b"").unwrap();
    support::make_private(&transcript);
    let submit = |prompt| {
        let payload = json!({
            "session_id": SESSION,
            "transcript_path": transcript,
            "cwd": "/work/scribe",
            "permission_mode": "default",
            "hook_event_name": "UserPromptSubmit",
            "prompt": prompt
        });
        let output = run_with_input(
            &fx,
            &["hook", "user-prompt-submit"],
            payload.to_string().as_bytes(),
            None,
        );
        assert!(output.status.success(), "{}", text(&output.stderr));
        text(&output.stdout)
    };
    let switched = submit("/scribe on");
    assert!(switched.contains("scribe: on"), "{switched}");
    let reply = submit("#scribe forget s-1");
    report(
        &compare_golden("output/session-forget-hook.json", &reply)
            .into_iter()
            .collect::<Vec<_>>(),
    );
}

#[test]
fn failed_session_pass_matches_golden() {
    const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
    let fx = Fixture::new();
    let project = fx.home().join(".claude/projects/-work-scribe");
    std::fs::create_dir_all(&project).unwrap();
    let transcript = project.join(format!("{SESSION}.jsonl"));
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type": "user",
                "uuid": "u-1",
                "timestamp": "2026-11-03T06:00:00Z",
                "cwd": "/work/scribe",
                "message": {"role": "user", "content": "Keep this session pass deterministic."}
            })
        ),
    )
    .unwrap();
    support::make_private(&transcript);
    let hook = json!({
        "session_id": SESSION,
        "transcript_path": transcript,
        "cwd": "/work/scribe",
        "permission_mode": "default",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "/scribe on"
    });
    let switched = run_with_input(
        &fx,
        &["hook", "user-prompt-submit"],
        hook.to_string().as_bytes(),
        None,
    );
    assert_eq!(
        switched.status.code(),
        Some(0),
        "{}",
        text(&switched.stderr)
    );

    // Exercise the actual failure tool's output with a prepared window.
    let prepared =
        support::pass::cli_step(&fx, &[], "prepare", json!({"sessions": [SESSION]})).unwrap();
    let failed = support::pass::cli_step(&fx, &[], "fail", json!({
        "basis": prepared["basis"], "error_code": "model_failed", "reason": "golden model refusal",
    })).unwrap();
    let output = serde_json::to_string_pretty(&failed).unwrap() + "\n";
    let failures: Vec<String> = compare_golden("output/session-pass-failed.json", &output)
        .into_iter()
        .collect();
    report(&failures);
}

/// The default search's hint when stored messages match and no entry or
/// session does (ORB-27628): the session pass keeps the user's turn `u-1`
/// under one segment that starts at it, and nothing else says the searched
/// word `rollout`. The goldens pin the JSON `message_matches` count,
/// unfiltered and filtered (the count takes the filters), and the
/// `search --kind message` listing the hint names, in every form.
#[cfg(unix)]
#[test]
fn message_search_hint_matches_goldens() {
    const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
    let fx = Fixture::new();
    let project = fx.home().join(".claude/projects/-work-scribe");
    std::fs::create_dir_all(&project).unwrap();
    let transcript = project.join(format!("{SESSION}.jsonl"));
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type": "user",
                "uuid": "u-1",
                "timestamp": "2026-11-03T06:00:00Z",
                "cwd": "/work/scribe",
                "message": {"role": "user", "content": "Keep the rollout note out of the release."}
            })
        ),
    )
    .unwrap();
    support::make_private(&transcript);
    let hook = json!({
        "session_id": SESSION,
        "transcript_path": transcript,
        "cwd": "/work/scribe",
        "permission_mode": "default",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "/scribe on"
    });
    let switched = run_with_input(
        &fx,
        &["hook", "user-prompt-submit"],
        hook.to_string().as_bytes(),
        None,
    );
    assert_eq!(
        switched.status.code(),
        Some(0),
        "{}",
        text(&switched.stderr)
    );

    let answer = json!({
        "title": "Release note",
        "summary": "Scribe keeps the release note out.",
        "goals": "Ship the release.",
        "open": null,
        "new": [{"starts_at": "u-1", "heading": "Release", "text": "The note stays out.", "topics": [], "related": []}],
        "next": [],
        "topics": [],
        "related": [],
        "terms": []
    });
    let pass = support::pass::run(
        |step, input| support::pass::cli_step(&fx, &[], step, input),
        json!({"sessions": [SESSION]}),
        |_, _| Ok(answer.clone()),
    )
    .unwrap();
    assert_eq!(pass["passes"][0]["result"], "written");

    let commands: &[(&str, &[&str])] = &[
        ("search-message-hint", &["search", "rollout"]),
        (
            "search-message-hint-filtered",
            &["search", "rollout", "--project", "scribe"],
        ),
        (
            "search-message-listed",
            &["search", "--kind", "message", "rollout"],
        ),
    ];
    let mut failures = Vec::new();
    for (name, args) in commands {
        for (suffix, mode) in MODES {
            let out = fx.check(&[*args, *mode].concat(), Success);
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

#[test]
fn transcript_list_and_show_match_goldens() {
    const SESSION: &str = "c0ffee00-1111-4222-8333-444455556666";
    let fx = Fixture::new();
    let root = fx.root();
    std::fs::create_dir_all(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = root.join("config.toml");
    std::fs::write(
        &config,
        "[projects.orbit]\npaths = [\"codebases/orbit\"]\n\n[projects.scribe]\npaths = [\"codebases/scribe\"]\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let project = fx.home().join(".claude/projects/-work-constellation");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join(format!("{SESSION}.jsonl")),
        format!(
            "{}\n{}\n",
            json!({
                "type": "user",
                "uuid": "u-1",
                "timestamp": "2026-10-15T10:00:00Z",
                "cwd": "/work/constellation",
                "message": {"role": "user", "content": "Backfill the orbit work in codebases/orbit and again codebases/orbit."}
            }),
            json!({
                "type": "user",
                "uuid": "u-2",
                "timestamp": "2026-10-16T11:00:00Z",
                "cwd": "/work/constellation",
                "message": {"role": "user", "content": "And one more turn about codebases/scribe."}
            })
        ),
    )
    .unwrap();
    support::make_private(&project.join(format!("{SESSION}.jsonl")));
    let mut failures = Vec::new();
    for (name, args) in [
        (
            "transcript-list",
            vec![
                "transcript",
                "list",
                "--since",
                "2026-10-01",
                "--min-turns",
                "1",
            ],
        ),
        ("transcript-show", vec!["transcript", "show", SESSION]),
    ] {
        for (suffix, mode) in MODES {
            let out = fx.check(&[args.as_slice(), *mode].concat(), Success);
            failures.extend(compare_golden(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

include!("golden_cases/entry_output.rs");
