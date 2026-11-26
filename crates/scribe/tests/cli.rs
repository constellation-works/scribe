//! End-to-end tests of the built binary: streams, exit codes, output modes,
//! pipe behaviour, inputs, and store safety (STD-01 R7–R34, STD-02 R20,
//! STD-05 R7–R9). Most run against a copy of `tests/fixtures/store`.
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/cli.rs"]
mod subject;
mod support;
#[cfg(unix)]
mod unix {
    //! Modes, links and read-only state (STD-01 R31, STD-05 R7–R9).
    use super::Capture;
    use super::support::{Fixture, make_private, snapshot, text};
    use std::fs::{self, Permissions};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::Path;

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
    }

    /// Make every directory under `dir` read-only (0500) and every file
    /// 0400, or restore 0700/0600.
    fn set_tree(dir: &Path, read_only: bool) {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                set_tree(&path, read_only);
            } else if read_only {
                chmod(&path, 0o400);
            } else {
                make_private(&path);
            }
        }
        if read_only {
            chmod(dir, 0o500);
        } else {
            make_private(dir);
        }
    }

    #[test]
    fn a_store_writable_by_others_is_refused_with_a_chmod_fix() {
        // STD-05 R9, and STD-02 R26: the message carries a runnable remedy.
        let fx = Fixture::seeded();
        let root = fx.root();
        for (path, loose, fix) in [
            (root.clone(), 0o770, "chmod 700"),
            (root.join("scribe.db"), 0o660, "chmod 600"),
            (root.join("control.json"), 0o666, "chmod 600"),
        ] {
            chmod(&path, loose);
            let args: &[&str] = if path.ends_with("control.json") {
                &["session", "list"]
            } else {
                &["entry", "list"]
            };
            let out = fx.check(args, Capture);
            chmod(&path, if path.is_dir() { 0o700 } else { 0o600 });
            assert_eq!(out.status.code(), Some(1), "{}", path.display());
            let stderr = text(&out.stderr);
            assert!(
                stderr.contains(fix) && stderr.contains(path.to_str().unwrap()),
                "{stderr}"
            );
        }
        chmod(&root, 0o770);
        let out = fx.check(&["tag", "list", "--json"], Capture);
        chmod(&root, 0o700);
        assert_eq!(super::json(&out.stderr)["code"], "store_permissions");
    }

    #[test]
    fn a_store_readable_by_others_is_refused_with_a_chmod_fix() {
        // STD-05 R8, R9: owner-only, so group and other read bits are refused
        // too. Each refusal names the chmod that fixes it.
        let fx = Fixture::seeded();
        let root = fx.root();
        for (path, loose, fix) in [
            (root.clone(), 0o755, "chmod 700"),
            (root.join("scribe.db"), 0o644, "chmod 600"),
            (root.join("config.toml"), 0o640, "chmod 600"),
        ] {
            chmod(&path, loose);
            let out = if path.ends_with("config.toml") {
                fx.check(&["tag", "list", "--json"], Capture)
            } else {
                fx.check(&["entry", "list"], Capture)
            };
            chmod(&path, if path.is_dir() { 0o700 } else { 0o600 });
            assert_eq!(out.status.code(), Some(1), "{}", path.display());
            let stderr = text(&out.stderr);
            assert!(
                stderr.contains(fix) && stderr.contains(path.to_str().unwrap()),
                "{stderr}"
            );
        }
        chmod(&root, 0o755);
        let out = fx.check(&["tag", "list", "--json"], Capture);
        chmod(&root, 0o700);
        assert_eq!(super::json(&out.stderr)["code"], "store_permissions");
    }

    #[test]
    fn a_linked_database_or_file_is_refused_and_its_target_left_alone() {
        // STD-05 R7, R9.
        let fx = Fixture::seeded();
        let db = fx.root().join("scribe.db");
        let target = fx.elsewhere("scribe.db");
        fs::rename(&db, &target).unwrap();
        symlink(&target, &db).unwrap();
        let before = fs::read(&target).unwrap();
        for args in [
            &["entry", "list", "--json"][..],
            &["entry", "show", "decisions-2", "--json"][..],
            &["session", "list", "--json"][..],
            &["tag", "list", "--json"][..],
        ] {
            let out = fx.check(args, Capture);
            assert_eq!(out.status.code(), Some(1), "{args:?}");
            assert_eq!(
                super::json(&out.stderr)["code"],
                "store_symlink",
                "{args:?}"
            );
        }
        assert_eq!(fs::read(&target).unwrap(), before);
        fs::remove_file(&db).unwrap();
        fs::rename(&target, &db).unwrap();

        let config = fx.root().join("config.toml");
        let elsewhere = fx.elsewhere("config.toml");
        fs::rename(&config, &elsewhere).unwrap();
        symlink(&elsewhere, &config).unwrap();
        let out = fx.check(&["tag", "list", "--json"], Capture);
        assert_eq!(super::json(&out.stderr)["code"], "store_symlink");
    }

    #[test]
    fn every_command_works_against_a_read_only_store_and_changes_nothing() {
        // STD-01 R31: reads take no lock and write nothing.
        let fx = Fixture::seeded();
        let root = fx.root();
        set_tree(&root, true);
        let before = snapshot(&root);
        let outs: Vec<_> = super::all_reads()
            .into_iter()
            .map(|args| {
                let out = fx.check(&args, Capture);
                (args, out)
            })
            .collect();
        let after = snapshot(&root);
        // Restore before asserting, so the fixture can always clean up.
        set_tree(&root, false);
        for (args, out) in outs {
            assert_eq!(
                out.status.code(),
                Some(0),
                "{args:?}: {}",
                text(&out.stderr)
            );
            assert!(!out.stdout.is_empty(), "{args:?}");
        }
        assert_eq!(after, before);
    }
}

