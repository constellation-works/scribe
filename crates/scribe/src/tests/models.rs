//! `ClaudeCliModels` against a fake `claude`: a shell script on the child's
//! `PATH` under a temporary `HOME`, which records its argv, environment,
//! stdin and process group there and answers as each test needs. No test
//! runs the real `claude` (2_design.md §8).
#![cfg(unix)]

use crate::models::{BASELINE_ENV, ClaudeCliModels, allowed_env, argv};
use scribe_core::Models;
use scribe_core::Speaker;
use scribe_core::models::{
    ANNOTATION_SCHEMA, AnnotationAnswer, AnnotationMessage, AnnotationRequest, MarkAnswer,
    ModelError,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A temporary `HOME` with a `bin/claude` script in it.
struct FakeClaude {
    dir: tempfile::TempDir,
}

impl FakeClaude {
    /// `body` is the script after the lines that record the call.
    fn new(body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let script = format!(
            "#!/bin/sh\n\
             for a in \"$@\"; do printf '%s\\n' \"<$a>\"; done > \"$HOME/argv\"\n\
             env > \"$HOME/env\"\n\
             echo $$ > \"$HOME/pid\"\n\
             ps -o pgid= -p $$ > \"$HOME/pgid\"\n\
             cat > \"$HOME/stdin\"\n\
             {body}\n"
        );
        let path = bin.join("claude");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir }
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    fn path_var(&self) -> OsString {
        let mut path = self.home().join("bin").into_os_string();
        path.push(":/usr/bin:/bin");
        path
    }

    /// The adapter, given the environment `extra` adds to the baseline: as
    /// the composition root builds it, from a lookup over these values.
    fn models(&self, credentials: &[&str], extra: &[(&str, &str)]) -> ClaudeCliModels {
        let mut values: BTreeMap<String, OsString> = BTreeMap::new();
        values.insert(String::from("PATH"), self.path_var());
        values.insert(String::from("HOME"), self.home().into());
        values.insert(String::from("LANG"), OsString::from("C.UTF-8"));
        for (k, v) in extra {
            values.insert((*k).to_owned(), OsString::from(*v));
        }
        let credentials: Vec<String> = credentials.iter().map(|c| (*c).to_owned()).collect();
        ClaudeCliModels::new(allowed_env(&credentials, |name| values.get(name).cloned()))
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.home().join(name)).unwrap_or_default()
    }
}

fn request(budget: Duration) -> AnnotationRequest {
    AnnotationRequest {
        model: String::from("claude-haiku-5-5"),
        budget,
        marks: Vec::new(),
        messages: vec![AnnotationMessage {
            uuid: String::from("a-1"),
            role: Speaker::Assistant,
            text: String::from("We squash-merge agent PRs."),
            marked: false,
        }],
        tags: Vec::new(),
        voice: scribe_core::voice::Voice::default(),
    }
}

const STRUCTURED: &str = r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"marks":[{"marker":1,"kind":"none","reason":"squash-merge agent PRs"}]}}'"#;

/// The answer [`STRUCTURED`] holds.
fn structured() -> AnnotationAnswer {
    AnnotationAnswer {
        marks: vec![MarkAnswer {
            marker: 1,
            kind: String::from("none"),
            reason: String::from("squash-merge agent PRs"),
            ..MarkAnswer::default()
        }],
    }
}

#[test]
fn the_argv_carries_safe_mode_and_no_tools_and_the_prompt_goes_on_stdin() {
    let fake = FakeClaude::new(STRUCTURED);
    let answer = fake
        .models(&[], &[])
        .annotation(&request(Duration::from_secs(10)))
        .unwrap();
    assert_eq!(answer, structured());
    let recorded: Vec<String> = fake.read("argv").lines().map(str::to_owned).collect();
    let expected: Vec<String> = [
        "-p",
        "--safe-mode",
        "--tools",
        "",
        "--model",
        "claude-haiku-5-5",
        "--output-format",
        "json",
        "--json-schema",
        ANNOTATION_SCHEMA,
    ]
    .iter()
    .map(|a| format!("<{a}>"))
    .collect();
    assert_eq!(recorded, expected, "`--tools \"\"` is one empty argument");
    assert_eq!(argv("claude-haiku-5-5", ANNOTATION_SCHEMA).len(), 10);
    assert_eq!(fake.read("stdin"), request(Duration::ZERO).prompt());
}

