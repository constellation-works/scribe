//! `scripts/redact-recording.sh` takes the redaction key on stdin, never in
//! argv, where `ps` shows it to every user (STD-05 R12). Each test runs the
//! script with a stub `cargo` first on `PATH` that records what it was given,
//! so no build or redaction runs.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const KEY: &str = "stdin-only-redaction-key";

/// Records argv, stdin and whether the key is in its environment, then
/// succeeds.
const STUB_CARGO: &str = r#"#!/bin/sh
printf '%s\n' "$@" > "$STUB_OUT/argv"
cat > "$STUB_OUT/stdin"
printf '%s' "${SCRIBE_REDACT_KEY-unset}" > "$STUB_OUT/env-key"
"#;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/redact-recording.sh")
}

/// Run the script with `args`, `stdin` on its stdin and the stub `cargo`
/// first on `PATH`. Returns the output and the directory the stub wrote to.
fn run_script(args: &[&Path], stdin: &str) -> (Output, tempfile::TempDir) {
    let work = tempfile::tempdir().unwrap();
    let bin = work.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let cargo = bin.join("cargo");
    fs::write(&cargo, STUB_CARGO).unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut child = Command::new("bash")
        .arg(script())
        .args(args)
        .env("PATH", path)
        .env("STUB_OUT", work.path())
        .env_remove("SCRIBE_REDACT_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (out, work)
}

#[test]
fn the_key_arrives_on_stdin_and_never_in_argv_or_the_environment() {
    let recording = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let to = parent.path().join("redacted");
    let (out, work) = run_script(&[recording.path(), to.as_path()], &format!("{KEY}\n"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    let (out, work) = run_script(&args, "");
    assert_eq!(out.status.code(), Some(2), "usage");
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
    let (out, work) = run_script(&[recording.path(), to.as_path()], "\n");
    assert_eq!(out.status.code(), Some(2));
    assert!(!work.path().join("argv").exists(), "cargo ran with no key");
}
