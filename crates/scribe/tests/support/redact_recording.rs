use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The search path the stub `cargo` is prepended to. A harness read, not a
/// variable the shipped program reads.
const PATH_ENV: &str = "PATH";

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
/// first on `PATH`. Check its exit code and return the directory the stub wrote to.
#[track_caller]
pub(crate) fn check(args: &[&Path], stdin: &str, exit: i32) -> tempfile::TempDir {
    let work = tempfile::tempdir().unwrap();
    let bin = work.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let cargo = bin.join("cargo");
    fs::write(&cargo, STUB_CARGO).unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var(PATH_ENV).unwrap_or_default()
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
    assert_eq!(
        out.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    work
}
