//! `scripts/redact-recording.sh` takes the redaction key on stdin, never in
//! argv, where `ps` shows it to every user (STD-05 R12). Each test runs the
//! script with a stub `cargo` first on `PATH` that records what it was given,
//! so no build or redaction runs.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "support/redact_recording.rs"]
mod subject;
use std::fs;
use std::path::Path;
use subject::check;

const KEY: &str = "stdin-only-redaction-key";

#[test]
fn the_key_arrives_on_stdin_and_never_in_argv_or_the_environment() {
    let recording = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let to = parent.path().join("redacted");
    let work = check(&[recording.path(), to.as_path()], &format!("{KEY}\n"), 0);
    let argv = fs::read_to_string(work.path().join("argv")).unwrap();
    assert!(!argv.contains(KEY), "the key is in argv: {argv}");
    assert_eq!(
        fs::read_to_string(work.path().join("stdin")).unwrap(),
        format!("{KEY}\n"),
        "the test run gets the key on stdin"
    );
    assert_eq!(
        fs::read_to_string(work.path().join("env-key")).unwrap(),
        "unset",
        "the key is not exported to cargo"
    );
}

#[test]
fn a_key_on_the_command_line_is_refused_before_anything_runs() {
    let recording = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let to = parent.path().join("redacted");
    let args: [&Path; 3] = [recording.path(), to.as_path(), Path::new(KEY)];
    let work = check(&args, "", 2);
    assert!(
        !work.path().join("argv").exists(),
        "cargo ran with a key on the command line"
    );
}

#[test]
fn an_empty_key_on_stdin_is_a_usage_error() {
    let recording = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let to = parent.path().join("redacted");
    let work = check(&[recording.path(), to.as_path()], "\n", 2);
    assert!(!work.path().join("argv").exists(), "cargo ran with no key");
}