#[test]
fn the_child_gets_only_the_allowlisted_environment() {
    let fake = FakeClaude::new(STRUCTURED);
    fake.models(
        &[
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "SCRIBE_TOKEN",
        ],
        &[
            ("CLAUDE_CODE_OAUTH_TOKEN", "oauth-value"),
            ("SCRIBE_TOKEN", "own-namespace"),
            ("GITHUB_TOKEN", "not-configured"),
        ],
    )
    .annotation(&request(Duration::from_secs(10)))
    .unwrap();
    let env: BTreeMap<String, String> = fake
        .read("env")
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    // The shell adds a few of its own (PWD, SHLVL, `_`); nothing else from
    // the test process (CARGO_*, RUST_*, the real HOME) arrives.
    let shell_own = ["PWD", "OLDPWD", "SHLVL", "_"];
    let names: Vec<&str> = env
        .keys()
        .map(String::as_str)
        .filter(|k| !shell_own.contains(k))
        .collect();
    assert_eq!(names, ["CLAUDE_CODE_OAUTH_TOKEN", "HOME", "LANG", "PATH"]);
    assert_eq!(env["CLAUDE_CODE_OAUTH_TOKEN"], "oauth-value");
    assert_eq!(Path::new(&env["HOME"]), fake.home());
    assert!(
        !fake.read("argv").contains("oauth-value"),
        "the credential is never in argv"
    );
    assert_eq!(BASELINE_ENV, ["PATH", "HOME", "LANG"]);
}

#[test]
fn the_child_leads_its_own_process_group() {
    let fake = FakeClaude::new(STRUCTURED);
    fake.models(&[], &[])
        .annotation(&request(Duration::from_secs(10)))
        .unwrap();
    let pid = fake.read("pid").trim().to_owned();
    assert!(!pid.is_empty());
    assert_eq!(fake.read("pgid").trim(), pid);
    assert_ne!(
        pid,
        std::process::id().to_string(),
        "a child, not this process"
    );
}

/// Whether `pid` names a live (or unreaped) process.
fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Wait up to five seconds for `pid` to be gone: a grandchild orphaned by
/// the kill is reaped by init, not by us.
fn gone_soon(pid: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn at_its_budget_the_whole_group_is_killed_and_the_child_reaped() {
    // The script and everything it starts ignore SIGTERM, so only the
    // SIGKILL after the grace period ends them; the grandchild proves the
    // signal went to the group, not just the child.
    let fake = FakeClaude::new(
        "trap '' TERM\n\
         sleep 300 &\n\
         echo $! > \"$HOME/grandchild\"\n\
         sleep 300",
    );
    let budget = Duration::from_millis(500);
    let started = Instant::now();
    let err = fake
        .models(&[], &[])
        .annotation(&request(budget))
        .unwrap_err();
    let took = started.elapsed();
    assert_eq!(err, ModelError::Timeout { budget });
    assert!(took >= budget, "{took:?}");
    assert!(
        took < Duration::from_secs(10),
        "killed, not waited out: {took:?}"
    );
    let pid = fake.read("pid").trim().to_owned();
    let grandchild = fake.read("grandchild").trim().to_owned();
    assert!(!grandchild.is_empty(), "the grandchild started");
    assert!(
        !alive(&pid),
        "the child was reaped before the call returned (a zombie would still answer kill -0)"
    );
    assert!(
        gone_soon(&grandchild),
        "the grandchild was killed with the group"
    );
}

#[test]
fn a_clean_exit_still_sweeps_what_the_child_left_running() {
    let fake = FakeClaude::new(&format!(
        "sleep 300 &\necho $! > \"$HOME/grandchild\"\n{STRUCTURED}"
    ));
    let started = Instant::now();
    fake.models(&[], &[])
        .annotation(&request(Duration::from_secs(10)))
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(8));
    let grandchild = fake.read("grandchild").trim().to_owned();
    assert!(
        gone_soon(&grandchild),
        "the group is swept on a clean exit too"
    );
}