use crate::support::command::Exit::Code;
use crate::support::command::Text;
use subject::environment;
use support::command::Exit::{Capture, Success};
use support::{Fixture, text};

const OPEN_SESSION: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

const INACTIVE_SESSION: &str = "3d5e7f90-1a2b-4c3d-8e9f-0a1b2c3d4e05";

/// Every list command with the key its JSON envelope carries.
const LISTS: &[(&[&str], &str)] = &[
    (&["entry", "list"], "entries"),
    (&["session", "list"], "sessions"),
    (&["proposal", "list"], "proposals"),
    (&["tag", "list"], "tags"),
];

fn json(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).expect("one JSON document")
}

/// Every command, in every mode, against the fixture.
fn all_reads() -> Vec<Vec<&'static str>> {
    let commands: [&[&str]; 6] = [
        &["entry", "list"],
        &["entry", "show", "decisions-2"],
        &["session", "list"],
        &["session", "show", OPEN_SESSION],
        &["proposal", "list"],
        &["tag", "list"],
    ];
    let modes: [&[&str]; 3] = [&[], &["--format", "table"], &["--json"]];
    let mut all = Vec::new();
    for command in commands {
        for mode in modes {
            all.push([command, mode].concat());
        }
    }
    all
}

#[test]
fn an_empty_store_lists_nothing_on_stdout_and_says_why_on_stderr() {
    // STD-01 R16. Tags are the exception: the default tags exist with no
    // entries.
    let fx = Fixture::new();
    for (args, _) in &LISTS[..3] {
        let out = fx.check(args, Success);
        assert!(out.stdout.is_empty(), "{args:?}");
        let stderr = text(&out.stderr);
        assert_eq!(stderr.lines().count(), 1, "{args:?}: {stderr}");
        assert!(
            stderr.contains(fx.root().to_str().unwrap()),
            "names the store: {stderr}"
        );
    }
    let tags = fx.check(&["tag", "list"], Text);
    let names: Vec<&str> = tags
        .lines()
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "decision",
            "handoff",
            "open-item",
            "preferences",
            "standard"
        ],
        "the shipped tags"
    );
    assert!(!fx.root().exists(), "a read never creates the store");
}

