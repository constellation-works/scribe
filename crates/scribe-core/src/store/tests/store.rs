use super::{SESSION_BODY, entry_text, private_tempdir, session_text};
use crate::entry::{EntryFilter, EntryStatus};
use crate::error::Error;
use crate::session::SessionFilter;
use crate::store::fsio::LockGuard;
use crate::store::{AuditEvent, AuditStatus, Store};
use std::fs;
use std::path::Path;
use std::time::Duration;
use time::OffsetDateTime;

fn event(command: &str) -> AuditEvent {
    AuditEvent {
        at: OffsetDateTime::UNIX_EPOCH,
        command: command.to_owned(),
        status: AuditStatus::Success,
        target: None,
        error_code: None,
        duration_ms: 0,
    }
}

/// Write `text` at `rel` under `root`, creating owner-only directories.
fn put(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    let mut dir = path.parent().unwrap().to_path_buf();
    let mut made = Vec::new();
    while !dir.exists() {
        made.push(dir.clone());
        dir = dir.parent().unwrap().to_path_buf();
    }
    for d in made.iter().rev() {
        fs::create_dir(d).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn a_missing_root_reads_as_an_empty_store() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("absent"));
    assert!(
        store
            .list_entries(&EntryFilter::default())
            .unwrap()
            .listing
            .items
            .is_empty()
    );
    assert!(
        store
            .list_sessions(&SessionFilter::default())
            .unwrap()
            .listing
            .items
            .is_empty()
    );
    assert!(store.proposals().unwrap().is_empty());
    assert!(store.control().unwrap().sessions.is_empty());
    let tags = store.list_tags().unwrap();
    assert_eq!(tags.tags.len(), 2, "the default tags, with no entries");
    let err = store.entry(&"decisions-1".parse().unwrap()).unwrap_err();
    assert_eq!(err.code(), "entry_not_found");
    assert!(
        !dir.path().join("absent").exists(),
        "reads never create the root"
    );
}

#[test]
fn a_corrupt_entry_is_skipped_in_a_list_and_refused_by_show() {
    let dir = private_tempdir();
    put(
        dir.path(),
        "entries/decisions/decisions-1.md",
        &entry_text(""),
    );
    put(
        dir.path(),
        "entries/decisions/decisions-2.md",
        &entry_text("colour: blue\n").replace("decisions-1", "decisions-2"),
    );
    put(
        dir.path(),
        "entries/decisions/.decisions-3.md.tmp",
        "partial",
    );
    put(dir.path(), "entries/decisions/notes.txt", "not a record");
    let store = Store::open(dir.path());
    let list = store.list_entries(&EntryFilter::default()).unwrap();
    assert_eq!(list.listing.items.len(), 1);
    assert_eq!(list.skipped.len(), 1);
    assert!(list.skipped[0].path.ends_with("decisions-2.md"));
    assert_eq!(list.skipped[0].error.code(), "store_corrupt");
    let err = store.entry(&"decisions-2".parse().unwrap()).unwrap_err();
    assert_eq!(err.code(), "store_corrupt");
    let shown = store.entry(&"decisions-1".parse().unwrap()).unwrap();
    assert_eq!(shown.status, EntryStatus::Active);
}

#[test]
fn a_session_is_shown_with_its_switch() {
    let dir = private_tempdir();
    put(
        dir.path(),
        "sessions/2026/11/s-1.md",
        &session_text("", SESSION_BODY),
    );
    put(
        dir.path(),
        "control.json",
        r#"{"format":1,"sessions":{"s-1":{"on":true,"since":"2026-11-01T09:00:00Z","due":true,"final":false}}}"#,
    );
    let store = Store::open(dir.path());
    let view = store.session(&"s-1".parse().unwrap()).unwrap();
    assert_eq!(view.record.outcome, "out");
    assert!(view.control.unwrap().due);
    let err = store.session(&"s-2".parse().unwrap()).unwrap_err();
    assert_eq!(err.code(), "session_not_found");
    let list = store.list_sessions(&SessionFilter::default()).unwrap();
    assert_eq!(list.listing.items.len(), 1);
}

#[test]
fn a_corrupt_config_refuses_every_read_naming_the_file() {
    let dir = private_tempdir();
    put(dir.path(), "config.toml", "[models]\nanswer = \"x\"\n");
    let err = Store::open(dir.path()).list_tags().unwrap_err();
    assert_eq!(err.code(), "config_invalid");
    assert!(err.to_string().contains("config.toml"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_linked_tag_directory_is_refused_not_followed() {
    let dir = private_tempdir();
    put(dir.path(), "elsewhere/decisions-1.md", &entry_text(""));
    put(dir.path(), "entries/.keep", "");
    std::os::unix::fs::symlink(
        dir.path().join("elsewhere"),
        dir.path().join("entries/decisions"),
    )
    .unwrap();
    let err = Store::open(dir.path())
        .list_entries(&EntryFilter::default())
        .unwrap_err();
    assert_eq!(err.code(), "store_symlink", "got {err:?}");
}

#[test]
fn record_audit_appends_one_line_per_event_and_creates_the_directory() {
    let dir = private_tempdir();
    let root = dir.path().join("data");
    let store = Store::open(&root);
    store
        .record_audit(&event("hook user-prompt-submit"))
        .unwrap();
    store
        .record_audit(&event("hook user-prompt-submit"))
        .unwrap();
    let text = fs::read_to_string(store.audit_path()).unwrap();
    assert_eq!(text.lines().count(), 2, "got {text:?}");
    assert!(text.ends_with('\n'));
}

#[test]
fn record_audit_waits_for_the_store_lock() {
    // Events and record writes share one lock, so a log line is never
    // written while another command is mid-write.
    let dir = private_tempdir();
    let store = Store::open(dir.path()).with_lock_wait(Duration::from_millis(60));
    let _held =
        LockGuard::acquire(&dir.path().join(".lock"), "test", Duration::from_secs(1)).unwrap();
    let err = store
        .record_audit(&event("hook user-prompt-submit"))
        .unwrap_err();
    assert!(matches!(err, Error::Locked { .. }), "got {err:?}");
    assert!(!store.audit_path().exists());
}