#[test]
fn a_failing_claude_is_a_model_error_naming_its_last_words() {
    let fake = FakeClaude::new("echo 'Not logged in. Run /login' >&2\nexit 3");
    let err = fake
        .models(&[], &[])
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    let ModelError::Failed(message) = &err else {
        panic!("expected a failure: {err:?}");
    };
    assert!(message.contains("exit status: 3"), "{message}");
    assert!(message.contains("Not logged in"), "{message}");
}

/// A hook started by an app that was not launched from a login shell (the
/// Claude desktop app) lacks the credential the login shell exports, so
/// `claude` exits refusing to run: the failure names the missing variables,
/// and only when none of them is set.
#[test]
fn a_failing_claude_names_the_credentials_missing_from_the_environment() {
    let fake = FakeClaude::new("echo 'Not logged in. Run /login' >&2\nexit 1");
    let names = [
        String::from("CLAUDE_CODE_OAUTH_TOKEN"),
        String::from("ANTHROPIC_API_KEY"),
    ];
    let err = fake
        .models(&["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"], &[])
        .with_credentials(&names)
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    let ModelError::Failed(message) = &err else {
        panic!("expected a failure: {err:?}");
    };
    assert!(
        message.starts_with(
            "claude ended with exit status: 1 (none of CLAUDE_CODE_OAUTH_TOKEN, ANTHROPIC_API_KEY is set in scribe's environment): Not logged in"
        ),
        "{message}"
    );

    let err = with_oauth(&fake)
        .with_credentials(&names)
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    let ModelError::Failed(message) = &err else {
        panic!("expected a failure: {err:?}");
    };
    assert!(
        !message.contains("is set in scribe's environment"),
        "{message}"
    );
}

#[test]
fn an_error_result_is_a_failure_and_a_bad_answer_is_malformed() {
    let fake = FakeClaude::new(
        r#"echo '{"type":"result","is_error":true,"result":"API Error: overloaded"}'"#,
    );
    let err = fake
        .models(&[], &[])
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    assert_eq!(
        err,
        ModelError::Failed(String::from("API Error: overloaded"))
    );

    for body in [
        "echo 'not json'",
        r#"echo '{"type":"result","is_error":false,"result":"no json here"}'"#,
        r#"echo '{"type":"result","is_error":false,"structured_output":{"message":"a-1"}}'"#,
    ] {
        let err = FakeClaude::new(body)
            .models(&[], &[])
            .annotation(&request(Duration::from_secs(10)))
            .unwrap_err();
        assert!(matches!(err, ModelError::Malformed(_)), "{body}: {err:?}");
    }
}

#[test]
fn an_answer_in_the_result_text_is_read_when_there_is_no_structured_output() {
    let fake = FakeClaude::new(
        r#"printf '%s\n' '{"type":"result","is_error":false,"result":"```json\n{\"marks\":[{\"marker\":1,\"kind\":\"none\",\"reason\":\"squash-merge agent PRs\"}]}\n```"}'"#,
    );
    let answer = fake
        .models(&[], &[])
        .annotation(&request(Duration::from_secs(10)))
        .unwrap();
    assert_eq!(answer, structured());
}

#[test]
fn no_claude_on_path_is_unavailable() {
    let fake = FakeClaude::new(STRUCTURED);
    let empty: PathBuf = fake.home().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let models = ClaudeCliModels::new(vec![(OsString::from("PATH"), empty.into_os_string())]);
    let err = models
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    assert!(matches!(err, ModelError::Unavailable(_)), "{err:?}");
}

/// A credential-shaped value for the child's environment; synthetic, never real.
const SYNTHETIC_OAUTH: &str = "synthetic-oauth-value-42";

/// The adapter for one credential, `CLAUDE_CODE_OAUTH_TOKEN`, set to
/// [`SYNTHETIC_OAUTH`] in the child's environment.
fn with_oauth(fake: &FakeClaude) -> ClaudeCliModels {
    fake.models(
        &["CLAUDE_CODE_OAUTH_TOKEN"],
        &[("CLAUDE_CODE_OAUTH_TOKEN", SYNTHETIC_OAUTH)],
    )
}

#[test]
fn a_failing_claude_masks_a_configured_credential_in_its_stderr() {
    let fake = FakeClaude::new("echo \"auth failed for $CLAUDE_CODE_OAUTH_TOKEN\" >&2\nexit 3");
    let err = with_oauth(&fake)
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    let ModelError::Failed(message) = &err else {
        panic!("expected a failure: {err:?}");
    };
    assert!(message.contains("exit status: 3"), "{message}");
    assert!(
        message.ends_with("auth failed for [redacted:CLAUDE_CODE_OAUTH_TOKEN]"),
        "{message}"
    );
    assert!(!message.contains(SYNTHETIC_OAUTH), "{message}");
}

