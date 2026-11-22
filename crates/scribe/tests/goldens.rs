//! Golden tests of the public surface, captured from the built binary as it
//! ships (STD-01 R24): every command's `--help`, and the plain, table and
//! JSON forms of every list and show, against the fixture store
//! (`tests/fixtures/store`), whose timestamps are fixed.
//!
//! A mismatch fails. After an intentional surface change, regenerate with
//! `make goldens UPDATE=1` (which sets `UPDATE_GOLDENS=1`) and review the
//! diff in the PR as a contract change.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Output, Stdio};
use support::{Fixture, text};

const HELP: &[(&str, &[&str])] = &[
    ("root", &["--help"]),
    ("init", &["init", "--help"]),
    ("entry", &["entry", "--help"]),
    ("entry-add", &["entry", "add", "--help"]),
    ("entry-update", &["entry", "update", "--help"]),
    ("entry-retire", &["entry", "retire", "--help"]),
    ("entry-forget", &["entry", "forget", "--help"]),
    ("entry-list", &["entry", "list", "--help"]),
    ("entry-show", &["entry", "show", "--help"]),
    ("entry-pickup", &["entry", "pickup", "--help"]),
    ("session", &["session", "--help"]),
    ("session-list", &["session", "list", "--help"]),
    ("session-show", &["session", "show", "--help"]),
    ("session-pass", &["session", "pass", "--help"]),
    ("session-rebuild", &["session", "rebuild", "--help"]),
    ("session-deactivate", &["session", "deactivate", "--help"]),
    ("session-activate", &["session", "activate", "--help"]),
    ("session-catalog", &["session", "catalog", "--help"]),
    ("proposal", &["proposal", "--help"]),
    ("proposal-add", &["proposal", "add", "--help"]),
    ("proposal-update", &["proposal", "update", "--help"]),
    ("proposal-settle", &["proposal", "settle", "--help"]),
    ("proposal-list", &["proposal", "list", "--help"]),
    ("proposal-show", &["proposal", "show", "--help"]),
    ("memory", &["memory", "--help"]),
    ("memory-show", &["memory", "show", "--help"]),
    ("memory-rules", &["memory", "rules", "--help"]),
    ("memory-ack", &["memory", "ack", "--help"]),
    ("memory-forget", &["memory", "forget", "--help"]),
    ("memory-restore", &["memory", "restore", "--help"]),
    ("memory-refresh", &["memory", "refresh", "--help"]),
    ("tag", &["tag", "--help"]),
    ("tag-add", &["tag", "add", "--help"]),
    ("tag-update", &["tag", "update", "--help"]),
    ("tag-list", &["tag", "list", "--help"]),
    ("tag-show", &["tag", "show", "--help"]),
    ("brief", &["brief", "--help"]),
    ("question", &["question", "--help"]),
    ("question-ask", &["question", "ask", "--help"]),
    ("search", &["search", "--help"]),
    ("transcript", &["transcript", "--help"]),
    ("transcript-list", &["transcript", "list", "--help"]),
    ("transcript-show", &["transcript", "show", "--help"]),
    ("tmux", &["tmux", "--help"]),
    ("consistency", &["consistency", "--help"]),
    ("consistency-scan", &["consistency", "scan", "--help"]),
    ("hook", &["hook", "--help"]),
    ("hook-session-start", &["hook", "session-start", "--help"]),
    (
        "hook-user-prompt-submit",
        &["hook", "user-prompt-submit", "--help"],
    ),
    ("hook-stop", &["hook", "stop", "--help"]),
    ("hook-session-end", &["hook", "session-end", "--help"]),
    ("plugin", &["plugin", "--help"]),
    ("plugin-call", &["plugin", "call", "--help"]),
];

/// Each command whose output is pinned, by golden name; every one is
/// captured plain, as a table and as JSON.
const OUTPUT: &[(&str, &[&str])] = &[
    ("entry-list", &["entry", "list"]),
    ("entry-show", &["entry", "show", "decisions-2"]),
    ("session-list", &["session", "list"]),
    ("session-list-tree", &["session", "list", "--tree"]),
    // An inactive session is listed only on request, with an ACTIVE column.
    (
        "session-list-inactive",
        &["session", "list", "--include-inactive"],
    ),
    (
        "session-show",
        &["session", "show", "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02"],
    ),
    // The log's texts, and one segment by its citation.
    (
        "session-show-log",
        &[
            "session",
            "show",
            "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02",
            "--log",
        ],
    ),
    (
        "session-show-segment",
        &[
            "session",
            "show",
            "session:7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02#2",
        ],
    ),
    // An inactive record, with the change that hid it.
    (
        "session-show-inactive",
        &["session", "show", "3d5e7f90-1a2b-4c3d-8e9f-0a1b2c3d4e05"],
    ),
    // A legacy (format 1) record whose child is derived from the other's link.
    (
        "session-show-parent",
        &["session", "show", "0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01"],
    ),
    ("proposal-list", &["proposal", "list"]),
    // The last two sessions in the project and the open consistency claim.
    (
        "brief",
        &[
            "brief",
            "--project",
            "constellation-works/scribe",
            "--sessions",
            "2",
        ],
    ),
    // Ranked with no model call: entries, session headers and segments.
    ("search", &["search", "merge agent PRs"]),
    ("tag-list", &["tag", "list"]),
    // Capture rules, each field with its origin: all shipped here.
    ("tag-show", &["tag", "show", "preferences"]),
    // A lookup: answered from the store, with no model call.
    (
        "question-ask",
        &["question", "ask", "can you fetch me last session summary?"],
    ),
];

