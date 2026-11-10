//! End-to-end tests of the built binary: streams, exit codes, output modes,
//! pipe behaviour, inputs, and store safety (STD-01 R7–R34, STD-02 R20,
//! STD-05 R7–R9). Most run against a copy of `tests/fixtures/store`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::io::{BufRead, BufReader};
use std::process::Stdio;
use support::{ChildGuard, Fixture, text};

const OPEN_SESSION: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

fn json(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).expect("one JSON document")
}

/// Every list command with the key its JSON envelope carries.
const LISTS: &[(&[&str], &str)] = &[
    (&["entry", "list"], "entries"),
    (&["session", "list"], "sessions"),
    (&["proposal", "list"], "proposals"),
    (&["tag", "list"], "tags"),
];

#[test]
fn an_empty_store_lists_nothing_on_stdout_and_says_why_on_stderr() {
    // STD-01 R16. Tags are the exception: the default tags exist with no
    // entries.
    let fx = Fixture::new();
    for (args, _) in &LISTS[..3] {
        let out = fx.ok(args);
        assert!(out.stdout.is_empty(), "{args:?}");
        let stderr = text(&out.stderr);
        assert_eq!(stderr.lines().count(), 1, "{args:?}: {stderr}");
        assert!(
            stderr.contains(fx.root().to_str().unwrap()),
            "names the store: {stderr}"
        );
    }
    let tags = text(&fx.ok(&["tag", "list"]).stdout);
    let names: Vec<&str> = tags
        .lines()
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    assert_eq!(names, ["decisions", "preferences"]);
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
            json(&fx.ok(&args).stdout),
            serde_json::json!({*key: [], "total": 0, "truncated": false, "warnings": []}),
            "{args:?}"
        );
    }
}

#[test]
fn a_filter_that_matches_nothing_names_what_was_searched() {
    let fx = Fixture::new();
    fx.seed();
    let out = fx.ok(&["entry", "list", "--tag", "nothing", "--status", "active"]);
    assert!(out.stdout.is_empty());
    assert_eq!(
        text(&out.stderr).trim_end(),
        "no entries match --tag nothing --status active"
    );
    let out = fx.ok(&["session", "list", "--topic", "absent"]);
    assert_eq!(
        text(&out.stderr).trim_end(),
        "no sessions match --topic absent"
    );
}

#[test]
fn every_filter_selects_what_it_names() {
    let fx = Fixture::new();
    fx.seed();
    let ids = |args: &[&str]| -> Vec<String> {
        let mut args = args.to_vec();
        args.push("--json");
        let doc = json(&fx.ok(&args).stdout);
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
    assert_eq!(
        ids(&["entry", "list", "--tag", "decisions"]),
        ["decisions-1", "decisions-2"]
    );
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
            "entry",
            "list",
            "--tag",
            "decisions",
            "--topic",
            "merge-policy",
            "--status",
            "superseded"
        ]),
        ["decisions-1"]
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
    let fx = Fixture::new();
    fx.seed();
    let out = fx.ok(&["entry", "list", "--limit", "2", "--json"]);
    let doc = json(&out.stdout);
    assert_eq!(doc["entries"].as_array().map(Vec::len), Some(2));
    assert_eq!(doc["total"], 4);
    assert_eq!(doc["truncated"], true);
    assert!(
        text(&out.stderr).contains("2 of 4"),
        "stderr: {}",
        text(&out.stderr)
    );

    let out = fx.ok(&["session", "list", "--limit", "1"]);
    assert_eq!(text(&out.stdout).lines().count(), 1);
    assert!(text(&out.stderr).contains("1 of 2"));

    let out = fx.ok(&["entry", "list", "--limit", "4", "--json"]);
    assert_eq!(json(&out.stdout)["truncated"], false);
    assert!(out.stderr.is_empty(), "an uncut list has no notice");
}

