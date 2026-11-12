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

use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Output, Stdio};
use support::{Fixture, text};

const HELP: &[(&str, &[&str])] = &[
    ("root", &["--help"]),
    ("entry", &["entry", "--help"]),
    ("entry-add", &["entry", "add", "--help"]),
    ("entry-update", &["entry", "update", "--help"]),
    ("entry-retire", &["entry", "retire", "--help"]),
    ("entry-list", &["entry", "list", "--help"]),
    ("entry-show", &["entry", "show", "--help"]),
    ("session", &["session", "--help"]),
    ("session-list", &["session", "list", "--help"]),
    ("session-show", &["session", "show", "--help"]),
    ("session-pass", &["session", "pass", "--help"]),
    ("session-rebuild", &["session", "rebuild", "--help"]),
    ("session-deactivate", &["session", "deactivate", "--help"]),
    ("session-activate", &["session", "activate", "--help"]),
    ("session-catalog", &["session", "catalog", "--help"]),
    ("transcript", &["transcript", "--help"]),
    ("transcript-list", &["transcript", "list", "--help"]),
    ("transcript-show", &["transcript", "show", "--help"]),
    ("proposal", &["proposal", "--help"]),
    ("proposal-add", &["proposal", "add", "--help"]),
    ("proposal-update", &["proposal", "update", "--help"]),
    ("proposal-settle", &["proposal", "settle", "--help"]),
    ("proposal-list", &["proposal", "list", "--help"]),
    ("record", &["record", "--help"]),
    ("record-search", &["record", "search", "--help"]),
    ("consistency", &["consistency", "--help"]),
    ("consistency-scan", &["consistency", "scan", "--help"]),
    ("tag", &["tag", "--help"]),
    ("tag-add", &["tag", "add", "--help"]),
    ("tag-update", &["tag", "update", "--help"]),
    ("tag-list", &["tag", "list", "--help"]),
    ("question", &["question", "--help"]),
    ("question-ask", &["question", "ask", "--help"]),
    ("hook", &["hook", "--help"]),
    ("hook-session-start", &["hook", "session-start", "--help"]),
    (
        "hook-user-prompt-submit",
        &["hook", "user-prompt-submit", "--help"],
    ),
    ("hook-stop", &["hook", "stop", "--help"]),
    ("hook-session-end", &["hook", "session-end", "--help"]),
    ("browse", &["browse", "--help"]),
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
    // Ranked with no model call: entries, session headers and segments.
    ("record-search", &["record", "search", "merge agent PRs"]),
    ("tag-list", &["tag", "list"]),
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

/// The subcommand names in a help page's `Commands:` section, less `help`.
fn commands_in(help: &str) -> Vec<String> {
    help.split("Commands:")
        .nth(1)
        .unwrap_or_default()
        .lines()
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_owned)
        .collect()
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