const MODES: &[(&str, &[&str])] = &[
    ("plain.txt", &[]),
    ("table.txt", &["--format", "table"]),
    ("json", &["--json"]),
];

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("goldens")
}

fn updating() -> bool {
    std::env::var_os("UPDATE_GOLDENS").is_some_and(|v| v == "1")
}

/// Compare `actual` to the golden at `rel`, or rewrite it when updating.
/// Returns a failure description instead of panicking so one run reports
/// every stale golden at once.
fn check(rel: &str, actual: &str) -> Option<String> {
    let path = golden_dir().join(rel);
    if updating() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return None;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    (expected != actual).then(|| {
        format!(
            "golden {rel} differs\n--- expected ({})\n{expected}\n--- actual\n{actual}",
            path.display()
        )
    })
}

fn report(failures: &[String]) {
    assert!(
        failures.is_empty(),
        "{}\n\nIf the change is intended: make goldens UPDATE=1, then review the diff.",
        failures.join("\n\n")
    );
}

#[test]
fn help_matches_goldens() {
    let fx = Fixture::new();
    let failures: Vec<String> = HELP
        .iter()
        .filter_map(|(name, args)| {
            let out = fx.ok(args);
            check(&format!("help/{name}.txt"), &text(&out.stdout))
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
        .filter(|(_, args)| text(&fx.ok(args).stdout).contains("Daniel"))
        .map(|(name, _)| *name)
        .collect();
    assert!(named.is_empty(), "help names a person: {named:?}");
}

#[test]
fn every_help_page_has_a_golden() {
    // Each subcommand path in the tree is pinned, so a new verb cannot ship
    // without its help golden.
    let fx = Fixture::new();
    let root = text(&fx.ok(&["--help"]).stdout);
    let nouns = commands_in(&root);
    let mut pages = vec![String::from("root")];
    for noun in &nouns {
        pages.push(noun.clone());
        let help = text(&fx.ok(&[noun.as_str(), "--help"]).stdout);
        pages.extend(
            commands_in(&help)
                .into_iter()
                .map(|verb| format!("{noun}-{verb}")),
        );
    }
    let pinned: Vec<String> = HELP.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(pages, pinned);
}

/// The subcommand names on a help page, less clap's built-in `help`.
///
/// A noun's page lists them under `Commands:`. The root page lists them under
/// group headings and has no `Commands:` line.
fn commands_in(help: &str) -> Vec<String> {
    command_rows_in(help)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name != "help")
        .collect()
}

/// `(name, summary)` for every subcommand row on a help page.
fn command_rows_in(help: &str) -> Vec<(String, String)> {
    if help.contains("\nCommands:\n") || help.starts_with("Commands:\n") {
        return help
            .split("Commands:")
            .nth(1)
            .unwrap_or_default()
            .lines()
            .skip(1)
            .take_while(|line| line.starts_with("  "))
            .filter_map(command_row)
            .collect();
    }
    // Grouped root help has no `Commands:` line. Rows sit under headings such
    // as `Setup:`, and stop at clap's own flag headings. A leaf page's
    // `<ID>` argument, and the examples, are not commands.
    let mut rows = Vec::new();
    let mut in_group = false;
    for line in help.lines() {
        if !line.starts_with(' ') && line.ends_with(':') {
            if matches!(
                line,
                "Options:" | "Global options:" | "Arguments:" | "Usage:"
            ) {
                break;
            }
            in_group = true;
            continue;
        }
        if in_group {
            rows.extend(command_row(line));
        }
    }
    rows
}

/// One command row: two spaces, a name, then its summary. A wrapped
/// continuation is indented further and is not a new command.
fn command_row(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("  ")?;
    if rest.is_empty() || rest.starts_with(' ') {
        return None;
    }
    Some(match rest.split_once(char::is_whitespace) {
        Some((name, summary)) => (name.to_owned(), summary.trim().to_owned()),
        None => (rest.to_owned(), String::new()),
    })
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
        let help = text(&fx.ok(&args).stdout);
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
    let fx = Fixture::new();
    fx.seed();
    let mut failures = Vec::new();
    for (name, args) in OUTPUT {
        for (suffix, mode) in MODES {
            let out = fx.ok(&[*args, *mode].concat());
            failures.extend(check(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
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

    let path = fx.fake_program(
        "claude",
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"result\",\"is_error\":true,\"result\":\"golden model refusal\"}'\n",
    );
    let out = run_with_input(
        &fx,
        &["session", "pass", "--session", SESSION, "--json"],
        b"",
        Some(&path),
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let failures: Vec<String> = check("output/session-pass-failed.json", &text(&out.stdout))
        .into_iter()
        .collect();
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
            let out = fx.ok(&[args.as_slice(), *mode].concat());
            failures.extend(check(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

/// Run the built binary with optional `PATH` and a JSON payload on stdin.
fn run_with_input(
    fx: &Fixture,
    args: &[&str],
    input: &[u8],
    path: Option<&std::ffi::OsStr>,
) -> Output {
    let mut command = fx.cmd(args);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = support::ChildGuard::new(child);
    guard
        .child()
        .stdin
        .take()
        .unwrap()
        .write_all(input)
        .unwrap();
    guard.finish()
}

#[test]
fn agent_write_shapes_match_goldens() {
    use scribe_core::{
        AgentEntry, Author, Label, Project, ProposalKind, Redactor, Store, StoreTag, Topics,
    };
    let fx = Fixture::new();
    let store = Store::open(fx.root());
    let now = time::macros::datetime!(2026-11-10 01:00 UTC);
    let author = || "opus".parse().unwrap();
    let redactor = Redactor::default();
    store
        .add_agent_tag(
            StoreTag {
                name: "notes".parse().unwrap(),
                aliases: vec!["note".parse().unwrap()],
                description: String::from("Agent notes"),
                stale_after_days: None,
                author: Author::agent(author()),
                updated_by: None,
                created: now,
                updated: now,
            },
            &redactor,
        )
        .unwrap();
    store
        .add_agent_entry(
            AgentEntry {
                tag: "notes".parse().unwrap(),
                content: String::from("Keep the API stable."),
                project: Project::sanitized("scribe"),
                topics: Topics::default(),
                related: Vec::new(),
                host: Label::sanitized("host", "host"),
                quote: None,
                evidence: None,
                as_of: None,
            },
            author(),
            now,
            &redactor,
        )
        .unwrap();
    store
        .add_agent_proposal(
            ProposalKind::Stale,
            "notes-1",
            None,
            "Review this note.",
            author(),
            now,
            &redactor,
        )
        .unwrap();
    let mut failures = Vec::new();
    for (name, args) in [
        (
            "entry-agent-write",
            vec!["entry", "update", "notes-1", "--author", "opus"],
        ),
        (
            "tag-write",
            vec!["tag", "update", "notes", "--author", "opus"],
        ),
        (
            "proposal-write",
            vec![
                "proposal",
                "update",
                "p-1",
                "--author",
                "opus",
                "--reason",
                "Review this note.",
            ],
        ),
        ("tag-list-store", vec!["tag", "list"]),
    ] {
        for (suffix, mode) in MODES {
            let out = fx.ok(&[args.as_slice(), *mode].concat());
            failures.extend(check(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

#[test]
fn consistency_scan_closed_retired_tag_matches_goldens() {
    let proposal = json!({
        "id":"p-4", "kind":"candidate", "older":"decisions-1", "newer":null,
        "candidate": {
            "tag":"decisions", "quote":"I always keep the store format named.",
            "cite": {"session":"s-1", "message":"user-choice", "at":"2026-11-15T00:00:00Z",
                "prompt_sha256":"0".repeat(64), "line":1, "cwd":"/work/scribe"},
            "project":"scribe", "provider":"claude-code", "host":"host",
            "role":"user", "evidence":null, "admitted":null
        },
        "reason":"Unmarked decisions statement: I always keep the store format named.",
        "quotes":[],
        "state":"open", "opened_at":"2026-11-15T00:01:00Z", "settled_at":null,
        "settled_by":null, "author":{"kind":"scribe", "name":"session-pass"},
        "settled_author":null, "updated_by":null, "closed_reason":null
    });
    let mut failures = Vec::new();
    for (suffix, mode) in [
        ("plain.txt", &[][..]),
        ("table.txt", &["--format", "table"][..]),
    ] {
        let fx = Fixture::new();
        let root = fx.root();
        std::fs::create_dir_all(&root).unwrap();
        support::make_private(&root);
        let path = root.join("proposals.json");
        std::fs::write(
            &path,
            json!({"format":11, "proposals":[proposal]}).to_string(),
        )
        .unwrap();
        support::make_private(&path);
        let out = fx.ok(&[&["consistency", "scan"][..], mode].concat());
        failures.extend(check(
            &format!("output/consistency-scan-closed.{suffix}"),
            &text(&out.stdout),
        ));
    }
    report(&failures);
}

#[test]
fn candidate_proposal_list_matches_goldens() {
    let fx = Fixture::new();
    fx.seed();
    let proposal = json!({
        "id":"p-3", "kind":"candidate", "older":"preferences-1", "newer":null,
        "candidate": {
            "tag":"preferences", "quote":"I prefer concise progress updates.",
            "cite": {"session":"s-1", "message":"user-choice", "at":"2026-11-15T00:00:00Z",
                "prompt_sha256":"0".repeat(64), "line":1, "cwd":"/work/scribe"},
            "project":"scribe", "provider":"claude-code", "host":"host",
            "role":"user", "evidence":null,
            "admitted": {"trigger":"I prefer", "matched":"I prefer",
                "criteria":"A standing rule from the user that applies beyond the work at hand."}
        },
        "reason":"Unmarked preference: I prefer concise progress updates.", "quotes":[],
        "state":"open", "opened_at":"2026-11-15T00:01:00Z", "settled_at":null,
        "settled_by":null, "author":{"kind":"scribe", "name":"session-pass"},
        "settled_author":null, "updated_by":null, "closed_reason":null
    });
    let path = fx.root().join("proposals.json");
    std::fs::write(
        &path,
        json!({"format":11, "proposals":[proposal]}).to_string(),
    )
    .unwrap();
    support::make_private(&path);
    let mut failures = Vec::new();
    for (suffix, mode) in MODES {
        let out = fx.ok(&[&["proposal", "list"][..], *mode].concat());
        failures.extend(check(
            &format!("output/proposal-list-candidate.{suffix}"),
            &text(&out.stdout),
        ));
    }
    report(&failures);
}

/// Daniel's turn the stated-claim goldens quote, verbatim.
const STATED_QUOTE: &str = "ok that makes sense. then let's keep the whole history in an append-only log, not one rewritten outcome.";
/// Scribe's statement of it, the approved entry's content.
const STATED: &str = "The user prefers that session records keep the whole session's history in an append-only log rather than one rewritten outcome.";

/// A store holding one approved pass claim with scribe's statement
/// (`preferences-2`, from `p-5`) and one open one (`p-6`), at format 14
/// (2_design.md §11.1).
fn stated_fixture() -> Fixture {
    let fx = Fixture::new();
    fx.seed();
    let root = fx.root();
    let sha = "0".repeat(64);
    let entry = format!(
        "---
format: 14
kind: entry
id: preferences-2
tag: preferences
status: active
project: constellation-works/scribe
topics: []
related: []
provider: claude-code
host: dk-server-1
author: {{kind: daniel, name: daniel}}
updated_by: null
created: 2026-11-15T00:05:00Z
updated: 2026-11-15T00:05:00Z
inactive_since: null
said: \"#scribe approve p-5\"
content_from: {{kind: message, role: user, message: user-history, at: 2026-11-15T00:00:00Z}}
cite: {{session: 7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02, message: user-history, at: 2026-11-15T00:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /home/user/workspace/scribe}}
retirement_reason: null
superseded_by: null
review_after: null
quote: \"{STATED_QUOTE}\"
window: null
evidence: null
as_of: null
approved_by: null
instruction: null
stated_by: {{kind: scribe, name: session-pass}}
---
{STATED}
"
    );
    let path = root.join("entries/preferences/preferences-2.md");
    std::fs::write(&path, entry).unwrap();
    support::make_private(&path);
    let cite = |message: &str| {
        json!({"session":"7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02", "message":message,
            "at":"2026-11-15T00:00:00Z", "prompt_sha256":sha, "line":1,
            "cwd":"/home/user/workspace/scribe"})
    };
    let admitted = json!({"trigger":"let's keep", "matched":"let's keep",
        "criteria":"A standing rule from the user that applies beyond the work at hand."});
    let approved = json!({
        "id":"p-5", "kind":"candidate", "older":"preferences-2", "newer":null,
        "candidate": {
            "tag":"preferences", "role":"user", "quote":STATED_QUOTE,
            "statement":STATED, "subject":"session records",
            "evidence":null, "cite":cite("user-history"),
            "project":"constellation-works/scribe", "provider":"claude-code",
            "host":"dk-server-1", "admitted":admitted
        },
        "reason":format!("Unmarked preferences statement: {STATED_QUOTE}"), "quotes":[],
        "state":"approved", "opened_at":"2026-11-15T00:01:00Z",
        "settled_at":"2026-11-15T00:05:00Z", "settled_by":cite("user-approve"),
        "author":{"kind":"scribe", "name":"session-pass"},
        "settled_author":{"kind":"daniel", "name":"daniel"}, "updated_by":null,
        "closed_reason":null, "instruction":null
    });
    let open_quote = "yeah, for the popup just show the newest five, nothing older.";
    let open = json!({
        "id":"p-6", "kind":"candidate", "older":"preferences-3", "newer":null,
        "candidate": {
            "tag":"preferences", "role":"user", "quote":open_quote,
            "statement":"The user prefers that the scribe tmux popup lists only the five newest records.",
            "subject":"the scribe tmux popup",
            "evidence":null, "cite":cite("user-popup"),
            "project":"constellation-works/scribe", "provider":"claude-code",
            "host":"dk-server-1", "admitted":admitted
        },
        "reason":format!("Unmarked preferences statement: {open_quote}"), "quotes":[],
        "state":"open", "opened_at":"2026-11-15T00:06:00Z", "settled_at":null,
        "settled_by":null, "author":{"kind":"scribe", "name":"session-pass"},
        "settled_author":null, "updated_by":null, "closed_reason":null, "instruction":null
    });
    let path = root.join("proposals.json");
    std::fs::write(
        &path,
        json!({"format":14, "proposals":[approved, open]}).to_string(),
    )
    .unwrap();
    support::make_private(&path);
    fx
}

/// The context paragraph of [`context_fixture`]'s open claim `p-7`
/// (2_design.md §15.3): what was being discussed and what the turn settles.
const CONTEXT: &str = "The user and the agent were choosing how scribe stores session records. The user rejected a single 2,500-character outcome because rewrites lose the session's start, and asked for an append-only log.";
/// The user's words `p-7` cites.
const CONTEXT_QUOTE: &str =
    "no, keep every window as its own segment so the start of the session is never rewritten.";

/// [`stated_fixture`] at format 24: its claims `p-5` and `p-6` kept as
/// older claims (a statement, no context), and an open claim `p-7` with a
/// context paragraph.
fn context_fixture() -> Fixture {
    let fx = stated_fixture();
    let path = fx.root().join("proposals.json");
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    doc["format"] = json!(24);
    let proposals = doc["proposals"].as_array_mut().unwrap();
    for p in proposals.iter_mut() {
        p["candidate"]["context"] = Value::Null;
    }
    let mut claim = proposals[1].clone();
    claim["id"] = json!("p-7");
    claim["older"] = json!("preferences-4");
    claim["opened_at"] = json!("2026-11-15T00:08:00Z");
    claim["reason"] = json!(format!("Unmarked preferences statement: {CONTEXT_QUOTE}"));
    claim["candidate"]["quote"] = json!(CONTEXT_QUOTE);
    claim["candidate"]["context"] = json!(CONTEXT);
    claim["candidate"]["statement"] = Value::Null;
    claim["candidate"]["subject"] = json!("session records");
    claim["candidate"]["cite"]["message"] = json!("user-segments");
    proposals.push(claim);
    std::fs::write(&path, doc.to_string()).unwrap();
    support::make_private(&path);
    fx
}

/// `proposal show` of a claim with context (`p-7`) and of an older claim
/// (`p-6`, its statement in the context's place): each open claim can be
/// settled from it without opening its session (2_design.md §15.3).
#[test]
fn proposal_show_matches_goldens() {
    let fx = context_fixture();
    let cases: &[(&str, &str)] = &[
        ("proposal-show-context", "p-7"),
        ("proposal-show-older", "p-6"),
    ];
    let mut failures = Vec::new();
    for (name, id) in cases {
        for (suffix, mode) in MODES {
            let out = fx.ok(&[&["proposal", "show", id][..], *mode].concat());
            failures.extend(check(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    let out = fx.ok(&["proposal", "list", "--state", "open"]);
    failures.extend(check(
        "output/proposal-list-context.plain.txt",
        &text(&out.stdout),
    ));
    report(&failures);
}

/// `scribe brief` with an open pass claim (the stated fixture's `p-6`, under
/// its tag, with its statement, subject and quote), and on an empty store.
#[test]
fn brief_matches_goldens() {
    let stated = stated_fixture();
    let empty = Fixture::new();
    let cases: &[(&str, &Fixture)] = &[("brief-claims", &stated), ("brief-empty", &empty)];
    let mut failures = Vec::new();
    for (name, fx) in cases {
        for (suffix, mode) in MODES {
            let args = [
                &["brief", "--project", "constellation-works/scribe"][..],
                *mode,
            ]
            .concat();
            let out = fx.ok(&args);
            failures.extend(check(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

#[cfg(unix)]
#[test]
fn stated_claims_match_goldens() {
    let fx = stated_fixture();
    // `ask` with a fake `claude`: one claim quotes Daniel's words, one
    // quotes scribe's statement; the check labels each by whose words the
    // quote is (2_design.md §6.4, §11.1).
    let answer = json!({
        "status": "answered",
        "answer": "The user said session records keep an append-only log.",
        "claims": [
            {"text": "his words", "quote": "keep the whole history in an append-only log", "cite": "preferences-2"},
            {"text": "the statement", "quote": "session records keep the whole session's history", "cite": "preferences-2"}
        ],
        "conflicts": []
    });
    let reply = fx.elsewhere("claude-reply.json");
    std::fs::write(
        &reply,
        json!({"type":"result", "is_error":false, "result":"", "structured_output":answer})
            .to_string(),
    )
    .unwrap();
    let script = format!("#!/bin/sh\ncat > /dev/null\ncat '{}'\n", reply.display());
    let path = fx.fake_program("claude", &script);
    let commands: &[(&str, &[&str])] = &[
        ("entry-show-stated", &["entry", "show", "preferences-2"]),
        ("proposal-list-stated", &["proposal", "list"]),
        ("search-stated", &["search", "append-only log history"]),
        (
            "question-ask-stated",
            &[
                "question",
                "ask",
                "how should session records keep their history?",
            ],
        ),
    ];
    let mut failures = Vec::new();
    for (name, args) in commands {
        for (suffix, mode) in MODES {
            let out = run_with_input(&fx, &[*args, *mode].concat(), b"", Some(&path));
            assert_eq!(out.status.code(), Some(0), "{name}: {}", text(&out.stderr));
            failures.extend(check(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

/// Daniel's instruction that approved `p-5` in the instructed goldens.
const INSTRUCTED_WORDS: &str = "approve p-5, that is how session records should work";
/// His instruction that rejected `p-6`.
const REJECTED_WORDS: &str = "reject p-6, the popup can list more";
/// The interactive session he typed it in.
const INSTRUCTED_SESSION: &str = "9a8b7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c03";

/// [`stated_fixture`], with `p-5` approved by the agent `opus` on Daniel's
/// checked instruction rather than by him typing it, and `p-6` rejected on
/// the same instruction (2_design.md §12.3).
fn instructed_fixture() -> Fixture {
    let fx = stated_fixture();
    let root = fx.root();
    let instruction = format!(
        "{{session: {INSTRUCTED_SESSION}, message: user-instruct, at: 2026-11-15T00:04:30Z, quote: \"{INSTRUCTED_WORDS}\"}}"
    );
    let path = root.join("entries/preferences/preferences-2.md");
    let entry = std::fs::read_to_string(&path)
        .unwrap()
        // Format 15 first carried an approval made on his instruction.
        .replace("format: 14\n", "format: 15\n")
        .replace(
            "said: \"#scribe approve p-5\"",
            &format!(
                "said: \"scribe proposal settle p-5 --approve --author opus --session {INSTRUCTED_SESSION} --instruction '{INSTRUCTED_WORDS}'\""
            ),
        )
        .replace(
            "approved_by: null\ninstruction: null",
            &format!("approved_by: {{kind: agent, name: opus}}\ninstruction: {instruction}"),
        );
    std::fs::write(&path, entry).unwrap();
    let path = root.join("proposals.json");
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let inst = json!({"session":INSTRUCTED_SESSION, "message":"user-instruct",
        "at":"2026-11-15T00:04:30Z", "quote":INSTRUCTED_WORDS});
    let settled_by = json!({"session":INSTRUCTED_SESSION, "message":"user-instruct",
        "at":"2026-11-15T00:04:30Z", "prompt_sha256":"0".repeat(64), "line":1,
        "cwd":"/home/user/workspace/scribe"});
    for p in doc["proposals"].as_array_mut().unwrap() {
        p["settled_author"] = json!({"kind":"agent", "name":"opus"});
        p["settled_by"] = settled_by.clone();
        p["instruction"] = inst.clone();
        if p["id"] == "p-6" {
            p["instruction"]["message"] = json!("user-reject");
            p["instruction"]["at"] = json!("2026-11-15T00:06:30Z");
            p["instruction"]["quote"] = json!(REJECTED_WORDS);
            p["settled_by"]["message"] = json!("user-reject");
            p["settled_by"]["at"] = json!("2026-11-15T00:06:30Z");
            p["state"] = json!("rejected");
            p["settled_at"] = json!("2026-11-15T00:07:00Z");
            p["closed_reason"] = json!("instructed");
        }
    }
    std::fs::write(&path, doc.to_string()).unwrap();
    fx
}

#[test]
fn instructed_approvals_match_goldens() {
    let fx = instructed_fixture();
    let commands: &[(&str, &[&str])] = &[
        ("entry-show-instructed", &["entry", "show", "preferences-2"]),
        ("proposal-list-instructed", &["proposal", "list"]),
        ("search-instructed", &["search", "append-only log history"]),
    ];
    let mut failures = Vec::new();
    for (name, args) in commands {
        for (suffix, mode) in MODES {
            let out = fx.ok(&[*args, *mode].concat());
            failures.extend(check(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    }
    report(&failures);
}

#[test]
fn only_an_instructed_approval_carries_the_approved_by_line() {
    let line = format!(
        "approved by opus on your instruction \"{INSTRUCTED_WORDS}\" ({INSTRUCTED_SESSION}, 2026-11-15)"
    );
    for (fx, instructed) in [(stated_fixture(), false), (instructed_fixture(), true)] {
        for mode in ["--format", "plain"] {
            let args: &[&str] = if mode == "plain" {
                &["entry", "show", "preferences-2"]
            } else {
                &["entry", "show", "preferences-2", "--format", "table"]
            };
            let out = text(&fx.ok(args).stdout);
            assert_eq!(out.contains(&line), instructed, "{out}");
            assert_eq!(out.contains("approved by"), instructed, "{out}");
        }
        let search = text(&fx.ok(&["search", "append-only log history"]).stdout);
        assert_eq!(search.contains(&line), instructed, "{search}");
        let proposals = text(&fx.ok(&["proposal", "list", "--state", "approved"]).stdout);
        assert_eq!(proposals.contains(&line), instructed, "{proposals}");
        let rejected = text(&fx.ok(&["proposal", "list", "--state", "rejected"]).stdout);
        assert_eq!(
            rejected.contains(&format!(
                "rejected by opus on your instruction \"{REJECTED_WORDS}\""
            )),
            instructed,
            "{rejected}"
        );
    }
}

#[cfg(unix)]
#[test]
fn ask_evidence_names_an_instructed_approval_outside_the_quotable_text() {
    let fx = instructed_fixture();
    let answer = json!({"status":"no_record", "answer":"", "claims":[], "conflicts":[]});
    let reply = fx.elsewhere("claude-reply.json");
    std::fs::write(
        &reply,
        json!({"type":"result", "is_error":false, "result":"", "structured_output":answer})
            .to_string(),
    )
    .unwrap();
    let prompt = fx.elsewhere("prompt.txt");
    let script = format!(
        "#!/bin/sh\ncat > '{}'\ncat '{}'\n",
        prompt.display(),
        reply.display()
    );
    let path = fx.fake_program("claude", &script);
    let out = run_with_input(
        &fx,
        &[
            "question",
            "ask",
            "how should session records keep their history?",
        ],
        b"",
        Some(&path),
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let prompt = std::fs::read_to_string(&prompt).unwrap();
    let item = prompt
        .split("<evidence ")
        .find(|item| item.contains("cite=\"preferences-2\""))
        .unwrap_or_else(|| panic!("{prompt}"));
    let line = format!(
        "<approval>approved by opus on the user's instruction \"{INSTRUCTED_WORDS}\" ({INSTRUCTED_SESSION}, 2026-11-15)</approval>\n</evidence>"
    );
    assert!(item.contains(&line), "{item}");
    // It is still his entry, weighted as his words.
    assert!(
        item.contains("words=\"scribe's statement of the user's turn\""),
        "{item}"
    );
}

/// Daniel's turn the captured-idea goldens quote, verbatim.
const CAPTURED_QUOTE: &str = "what if we cached the routing table per host? not now though, let's park it until the drains are quiet.";
/// Scribe's statement of it, the captured entry's content.
const CAPTURED: &str =
    "The user suggests caching the routing table per host so lookups stay local.";
/// His words parking it.
const PARKED: &str = "let's park it until the drains are quiet";

/// One idea entry at format 16, the format that first carried `outcome`.
#[allow(clippy::too_many_arguments)]
fn idea_entry(
    id: &str,
    author: &str,
    said: &str,
    content_from: &str,
    quote: &str,
    stated_by: &str,
    outcome: &str,
    body: &str,
) -> String {
    let sha = "0".repeat(64);
    format!(
        "---
format: 16
kind: entry
id: {id}
tag: idea
status: active
project: constellation-works/scribe
topics: []
related: []
provider: claude-code
host: dk-server-1
author: {author}
updated_by: null
created: 2026-11-15T00:05:00Z
updated: 2026-11-15T00:05:00Z
inactive_since: null
said: \"{said}\"
content_from: {content_from}
cite: {{session: 7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02, message: user-idea, at: 2026-11-15T00:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /home/user/workspace/scribe}}
retirement_reason: null
superseded_by: null
review_after: null
quote: {quote}
window: null
evidence: null
as_of: null
approved_by: null
instruction: null
stated_by: {stated_by}
outcome: {outcome}
---
{body}
"
    )
}

/// A store holding an idea the session pass captured itself (`idea-1`,
/// parked in Daniel's words) and one he typed with `#scribe idea:`
/// (`idea-2`), at format 16 (2_design.md §11.6).
fn captured_fixture() -> Fixture {
    let fx = Fixture::new();
    fx.seed();
    // `idea` is a config example now (specs/catchers.md): declare it.
    let config = fx.root().join("config.toml");
    let seeded = std::fs::read_to_string(&config).unwrap_or_default();
    fx.config(&format!("{seeded}\n{}", support::IDEA_CATCHER));
    let dir = fx.root().join("entries/idea");
    std::fs::create_dir(&dir).unwrap();
    support::make_private(&dir);
    let pass = "{kind: scribe, name: session-pass}";
    let captured = idea_entry(
        "idea-1",
        pass,
        "",
        "{kind: message, role: user, message: user-idea, at: 2026-11-15T00:00:00Z}",
        &format!("\"{CAPTURED_QUOTE}\""),
        pass,
        &format!("{{state: parked, reason: \"{PARKED}\", task: null}}"),
        CAPTURED,
    );
    let typed = idea_entry(
        "idea-2",
        "{kind: daniel, name: daniel}",
        "#scribe idea: a dry-run flag for the session pass",
        "{kind: instruction}",
        "null",
        "null",
        "null",
        "a dry-run flag for the session pass",
    );
    for (id, text) in [("idea-1", captured), ("idea-2", typed)] {
        let path = dir.join(format!("{id}.md"));
        std::fs::write(&path, text).unwrap();
        support::make_private(&path);
    }
    fx
}

#[cfg(unix)]
#[test]
fn captured_ideas_match_goldens() {
    let fx = captured_fixture();
    // `ask --tag idea` with a fake `claude`: one claim quotes Daniel's words
    // in the captured idea, one its statement, one the idea he typed; the
    // check labels each (2_design.md §6.4, §11.6).
    let answer = json!({
        "status": "answered",
        "answer": "The user floated a per-host routing cache and a dry-run flag.",
        "claims": [
            {"text": "his words", "quote": "what if we cached the routing table per host?", "cite": "idea-1"},
            {"text": "the statement", "quote": "caching the routing table per host", "cite": "idea-1"},
            {"text": "typed", "quote": "a dry-run flag for the session pass", "cite": "idea-2"}
        ],
        "conflicts": []
    });
    let reply = fx.elsewhere("claude-reply.json");
    std::fs::write(
        &reply,
        json!({"type":"result", "is_error":false, "result":"", "structured_output":answer})
            .to_string(),
    )
    .unwrap();
    let script = format!("#!/bin/sh\ncat > /dev/null\ncat '{}'\n", reply.display());
    let path = fx.fake_program("claude", &script);
    let commands: &[(&str, &[&str])] = &[
        ("entry-show-captured", &["entry", "show", "idea-1"]),
        ("entry-show-typed-idea", &["entry", "show", "idea-2"]),
        ("search-captured", &["search", "routing table per host"]),
        (
            "question-ask-captured",
            &[
                "question",
                "ask",
                "what ideas did I have for routing?",
                "--tag",
                "idea",
            ],
        ),
    ];
    let mut failures = Vec::new();
    for (name, args) in commands {
        for (suffix, mode) in MODES {
            let out = run_with_input(&fx, &[*args, *mode].concat(), b"", Some(&path));
            assert_eq!(out.status.code(), Some(0), "{name}: {}", text(&out.stderr));
            failures.extend(check(
                &format!("output/{name}.{suffix}"),
                &without_age(&text(&out.stdout)),
            ));
        }
    }
    report(&failures);

    // Whatever the goldens say, the labels hold: Daniel's words captured by
    // the pass, scribe's statement, and Daniel's own typed idea.
    let out = run_with_input(
        &fx,
        &[
            "question",
            "ask",
            "what ideas did I have for routing?",
            "--tag",
            "idea",
            "--json",
        ],
        b"",
        Some(&path),
    );
    let asked: Value = serde_json::from_slice(&out.stdout).unwrap();
    let labels: Vec<&str> = asked["claims"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        [
            "The user (captured by the session pass)",
            "scribe's statement of the user's turn, captured by the session pass (not typed by the user)",
            "The user"
        ]
    );
}

/// `out` with an action entry's age, which moves with the clock, masked as
/// `<age>`: the JSON `age_seconds`, the table's `age` field and the plain
/// row's last column.
fn without_age(out: &str) -> String {
    let lines: Vec<String> = out
        .lines()
        .map(|line| {
            if let Some((head, _)) = line.split_once("\"age_seconds\": ") {
                format!("{head}\"age_seconds\": \"<age>\",")
            } else if let Some(rest) = line.strip_prefix("age ") {
                format!(
                    "age {}<age>",
                    " ".repeat(rest.len() - rest.trim_start().len())
                )
            } else if line.split('\t').count() == 8 {
                let mut cols: Vec<&str> = line.split('\t').collect();
                cols[7] = "<age>";
                cols.join("\t")
            } else {
                line.to_owned()
            }
        })
        .collect();
    let mut joined = lines.join("\n");
    if out.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// The memory block, each file shown, the fixed rules, and the writes on
/// them, against the memory fixture (`tests/fixtures/memory`): a profile,
/// two `now` items and two milestones, all with fixed dates, so the output
/// is the same every day.
#[test]
fn memory_matches_goldens() {
    let fx = Fixture::new();
    fx.seed();
    support::copy_private(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memory"),
        &fx.root().join("memory"),
    );
    let session = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
    let mut failures = Vec::new();
    // The fixture config redacts GITHUB_TOKEN's value, so a write needs it set.
    let ok = |args: &[&str]| {
        let out = fx
            .cmd(args)
            .env("GITHUB_TOKEN", "synthetic-fixture-token-value")
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        out
    };
    let mut capture = |name: &str, args: &[&str]| {
        for (suffix, mode) in MODES {
            let out = ok(&[args, *mode].concat());
            failures.extend(check(
                &format!("output/{name}.{suffix}"),
                &text(&out.stdout),
            ));
        }
    };
    capture("memory-show", &["memory", "show"]);
    capture("memory-show-profile", &["memory", "show", "profile"]);
    capture(
        "memory-show-item",
        &["memory", "show", "2026-12-01-hackathon-deadline"],
    );
    capture(
        "memory-show-milestone",
        &["memory", "show", "2026-06-14-married"],
    );
    capture("memory-rules", &["memory", "rules"]);
    // The fixture's item was raised in this session already, so the ack
    // changes nothing and prints the same every day.
    capture(
        "memory-ack",
        &[
            "memory",
            "ack",
            "2026-12-12-sisters-wedding",
            "--session",
            session,
            "--author",
            "opus",
        ],
    );
    // Forget removes the file, so each mode forgets a fresh copy of it.
    let stone = "milestones/2026-10-02-pulsar-decommissioned.md";
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memory");
    for (suffix, mode) in MODES {
        let target = fx.root().join("memory").join(stone);
        std::fs::copy(fixture.join(stone), &target).unwrap();
        support::make_private(&target);
        let out = ok(&[
            &[
                "memory",
                "forget",
                "2026-10-02-pulsar-decommissioned",
                "--author",
                "opus",
                "--confirm",
            ][..],
            *mode,
        ]
        .concat());
        failures.extend(check(
            &format!("output/memory-forget.{suffix}"),
            &text(&out.stdout),
        ));
    }
    report(&failures);
}
