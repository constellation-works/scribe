use crate::audit_middleware::{AuditGuard, AuditMeta};
use crate::error::CliError;
use crate::output::{EntryPayload, HookOutcome, HookReply, Output, Record};
use scribe_core::{AuditStatus, Store};
use std::path::PathBuf;
use time::macros::datetime;

const ADD: AuditMeta = AuditMeta {
    command: "hook user-prompt-submit",
    stepwise: false,
};

/// An owner-only temporary directory, as a store directory must be.
fn private_tempdir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

fn lines(store: &Store) -> Vec<serde_json::Value> {
    let (events, torn) = store.audit_events().unwrap();
    assert_eq!(torn, 0);
    events
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect()
}

/// Write `preferences-1`, whose content is `secret content`, and show it.
fn shown_entry(store: &Store) -> Output {
    let dir = store.root().join("entries/preferences");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for d in [store.root().join("entries"), dir.clone()] {
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let path = dir.join("preferences-1.md");
    let text = format!(
        "---
format: 1
kind: entry
id: preferences-1
tag: preferences
status: active
project: scribe
topics: []
related: []
provider: claude-code
host: host
created: 2026-09-26T12:00:00Z
updated: 2026-09-26T12:00:00Z
said: \"#scribe preferences: secret content\"
content_from: {{kind: instruction}}
cite: {{session: s-1, message: null, at: 2026-09-26T12:00:00Z, prompt_sha256: \"{}\", line: 1, cwd: /work}}
superseded_by: null
review_after: null
---
secret content
",
        "0".repeat(64)
    );
    std::fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let entry = store.entry(&"preferences-1".parse().unwrap()).unwrap();
    Output::Show {
        record: Record::Entry(Box::new(EntryPayload::from(&entry))),
    }
}

#[test]
fn an_unmarked_guard_records_a_failure() {
    // A command that returned early or panicked never reads as success.
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let guard = AuditGuard::start(&store, ADD, datetime!(2026-09-26 12:00:00 UTC));
    assert_eq!(guard.events()[0].status, AuditStatus::Failure);
    drop(guard);
    let lines = lines(&store);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["status"], "failure");
    assert_eq!(lines[0]["command"], "hook user-prompt-submit");
    assert_eq!(lines[0]["error_code"], serde_json::Value::Null);
}

#[test]
fn a_success_records_the_record_it_acted_on_and_nothing_it_contained() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let output = shown_entry(&store);
    let mut guard = AuditGuard::start(&store, ADD, datetime!(2026-09-26 12:00:00 UTC));
    guard.mark_result(&Ok(output));
    drop(guard);
    let lines = lines(&store);
    assert_eq!(lines[0]["status"], "success");
    assert_eq!(lines[0]["target"], "preferences-1");
    assert_eq!(lines[0]["at"], "2026-09-26T12:00:00Z");
    // STD-05 R13: no argument values in the log.
    let text = serde_json::Value::Array(lines.clone()).to_string();
    assert!(!text.contains("secret content"), "got {text}");
}

#[test]
fn a_failure_records_the_stable_code_never_the_message() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let err = CliError::Core(store.entry(&"preferences-9".parse().unwrap()).unwrap_err());
    let message = err.to_string();
    let mut guard = AuditGuard::start(&store, ADD, datetime!(2026-09-26 12:00:00 UTC));
    guard.mark_result(&Err(err));
    drop(guard);
    let lines = lines(&store);
    assert_eq!(lines[0]["status"], "failure");
    assert_eq!(lines[0]["error_code"], "entry_not_found");
    assert_eq!(lines[0]["target"], serde_json::Value::Null);
    let text = serde_json::Value::Array(lines.clone()).to_string();
    assert!(!text.contains(&message), "the message names a path: {text}");
}

#[test]
fn an_unwritable_audit_log_fails_open() {
    // STD-02 R31: the data directory is a regular file, so the line cannot be
    // written; dropping the guard must neither panic nor fail the command.
    let dir = private_tempdir();
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"").unwrap();
    let store = Store::open(PathBuf::from(&file));
    let mut guard = AuditGuard::start(&store, ADD, datetime!(2026-09-26 12:00:00 UTC));
    guard.mark_result(&Err(CliError::NoDataDir));
    drop(guard);
    assert_eq!(std::fs::read(&file).unwrap(), b"");
}

fn hook_reply(outcomes: Vec<HookOutcome>) -> Output {
    Output::Hook(HookReply {
        event: "UserPromptSubmit",
        message: Some(String::from(
            "scribe: added preferences-1 to preferences — \"secret content\"",
        )),
        context: None,
        block: None,
        outcomes,
    })
}

#[test]
fn a_hook_writes_one_line_per_write_or_refusal_with_ids_and_codes_only() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let mut guard = AuditGuard::start(&store, ADD, datetime!(2026-09-26 12:00:00 UTC));
    guard.mark_result(&Ok(hook_reply(vec![
        HookOutcome {
            target: Some(String::from("preferences-1")),
            error_code: None,
        },
        HookOutcome {
            target: Some(String::from("session:s-1")),
            error_code: Some("unknown_tag"),
        },
    ])));
    drop(guard);
    let lines = lines(&store);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["status"], "success");
    assert_eq!(lines[0]["target"], "preferences-1");
    assert_eq!(lines[0]["error_code"], serde_json::Value::Null);
    assert_eq!(lines[1]["status"], "failure");
    assert_eq!(lines[1]["target"], "session:s-1");
    assert_eq!(lines[1]["error_code"], "unknown_tag");
    let text = serde_json::Value::Array(lines.clone()).to_string();
    assert!(!text.contains("secret content"), "got {text}");
}

#[test]
fn a_hook_that_changed_nothing_writes_no_line() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let mut guard = AuditGuard::start(&store, ADD, datetime!(2026-09-26 12:00:00 UTC));
    guard.mark_result(&Ok(hook_reply(Vec::new())));
    drop(guard);
    assert!(!store.audit_path().exists());
}