#[test]
fn an_empty_json_list_is_the_envelope_with_no_records() {
    // STD-01 R16, R34: still one document, with an empty record array.
    let fx = Fixture::new();
    for (args, key) in &LISTS[..3] {
        let mut args = args.to_vec();
        args.push("--json");
        assert_eq!(
            json(&fx.check(&args, Success).stdout),
            serde_json::json!({*key: [], "total": 0, "truncated": false, "warnings": []}),
            "{args:?}"
        );
    }
}

#[test]
fn a_filter_that_matches_nothing_names_what_was_searched() {
    let fx = Fixture::seeded();
    let out = fx.check(
        &["entry", "list", "--tag", "nothing", "--status", "active"],
        Success,
    );
    assert!(out.stdout.is_empty());
    assert_eq!(
        text(&out.stderr).trim_end(),
        "no entries match --tag nothing --status active"
    );
    let out = fx.check(&["session", "list", "--topic", "absent"], Success);
    assert_eq!(
        text(&out.stderr).trim_end(),
        "no sessions match --topic absent"
    );
}

#[test]
fn every_filter_selects_what_it_names() {
    let fx = Fixture::seeded();
    let ids = |args: &[&str]| -> Vec<String> {
        let mut args = args.to_vec();
        args.push("--json");
        let doc = json(&fx.check(&args, Success).stdout);
        let key = if args[0] == "entry" {
            "entries"
        } else {
            "sessions"
        };
        doc[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(&["entry", "list", "--tag", "rituals"]), ["rituals-1"]);
    assert_eq!(
        ids(&["entry", "list", "--status", "active"]),
        ["preferences-1", "decisions-2"]
    );
    assert_eq!(ids(&["entry", "list", "--project", "orbit"]), ["rituals-1"]);
    assert_eq!(
        ids(&["entry", "list", "--topic", "release"]),
        ["decisions-2"]
    );
    assert_eq!(
        ids(&[
            "entry", "list", "--tag", "rituals", "--topic", "ci", "--status", "retired"
        ]),
        ["rituals-1"]
    );
    assert_eq!(
        ids(&["session", "list", "--topic", "design"]),
        ["0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01"]
    );
    assert_eq!(
        ids(&["session", "list", "--project", "constellation-works/scribe"]).len(),
        2
    );
}

#[test]
fn a_limited_list_says_it_was_cut_in_the_payload_and_on_stderr() {
    // STD-01 R34 (and R12: the notice reaches stderr in every mode).
    let fx = Fixture::seeded();
    let out = fx.check(&["entry", "list", "--limit", "2", "--json"], Success);
    let doc = json(&out.stdout);
    assert_eq!(doc["entries"].as_array().map(Vec::len), Some(2));
    assert_eq!(doc["total"], 4);
    assert_eq!(doc["truncated"], true);
    assert!(
        text(&out.stderr).contains("2 of 4"),
        "stderr: {}",
        text(&out.stderr)
    );

    let out = fx.check(&["session", "list", "--limit", "1"], Success);
    assert_eq!(text(&out.stdout).lines().count(), 1);
    assert!(text(&out.stderr).contains("1 of 2"));

    let out = fx.check(&["entry", "list", "--limit", "4", "--json"], Success);
    assert_eq!(json(&out.stdout)["truncated"], false);
    assert!(out.stderr.is_empty(), "an uncut list has no notice");
}

#[test]
fn piped_output_is_one_undecorated_line_per_record() {
    // STD-01 R9, R12: no escapes, no header, N records → N lines; stderr silent.
    let fx = Fixture::seeded();
    for ((args, _), (rows, fields)) in LISTS.iter().zip([(4, 8), (2, 7), (2, 9), (7, 10)]) {
        let out = environment(
            &fx,
            args,
            &[
                ("CLICOLOR_FORCE", Some("1")),
                ("TERM", Some("xterm-256color")),
            ],
        );
        let stdout = text(&out.stdout);
        assert_eq!(stdout.lines().count(), rows, "{args:?}: {stdout}");
        assert!(!stdout.contains('\x1b'), "no ANSI escapes off a terminal");
        assert!(
            stdout.lines().all(|l| l.split('\t').count() == fields),
            "{args:?}: {stdout}"
        );
        // The one stderr line allowed is the hint that the fixture's inactive
        // session is hidden (R12: hints go to stderr).
        let hint = if args[0] == "session" {
            "1 inactive session(s) also match and are hidden; --include-inactive lists them\n"
        } else {
            ""
        };
        assert_eq!(text(&out.stderr), hint, "{args:?}");
    }
}

#[test]
fn an_explicit_table_off_a_terminal_has_a_header_and_no_color_or_truncation() {
    let fx = Fixture::seeded();
    let stdout = text(
        &environment(
            &fx,
            &["entry", "list", "--format", "table"],
            &[("CLICOLOR_FORCE", Some("1")), ("COLUMNS", Some("20"))],
        )
        .stdout,
    );
    assert_eq!(stdout.lines().count(), 5);
    assert!(stdout.starts_with("ID"));
    assert!(!stdout.contains('\x1b'));
    assert!(!stdout.contains('…'), "a pipe is never truncated");
}

#[test]
fn json_list_and_show_are_typed_documents() {
    // STD-01 R7, R10, R11.
    let fx = Fixture::seeded();
    let doc = json(&fx.check(&["entry", "list", "--json"], Success).stdout);
    let list = doc["entries"]
        .as_array()
        .expect("a list carries an entries array");
    assert_eq!(list.len(), 4);
    let last = &list[3];
    assert_eq!(last["id"], "decisions-2");
    assert_eq!(
        last["topics"],
        serde_json::json!(["merge-policy", "release"])
    );
    assert_eq!(last["content_from"]["role"], "assistant");
    assert!(
        last["cite"]["message"].is_null(),
        "absent is null, not omitted"
    );
    assert!(last["review_after"].is_null());

    let show = json(
        &fx.check(&["entry", "show", "decisions-2", "--json"], Success)
            .stdout,
    );
    assert!(show.is_object(), "one record is a JSON object");
    assert_eq!(
        show["content"],
        "squash-merge agent PRs;\nkeep merge commits for releases"
    );
    let pref = json(
        &fx.check(&["entry", "show", "preferences-1", "--json"], Success)
            .stdout,
    );
    assert_eq!(pref["review_after"], "2027-03-30");

    let session = json(
        &fx.check(&["session", "show", OPEN_SESSION, "--json"], Success)
            .stdout,
    );
    assert_eq!(session["on"], true);
    assert_eq!(session["due"], true);
    assert_eq!(
        session["next"],
        "- update the contributing guide\n- tell the release crew"
    );
    assert_eq!(session["terms"][0]["term"], "agent PR");
    assert_eq!(
        session["terms"][0]["said"],
        "by agent PR I mean any PR a crew opens"
    );

    let proposals = json(&fx.check(&["proposal", "list", "--json"], Success).stdout);
    let ids: Vec<&str> = proposals["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["p-1", "p-2"], "sorted by id, not file order");
    assert_eq!(proposals["proposals"][1]["settled_by"]["line"], 1);

    let tags = json(&fx.check(&["tag", "list", "--json"], Success).stdout);
    let tag = |name| {
        tags["tags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tag| tag["name"] == name)
            .unwrap()
    };
    assert_eq!(tag("rituals")["configured"], false);
    assert_eq!(tag("preferences")["stale_after_days"], 180);
}

#[test]
fn the_format_environment_variable_applies_and_a_bad_value_falls_back() {
    // STD-01 R8.
    let fx = Fixture::seeded();
    assert!(
        text(&environment(&fx, &["entry", "list"], &[("SCRIBE_FORMAT", Some("json"))]).stdout)
            .starts_with('{')
    );

    let out = environment(&fx, &["entry", "list"], &[("SCRIBE_FORMAT", Some("yaml"))]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout).lines().count(),
        4,
        "falls back to the piped form"
    );

    assert!(
        text(
            &environment(
                &fx,
                &["entry", "list", "--format", "table"],
                &[("SCRIBE_FORMAT", Some("json"))]
            )
            .stdout
        )
        .starts_with("ID"),
        "the flag wins"
    );
}

#[test]
fn a_missing_record_fails_with_exit_one_and_says_how_to_find_ids() {
    // STD-01 R19, R20, R21.
    let fx = Fixture::seeded();
    for (args, start, hint) in [
        (
            &["entry", "show", "decisions-7"][..],
            "error: no entry decisions-7",
            "entry list",
        ),
        (
            &["session", "show", "absent-session"][..],
            "error: no session record absent-session",
            "session list",
        ),
    ] {
        let out = fx.check(args, Capture);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(out.stdout.is_empty());
        let stderr = text(&out.stderr);
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        assert!(stderr.starts_with(start), "got {stderr:?}");
        assert!(stderr.contains(hint), "says how to find ids: {stderr}");
    }
}

#[test]
fn in_json_mode_an_error_is_one_json_object_on_stderr() {
    let fx = Fixture::new();
    let out = fx.check(&["entry", "show", "decisions-7", "--json"], Code(1));
    assert!(out.stdout.is_empty());
    let err = json(&out.stderr);
    assert_eq!(err["code"], "entry_not_found");
    assert!(err["error"].as_str().unwrap().contains("decisions-7"));
    let out = fx.check(&["session", "show", "s-9-missing", "--json"], Capture);
    assert_eq!(json(&out.stderr)["code"], "session_not_found");
}

#[test]
fn usage_errors_exit_two() {
    // STD-01 R20: bad flag, unknown subcommand, invalid value.
    let fx = Fixture::new();
    for args in [
        &["entry", "list", "--bogus"][..],
        &["entry", "frobnicate"],
        &["entry", "show", "7"],
        &["entry", "show", "decisions-07"],
        &["entry", "show", "Decisions-1"],
        &["entry", "list", "--status", "deleted"],
        &["entry", "list", "--tag", "Not A Tag"],
        &["entry", "list", "--topic", "CI"],
        &["entry", "list", "--project", "two words"],
        &["entry", "list", "--limit", "0"],
        &["session", "show", "../escape"],
        &["session", "show", ".hidden"],
        &["proposal", "list", "--limit", "3"],
        &["note", "list"],
        &["--json", "entry", "list", "--format", "table"],
        &["entry"],
        &["tag"],
    ] {
        let out = fx.check(args, Capture);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert!(out.stdout.is_empty(), "{args:?} wrote to stdout");
    }
}

#[test]
fn a_usage_error_in_json_mode_is_a_json_object() {
    let fx = Fixture::new();
    let out = fx.check(&["--json", "entry", "list", "--bogus"], Code(2));
    assert_eq!(json(&out.stderr)["code"], "usage_error");
}

#[test]
fn a_closed_stdout_ends_the_process_quietly_with_exit_zero() {
    // STD-01 R13: `scribe entry list | head -1`. The store holds far more
    // than a pipe buffer, so the binary is still writing when the reader goes.
    let fx = Fixture::new();
    support::write_entries(&fx.root(), 3000);
    let (first, out) = fx.check(&["entry", "list"], subject::CloseReader);
    assert!(first.starts_with("preferences-"), "{first}");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stderr.is_empty(), "stderr: {}", text(&out.stderr));
}

include!("cli/store_access.rs");