#[test]
fn a_failing_claude_with_only_stdout_masks_the_shapes_it_prints() {
    // Stderr is empty, so the diagnostic is the last line of stdout.
    let fake = FakeClaude::new(
        "echo 'token ghp_0123456789abcdefghij leaked, Authorization: Bearer abcdef0123456789'\nexit 3",
    );
    let err = fake
        .models(&[], &[])
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    let ModelError::Failed(message) = &err else {
        panic!("expected a failure: {err:?}");
    };
    assert!(
        message.ends_with(
            "token [redacted:github-token] leaked, Authorization: Bearer [redacted:bearer]"
        ),
        "{message}"
    );
    assert!(!message.contains("ghp_0123"), "{message}");
    assert!(!message.contains("abcdef0123456789"), "{message}");
}

#[test]
fn an_error_result_masks_the_credential_and_the_shape_it_echoes() {
    let fake = FakeClaude::new(
        r#"printf '{"type":"result","is_error":true,"result":"API Error: token %s rejected, key sk-ant-api03-SYNTHETICTAIL123"}\n' "$CLAUDE_CODE_OAUTH_TOKEN""#,
    );
    let err = with_oauth(&fake)
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    assert_eq!(
        err,
        ModelError::Failed(String::from(
            "API Error: token [redacted:CLAUDE_CODE_OAUTH_TOKEN] rejected, key [redacted:api-key]"
        ))
    );
}

#[test]
fn a_mistyped_answer_does_not_echo_the_credential_in_its_error() {
    // serde names the value of a mistyped field; the error must not carry it.
    let fake = FakeClaude::new(
        r#"printf '{"type":"result","is_error":false,"structured_output":{"marks":[{"marker":"%s","kind":"none","reason":"r"}]}}\n' "$CLAUDE_CODE_OAUTH_TOKEN""#,
    );
    let err = with_oauth(&fake)
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    let ModelError::Malformed(message) = &err else {
        panic!("expected a malformed answer: {err:?}");
    };
    assert!(
        message.contains("[redacted:CLAUDE_CODE_OAUTH_TOKEN]"),
        "{message}"
    );
    assert!(!message.contains(SYNTHETIC_OAUTH), "{message}");
}

#[test]
fn a_stderr_cut_at_its_limit_does_not_report_the_credential_it_cuts() {
    // 65,526 `x` bytes and a newline end at byte 65,527, so the credential
    // starts there and only its first nine bytes fit under the 64 KiB limit.
    // That partial line must not be reported: the credential is never
    // complete in what the child wrote.
    let fake = FakeClaude::new(
        r#"{ head -c 65526 /dev/zero | tr '\0' x; echo; printf '%s\n' "$CLAUDE_CODE_OAUTH_TOKEN"; } >&2
exit 3"#,
    );
    let err = with_oauth(&fake)
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    let ModelError::Failed(message) = &err else {
        panic!("expected a failure: {err:?}");
    };
    assert!(message.contains("exit status: 3"), "{message}");
    assert!(!message.contains("synthetic"), "{message}");
}

#[test]
fn a_configured_secret_the_child_never_received_is_masked_too() {
    // A `[redact] env` value is not in the child's environment, but a
    // provider error can echo prompt text that holds it. The composition root
    // passes every configured secret name, and that set masks it.
    let fake = FakeClaude::new("echo 'prompt echoed synthetic-redact-value-99' >&2\nexit 3");
    let redactor = scribe_core::Redactor::new([(
        String::from("MY_REDACTED_NAME"),
        String::from("synthetic-redact-value-99"),
    )]);
    let err = fake
        .models(&[], &[])
        .with_redactor(redactor)
        .annotation(&request(Duration::from_secs(10)))
        .unwrap_err();
    let ModelError::Failed(message) = &err else {
        panic!("expected a failure: {err:?}");
    };
    assert!(
        message.ends_with("prompt echoed [redacted:MY_REDACTED_NAME]"),
        "{message}"
    );
}