#[test]
fn piped_output_is_one_undecorated_line_per_record() {
    // STD-01 R9, R12: no escapes, no header, N records → N lines; stderr silent.
    let fx = Fixture::new();
    fx.seed();
    for ((args, _), (rows, fields)) in LISTS.iter().zip([(4, 6), (2, 7), (2, 7), (3, 5)]) {
        let mut cmd = fx.cmd(args);
        cmd.env("CLICOLOR_FORCE", "1").env("TERM", "xterm-256color");
        let out = cmd.output().unwrap();
        let stdout = text(&out.stdout);
        assert_eq!(stdout.lines().count(), rows, "{args:?}: {stdout}");
        assert!(!stdout.contains('\x1b'), "no ANSI escapes off a terminal");
        assert!(
            stdout.lines().all(|l| l.split('\t').count() == fields),
            "{args:?}: {stdout}"
        );
        assert!(
            out.stderr.is_empty(),
            "{args:?} stderr: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn an_explicit_table_off_a_terminal_has_a_header_and_no_color_or_truncation() {
    let fx = Fixture::new();
    fx.seed();
    let mut cmd = fx.cmd(&["entry", "list", "--format", "table"]);
    cmd.env("CLICOLOR_FORCE", "1").env("COLUMNS", "20");
    let stdout = text(&cmd.output().unwrap().stdout);
    assert_eq!(stdout.lines().count(), 5);
    assert!(stdout.starts_with("ID"));
    assert!(!stdout.contains('\x1b'));
    assert!(!stdout.contains('…'), "a pipe is never truncated");
}

#[test]
fn json_list_and_show_are_typed_documents() {
    // STD-01 R7, R10, R11.
    let fx = Fixture::new();
    fx.seed();
    let doc = json(&fx.ok(&["entry", "list", "--json"]).stdout);
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

    let show = json(&fx.ok(&["entry", "show", "decisions-2", "--json"]).stdout);
    assert!(show.is_object(), "one record is a JSON object");
    assert_eq!(
        show["content"],
        "squash-merge agent PRs;\nkeep merge commits for releases"
    );
    let pref = json(&fx.ok(&["entry", "show", "preferences-1", "--json"]).stdout);
    assert_eq!(pref["review_after"], "2027-03-30");

    let session = json(&fx.ok(&["session", "show", OPEN_SESSION, "--json"]).stdout);
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

    let proposals = json(&fx.ok(&["proposal", "list", "--json"]).stdout);
    let ids: Vec<&str> = proposals["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["p-1", "p-2"], "sorted by id, not file order");
    assert_eq!(proposals["proposals"][1]["settled_by"]["line"], 1);

    let tags = json(&fx.ok(&["tag", "list", "--json"]).stdout);
    assert_eq!(tags["tags"][2]["name"], "rituals");
    assert_eq!(tags["tags"][2]["configured"], false);
    assert_eq!(tags["tags"][1]["stale_after_days"], 180);
}

#[test]
fn the_format_environment_variable_applies_and_a_bad_value_falls_back() {
    // STD-01 R8.
    let fx = Fixture::new();
    fx.seed();
    let mut cmd = fx.cmd(&["entry", "list"]);
    cmd.env("SCRIBE_FORMAT", "json");
    assert!(text(&cmd.output().unwrap().stdout).starts_with('{'));

    let mut cmd = fx.cmd(&["entry", "list"]);
    cmd.env("SCRIBE_FORMAT", "yaml");
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout).lines().count(),
        4,
        "falls back to the piped form"
    );

    let mut cmd = fx.cmd(&["entry", "list", "--format", "table"]);
    cmd.env("SCRIBE_FORMAT", "json");
    assert!(
        text(&cmd.output().unwrap().stdout).starts_with("ID"),
        "the flag wins"
    );
}

#[test]
fn a_missing_record_fails_with_exit_one_and_says_how_to_find_ids() {
    // STD-01 R19, R20, R21.
    let fx = Fixture::new();
    fx.seed();
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
        let out = fx.run(args);
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
    let out = fx.run(&["entry", "show", "decisions-7", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err = json(&out.stderr);
    assert_eq!(err["code"], "entry_not_found");
    assert!(err["error"].as_str().unwrap().contains("decisions-7"));
    let out = fx.run(&["session", "show", "s-9", "--json"]);
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
        let out = fx.run(args);
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
    let out = fx.run(&["--json", "entry", "list", "--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out.stderr)["code"], "usage_error");
}

#[test]
fn a_closed_stdout_ends_the_process_quietly_with_exit_zero() {
    // STD-01 R13: `scribe entry list | head -1`. The store holds far more
    // than a pipe buffer, so the binary is still writing when the reader goes.
    let fx = Fixture::new();
    support::write_entries(&fx.root(), 3000);
    let mut child = ChildGuard::new(
        fx.cmd(&["entry", "list"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut first = String::new();
    BufReader::new(child.child().stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    assert!(first.starts_with("decisions-"), "{first}");
    // The reader was dropped above, closing the pipe.
    let out = child.finish();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stderr.is_empty(), "stderr: {}", text(&out.stderr));
}

#[test]
fn the_data_directory_defaults_under_home_and_is_never_guessed() {
    let fx = Fixture::new();
    support::copy_private(&support::fixture_store(), &fx.home().join(".scribe"));
    let mut cmd = fx.cmd(&["entry", "list"]);
    cmd.env_remove("SCRIBE_ROOT");
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stdout).lines().count(), 4, "read ~/.scribe");

    let mut cmd = fx.cmd(&["entry", "list"]);
    cmd.env_remove("SCRIBE_ROOT").env_remove("HOME");
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("--root"), "says how to fix it");
}

#[test]
fn the_root_flag_beats_the_environment_and_nothing_else_is_touched() {
    // STD-01 R28: a scoped invocation reads only inside its scope.
    let fx = Fixture::new();
    let other = fx.elsewhere("other");
    support::copy_private(&support::fixture_store(), &other);
    let other_arg = other.to_str().unwrap();
    let out = fx.ok(&["--root", other_arg, "entry", "list"]);
    assert_eq!(text(&out.stdout).lines().count(), 4);
    assert!(!fx.root().exists(), "SCRIBE_ROOT was not used");
    let home: Vec<_> = std::fs::read_dir(fx.home()).unwrap().collect();
    assert!(home.is_empty(), "HOME was touched: {home:?}");
}

#[test]
fn no_command_writes_anything_not_even_an_audit_line() {
    // STD-01 R31: every command in this slice reads.
    let fx = Fixture::new();
    fx.seed();
    let before = support::snapshot(&fx.root());
    for args in all_reads() {
        fx.ok(&args);
    }
    assert_eq!(support::snapshot(&fx.root()), before);
    assert!(!fx.root().join("audit.jsonl").exists());
    assert!(!fx.root().join(".lock").exists(), "reads take no lock");
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
fn every_listed_id_is_accepted_back_by_show() {
    // STD-01 R32: identifiers round-trip, from both the JSON and piped forms.
    let fx = Fixture::new();
    fx.seed();
    for (noun, key) in [("entry", "entries"), ("session", "sessions")] {
        let doc = json(&fx.ok(&[noun, "list", "--json"]).stdout);
        for record in doc[key].as_array().unwrap() {
            let id = record["id"].as_str().unwrap();
            assert_eq!(&json(&fx.ok(&[noun, "show", id, "--json"]).stdout), record);
        }
        let piped = text(&fx.ok(&[noun, "list"]).stdout);
        for line in piped.lines() {
            fx.ok(&[noun, "show", line.split('\t').next().unwrap()]);
        }
    }
}

#[test]
fn a_corrupt_record_is_skipped_with_a_warning_naming_the_file() {
    // docs/design/scribe/4_decisions.md, "One malformed record makes its
    // file corrupt": a list goes on without it; showing it fails.
    let fx = Fixture::new();
    fx.seed();
    let unknown = fx.root().join("entries/decisions/decisions-2.md");
    let text_with_key = std::fs::read_to_string(&unknown)
        .unwrap()
        .replace("review_after: null\n", "review_after: null\ncolour: blue\n");
    std::fs::write(&unknown, text_with_key).unwrap();
    let missing = fx.root().join("entries/preferences/preferences-1.md");
    let without_key: String = std::fs::read_to_string(&missing)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with("superseded_by:"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&missing, without_key).unwrap();

    let out = fx.ok(&["entry", "list"]);
    assert_eq!(text(&out.stdout).lines().count(), 2);
    let stderr = text(&out.stderr);
    assert_eq!(stderr.lines().count(), 2, "{stderr}");
    assert!(
        stderr.contains(unknown.to_str().unwrap()) && stderr.contains("colour"),
        "{stderr}"
    );
    assert!(
        stderr.contains(missing.to_str().unwrap()) && stderr.contains("superseded_by"),
        "{stderr}"
    );

    let doc = json(&fx.ok(&["entry", "list", "--json"]).stdout);
    assert_eq!(doc["total"], 2);
    assert_eq!(doc["warnings"].as_array().map(Vec::len), Some(2));
    assert_eq!(doc["warnings"][0]["code"], "store_corrupt");

    let tags = json(&fx.ok(&["tag", "list", "--json"]).stdout);
    assert_eq!(
        tags["warnings"].as_array().map(Vec::len),
        Some(2),
        "tag counts skip it too"
    );

    let out = fx.run(&["entry", "show", "decisions-2", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let err = json(&out.stderr);
    assert_eq!(err["code"], "store_corrupt");
    assert!(
        err["error"].as_str().unwrap().contains("decisions-2.md"),
        "{err}"
    );
}

#[test]
fn a_record_from_a_newer_build_is_refused_whole_and_left_alone() {
    // STD-03 R10: not skipped like a corrupt file; this build cannot read
    // the store safely.
    let fx = Fixture::new();
    fx.seed();
    for (file, args) in [
        (
            "entries/decisions/decisions-1.md",
            &["entry", "list", "--json"][..],
        ),
        ("control.json", &["session", "list", "--json"][..]),
        ("proposals.json", &["proposal", "list", "--json"][..]),
    ] {
        let path = fx.root().join(file);
        let original = std::fs::read_to_string(&path).unwrap();
        let newer = original.replacen("format: 1", "format: 999", 1).replacen(
            "\"format\": 1",
            "\"format\": 999",
            1,
        );
        std::fs::write(&path, &newer).unwrap();
        let out = fx.run(args);
        assert_eq!(out.status.code(), Some(1), "{file}");
        assert_eq!(json(&out.stderr)["code"], "store_too_new", "{file}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            newer,
            "not rewritten"
        );
        std::fs::write(&path, original).unwrap();
    }
}

#[test]
fn an_invalid_config_is_refused_naming_the_file_and_line() {
    let fx = Fixture::new();
    fx.seed();
    std::fs::write(
        fx.root().join("config.toml"),
        "[models]\nask = \"x\"\nanswer = 1\n",
    )
    .unwrap();
    let out = fx.run(&["tag", "list", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let err = json(&out.stderr);
    assert_eq!(err["code"], "config_invalid");
    let message = err["error"].as_str().unwrap();
    assert!(
        message.contains("config.toml") && message.contains("line 3"),
        "{message}"
    );
}

#[test]
fn logs_go_to_stderr_and_leave_stdout_as_pure_payload() {
    // STD-01 R12, STD-02 R15.
    let fx = Fixture::new();
    fx.seed();
    let mut cmd = fx.cmd(&["entry", "list", "--json"]);
    cmd.env("SCRIBE_LOG", "debug");
    let out = cmd.output().unwrap();
    let _: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is only JSON");
    assert!(!out.stderr.is_empty(), "debug logs should appear on stderr");
}

#[cfg(unix)]
mod unix {
    //! Modes, links and read-only state (STD-01 R31, STD-05 R7–R9).
    use super::support::{Fixture, make_private, snapshot, text};
    use std::fs::{self, Permissions};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::Path;

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_store_writable_by_others_is_refused_with_a_chmod_fix() {
        // STD-05 R9, and STD-02 R26: the message carries a runnable remedy.
        let fx = Fixture::new();
        fx.seed();
        let root = fx.root();
        for (path, loose, fix) in [
            (root.clone(), 0o770, "chmod 700"),
            (root.join("entries/decisions"), 0o777, "chmod 700"),
            (
                root.join("entries/decisions/decisions-1.md"),
                0o660,
                "chmod 600",
            ),
            (root.join("control.json"), 0o666, "chmod 600"),
        ] {
            chmod(&path, loose);
            let args: &[&str] = if path.ends_with("control.json") {
                &["session", "list"]
            } else {
                &["entry", "list"]
            };
            let out = fx.run(args);
            chmod(&path, if path.is_dir() { 0o700 } else { 0o600 });
            assert_eq!(out.status.code(), Some(1), "{}", path.display());
            let stderr = text(&out.stderr);
            assert!(
                stderr.contains(fix) && stderr.contains(path.to_str().unwrap()),
                "{stderr}"
            );
        }
        chmod(&root, 0o770);
        let out = fx.run(&["tag", "list", "--json"]);
        chmod(&root, 0o700);
        assert_eq!(super::json(&out.stderr)["code"], "store_permissions");
    }

    #[test]
    fn a_linked_record_or_directory_is_refused_and_its_target_left_alone() {
        // STD-05 R7, R9.
        let fx = Fixture::new();
        fx.seed();
        let entry = fx.root().join("entries/decisions/decisions-2.md");
        let target = fx.elsewhere("decisions-2.md");
        fs::rename(&entry, &target).unwrap();
        symlink(&target, &entry).unwrap();
        let before = fs::read(&target).unwrap();
        for args in [
            &["entry", "list", "--json"][..],
            &["entry", "show", "decisions-2", "--json"][..],
            &["tag", "list", "--json"][..],
        ] {
            let out = fx.run(args);
            assert_eq!(out.status.code(), Some(1), "{args:?}");
            assert_eq!(
                super::json(&out.stderr)["code"],
                "store_symlink",
                "{args:?}"
            );
        }
        assert_eq!(fs::read(&target).unwrap(), before);
        fs::remove_file(&entry).unwrap();
        fs::rename(&target, &entry).unwrap();

        let month = fx.root().join("sessions/2026/11");
        let moved = fx.elsewhere("month");
        fs::rename(&month, &moved).unwrap();
        symlink(&moved, &month).unwrap();
        let out = fx.run(&["session", "list", "--json"]);
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(super::json(&out.stderr)["code"], "store_symlink");

        let config = fx.root().join("config.toml");
        fs::remove_file(&config).unwrap();
        symlink(fx.elsewhere("decisions-2.md"), &config).unwrap();
        let out = fx.run(&["tag", "list", "--json"]);
        assert_eq!(super::json(&out.stderr)["code"], "store_symlink");
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
    fn every_command_works_against_a_read_only_store_and_changes_nothing() {
        // STD-01 R31: reads take no lock and write nothing.
        let fx = Fixture::new();
        fx.seed();
        let root = fx.root();
        set_tree(&root, true);
        let before = snapshot(&root);
        let outs: Vec<_> = super::all_reads()
            .into_iter()
            .map(|args| {
                let out = fx.run(&args);
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
