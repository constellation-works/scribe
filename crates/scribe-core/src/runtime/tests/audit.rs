use crate::apps::entry::{EntryWritten, Outcome as Output};
use crate::apps::hook::{HookReply, Outcome as HookOutput};
use crate::runtime::audit::{AuditGuard, AuditMeta, HookOutcome};
use crate::runtime::errors::CommandError as CliError;
use scribe_core::testing::StoreTestReads;
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

/// Seed `preferences-1`, whose content is `secret content`, and show it.
fn shown_entry(store: &Store) -> Output {
    let text = format!(
        "---
format: 31
kind: entry
id: preferences-1
tag: preferences
status: active
project: scribe
topics: []
related: []
provider: claude-code
host: host
author: {{kind: user, name: user}}
updated_by: null
created: 2026-09-26T12:00:00Z
updated: 2026-09-26T12:00:00Z
inactive_since: null
said: \"#scribe preferences: secret content\"
content_from: {{kind: instruction}}
cite: {{session: s-1, message: null, at: 2026-09-26T12:00:00Z, prompt_sha256: \"{}\", line: 1, cwd: /work}}
retirement_reason: null
superseded_by: null
review_after: null
quote: null
window: null
evidence: null
as_of: null
approved_by: null
instruction: null
removal_instruction: null
stated_by: null
outcome: null
---
secret content
",
        "0".repeat(64)
    );
    store.seed_records(&[&text]).unwrap();
    let entry = store.entry(&"preferences-1".parse().unwrap()).unwrap();
    Output::Written(Box::new(EntryWritten {
        entry,
        changed: true,
        action: None,
        notices: Vec::new(),
    }))
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
    guard.mark_result(&Ok::<_, CliError>(output));
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
    guard.mark_result(&Err::<Output, _>(err));
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
    guard.mark_result(&Err::<Output, _>(CliError::NoDataDir));
    drop(guard);
    assert_eq!(std::fs::read(&file).unwrap(), b"");
}

fn hook_reply(outcomes: Vec<HookOutcome>) -> HookOutput {
    HookOutput::Replied(Box::new(HookReply {
        event: "UserPromptSubmit",
        message: Some(String::from(
            "scribe: added preferences-1 to preferences — \"secret content\"",
        )),
        context: None,
        block: None,
        outcomes,
    }))
}

#[test]
fn a_hook_writes_one_line_per_write_or_refusal_with_ids_and_codes_only() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let mut guard = AuditGuard::start(&store, ADD, datetime!(2026-09-26 12:00:00 UTC));
    guard.mark_result(&Ok::<_, CliError>(hook_reply(vec![
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
    guard.mark_result(&Ok::<_, CliError>(hook_reply(Vec::new())));
    drop(guard);
    assert!(!store.audit_path().exists());
}

#[test]
fn committed_flag_suppresses_guard_lines_on_mark_result_and_drop() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let committed = Arc::new(AtomicBool::new(false));

    let Output::Written(entry) = shown_entry(&store) else {
        panic!("expected shown entry");
    };
    let mut guard = AuditGuard::start(&store, ADD, datetime!(2026-09-26 12:00:00 UTC));
    guard.set_committed_flag(Arc::clone(&committed));
    committed.store(true, Ordering::Release);
    guard.mark_result(&Ok::<_, CliError>(Output::Written(entry)));
    drop(guard);
    assert!(store.audit_events().unwrap().0.is_empty());

    // Even if a panic unwinds before mark_result runs, Drop writes nothing once committed.
    let mut unwind_guard = AuditGuard::start(&store, ADD, datetime!(2026-09-26 12:00:00 UTC));
    unwind_guard.set_committed_flag(committed);
    drop(unwind_guard);
    assert!(store.audit_events().unwrap().0.is_empty());
}

#[test]
fn only_the_hook_commands_are_audited_because_every_other_command_reads() {
    // STD-01 R31: a read writes nothing, not even an audit line.
    use crate::apps::{entry, proposal, session, tag};
    use crate::{DateRange, Provider, SessionId};
    let provider = Provider::ClaudeCode;
    let id = |id: &str| id.parse::<SessionId>().unwrap();
    for (command, audit) in [
        (
            "entry list",
            entry::Command::List(Box::new(entry::ListInput {
                filter: crate::EntryFilter::default(),
                since: None,
                until: None,
                group: None,
                provider,
            }))
            .audit(),
        ),
        (
            "entry show",
            entry::Command::Show(String::from("preferences-1")).audit(),
        ),
        (
            "session list",
            session::Command::List(Box::new(session::ListInput {
                filter: crate::SessionFilter::default(),
                since: None,
                until: None,
                group: None,
                provider,
            }))
            .audit(),
        ),
        (
            "session show",
            session::Command::Show {
                id: id("s-1"),
                segment: None,
            }
            .audit(),
        ),
        (
            "proposal list",
            proposal::Command::List(proposal::ListInput {
                filter: crate::ProposalFilter::default(),
                entry: None,
                since: None,
                until: None,
            })
            .audit(),
        ),
        ("tag list", tag::Command::List.audit()),
        (
            "transcript list",
            session::Command::TranscriptList {
                provider,
                input: session::TranscriptListInput {
                    since: time::macros::date!(2026 - 10 - 01),
                    dates: DateRange::default(),
                    projects: &[],
                    min_turns: 20,
                    limit: None,
                },
            }
            .audit(),
        ),
        (
            "transcript show",
            session::Command::TranscriptShow {
                provider,
                id: id("abcd"),
            }
            .audit(),
        ),
    ] {
        assert!(audit.is_none(), "{command}");
    }
}

#[test]
fn both_hook_commands_are_audited_under_their_command_path() {
    use crate::apps::hook::Event;
    use crate::apps::session;
    use crate::{AuthorName, Label, Provider, SessionId};
    let mut read_past = |_: Provider, _: &SessionId, _: &std::path::Path| Ok(None);
    let (id, author): (SessionId, AuthorName) =
        ("abcd".parse().unwrap(), "daniel".parse().unwrap());
    let catalog = session::Command::Catalog(session::CatalogInput {
        id: &id,
        author: &author,
        project: None,
        provider: Provider::ClaudeCode,
        host: Label::sanitized("host", "host"),
        models: Box::new(|_| unreachable!("audit() never runs the command")),
    });
    for (audit, path) in [
        (
            Event::UserPromptSubmit {
                input: Ok(Vec::new()),
                read_past: &mut read_past,
            }
            .audit(),
            "hook user-prompt-submit",
        ),
        (Event::Stop(Ok(Vec::new())).audit(), "hook stop"),
        (catalog.audit(), "session catalog"),
    ] {
        assert_eq!(audit.map(|m| m.command), Some(path));
    }
}
