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
    assert_eq!(view.record.log[0].text, "sum\n\nout");
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

mod capture {
    use super::put;
    use crate::entry::{Cite, ContentFrom, EntryFilter, NewEntry};
    use crate::error::Error;
    use crate::store::tests::{entry_text, private_tempdir};
    use crate::store::{Captured, Redactor, Store};
    use std::fs;
    use time::macros::datetime;

    fn draft(tag: &str, line: u32, content: &str) -> NewEntry {
        let at = datetime!(2026-11-02 19:05:40 UTC);
        NewEntry {
            tag: tag.parse().unwrap(),
            project: "scribe".parse().unwrap(),
            topics: crate::record::Topics::default(),
            related: Vec::new(),
            provider: String::from("claude-code").try_into().unwrap(),
            host: String::from("host").try_into().unwrap(),
            created: at,
            said: format!("#scribe {tag}: {content}"),
            content: content.to_owned(),
            content_from: ContentFrom::Instruction,
            cite: Cite {
                session: "s-9".parse().unwrap(),
                message: None,
                at,
                prompt_sha256: "a".repeat(64),
                line,
                cwd: String::from("/work"),
            },
            review_after: None,
        }
    }

    fn ids(results: &[Captured]) -> Vec<String> {
        results.iter().map(|c| c.entry().id.to_string()).collect()
    }

    #[test]
    fn a_draft_becomes_the_next_entry_of_its_tag_and_reads_back() {
        let dir = private_tempdir();
        let root = dir.path().join("root");
        let store = Store::open(&root);
        let added = store
            .capture(
                vec![draft("decisions", 1, "one"), draft("preferences", 2, "two")],
                &Redactor::default(),
            )
            .unwrap();
        assert!(matches!(added[0], Captured::Added(_)));
        assert_eq!(ids(&added), ["decisions-1", "preferences-1"]);
        let read = store.entry(&"decisions-1".parse().unwrap()).unwrap();
        assert_eq!(&read, added[0].entry());
        assert_eq!(read.content, "one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&root), 0o700);
            assert_eq!(mode(&root.join("entries/decisions")), 0o700);
            assert_eq!(mode(&store.entry_path(&read.id)), 0o600);
        }
    }

    #[test]
    fn the_same_line_of_the_same_prompt_is_never_written_twice() {
        let dir = private_tempdir();
        let store = Store::open(dir.path());
        store
            .capture(vec![draft("decisions", 1, "one")], &Redactor::default())
            .unwrap();
        let again = store
            .capture(
                vec![draft("decisions", 1, "one"), draft("decisions", 2, "two")],
                &Redactor::default(),
            )
            .unwrap();
        assert!(matches!(again[0], Captured::Existing(_)));
        assert!(matches!(again[1], Captured::Added(_)));
        assert_eq!(ids(&again), ["decisions-1", "decisions-2"]);
        let listed = store.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(listed.listing.total, 2);
    }

    #[test]
    fn a_number_held_by_a_file_that_does_not_parse_is_not_reused() {
        let dir = private_tempdir();
        let root = dir.path();
        put(root, "entries/decisions/decisions-1.md", &entry_text(""));
        put(root, "entries/decisions/decisions-4.md", "not an entry");
        let store = Store::open(root);
        let added = store
            .capture(vec![draft("decisions", 7, "new")], &Redactor::default())
            .unwrap();
        assert_eq!(ids(&added), ["decisions-5"]);
        assert_eq!(
            fs::read_to_string(root.join("entries/decisions/decisions-4.md")).unwrap(),
            "not an entry"
        );
    }

    #[test]
    fn free_text_is_redacted_before_it_is_written() {
        let dir = private_tempdir();
        let store = Store::open(dir.path());
        let secret = "sk-test-0123456789";
        let redactor = Redactor::new([(String::from("ANTHROPIC_API_KEY"), secret.to_owned())]);
        store
            .capture(
                vec![draft("decisions", 1, &format!("use {secret} here"))],
                &redactor,
            )
            .unwrap();
        let text = fs::read_to_string(dir.path().join("entries/decisions/decisions-1.md")).unwrap();
        assert!(!text.contains(secret), "{text}");
        assert!(
            text.contains("use [redacted:ANTHROPIC_API_KEY] here"),
            "{text}"
        );
    }

    #[test]
    fn an_unsafe_root_is_refused_and_nothing_is_written() {
        let dir = private_tempdir();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).unwrap();
            let err = Store::open(dir.path())
                .capture(vec![draft("decisions", 1, "one")], &Redactor::default())
                .unwrap_err();
            assert!(matches!(err, Error::Permissions { .. }), "{err:?}");
            assert!(!dir.path().join("entries").exists());
        }
    }

    #[test]
    fn cites_of_the_session_are_filled_once_and_others_are_left_alone() {
        let dir = private_tempdir();
        let store = Store::open(dir.path());
        let mut other = draft("decisions", 1, "other session");
        other.cite.session = "s-other".parse().unwrap();
        store
            .capture(
                vec![
                    draft("decisions", 1, "one"),
                    draft("decisions", 2, "two"),
                    other,
                ],
                &Redactor::default(),
            )
            .unwrap();
        let session = "s-9".parse().unwrap();
        assert!(store.has_unfilled_cites(&session).unwrap());
        let now = datetime!(2026-11-02 19:06:00 UTC);
        let filled = store
            .fill_cite_messages(&session, now, |cite| {
                (cite.line == 1).then(|| String::from("m-1"))
            })
            .unwrap();
        let filled: Vec<String> = filled.iter().map(ToString::to_string).collect();
        assert_eq!(filled, ["decisions-1"]);
        let one = store.entry(&"decisions-1".parse().unwrap()).unwrap();
        assert_eq!(one.cite.message.as_deref(), Some("m-1"));
        assert_eq!(one.updated, now);
        let two = store.entry(&"decisions-2".parse().unwrap()).unwrap();
        assert_eq!(two.cite.message, None, "no match leaves it waiting");
        let three = store.entry(&"decisions-3".parse().unwrap()).unwrap();
        assert_eq!(three.cite.message, None, "another session is not touched");
        // A filled cite is not filled again.
        let again = store
            .fill_cite_messages(&session, now, |_| Some(String::from("m-2")))
            .unwrap();
        let again: Vec<String> = again.iter().map(ToString::to_string).collect();
        assert_eq!(again, ["decisions-2"]);
        assert!(!store.has_unfilled_cites(&session).unwrap());
    }

    #[test]
    fn entries_from_one_prompt_are_found_by_session_and_hash_without_writing() {
        let dir = private_tempdir();
        let store = Store::open(dir.path().join("absent"));
        let session = "s-9".parse().unwrap();
        assert!(
            store
                .entries_from_prompt(&session, &"a".repeat(64))
                .unwrap()
                .is_empty()
        );
        assert!(
            !dir.path().join("absent").exists(),
            "a read creates nothing"
        );
        let mut other_prompt = draft("decisions", 1, "other prompt");
        other_prompt.cite.prompt_sha256 = "b".repeat(64);
        let mut other_session = draft("decisions", 1, "other session");
        other_session.cite.session = "s-other".parse().unwrap();
        let store = Store::open(dir.path());
        store
            .capture(
                vec![
                    draft("decisions", 1, "one"),
                    draft("preferences", 2, "two"),
                    other_prompt,
                    other_session,
                ],
                &Redactor::default(),
            )
            .unwrap();
        let mut found: Vec<String> = store
            .entries_from_prompt(&session, &"a".repeat(64))
            .unwrap()
            .iter()
            .map(|e| e.id.to_string())
            .collect();
        found.sort();
        assert_eq!(found, ["decisions-1", "preferences-1"]);
    }

    #[test]
    fn checking_for_waiting_cites_writes_nothing() {
        let dir = private_tempdir();
        let root = dir.path().join("absent");
        let store = Store::open(&root);
        assert!(!store.has_unfilled_cites(&"s-9".parse().unwrap()).unwrap());
        let filled = store
            .fill_cite_messages(
                &"s-9".parse().unwrap(),
                datetime!(2026-11-02 19:06:00 UTC),
                |_| Some(String::from("m")),
            )
            .unwrap();
        assert!(filled.is_empty());
        assert!(!root.exists());
    }
}

#[test]
fn update_control_writes_only_a_change() {
    use crate::control::{self, Switch};
    let dir = private_tempdir();
    let root = dir.path().join("root");
    let store = Store::open(&root);
    let s: crate::session::SessionId = "s-1".parse().unwrap();
    let t0 = time::macros::datetime!(2026-11-01 10:00 UTC);

    // Nothing changed: no control.json is written.
    let off = store
        .update_control(|state| control::switch(state, &s, &Switch::Off, t0))
        .unwrap();
    assert!(!off.changed());
    assert!(!root.join("control.json").exists());

    store
        .update_control(|state| control::switch(state, &s, &Switch::On, t0))
        .unwrap();
    let path = root.join("control.json");
    let written = fs::read(&path).unwrap();
    let on = store.control().unwrap();
    assert!(on.get(&s).is_some_and(|c| c.on && c.due));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    store
        .update_control(|state| control::switch(state, &s, &Switch::On, t0))
        .unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        written,
        "on twice rewrites nothing"
    );
}

#[test]
fn settle_pass_writes_the_record_and_clears_due_in_one_call() {
    use crate::control::{self, Switch};
    let dir = private_tempdir();
    let root = dir.path().join("root");
    let store = Store::open(&root);
    let s: crate::session::SessionId = "s-1".parse().unwrap();
    let t0 = time::macros::datetime!(2026-11-01 10:00 UTC);
    store
        .update_control(|state| control::switch(state, &s, &Switch::On, t0))
        .unwrap();

    let mut record = crate::tests::session("s-1", t0);
    record.summary = String::from("the token is s3cr3t-value-123 here");
    let redactor =
        crate::store::Redactor::new([(String::from("TOKEN"), String::from("s3cr3t-value-123"))]);
    let written = store
        .settle_pass(&s, Some(record.clone()), false, &redactor)
        .unwrap()
        .unwrap();
    assert_eq!(written.summary, "the token is [redacted:TOKEN] here");
    assert!(root.join("sessions/2026/11/s-1.md").is_file());
    let view = store.session(&s).unwrap();
    assert_eq!(view.record, written);
    assert!(!view.control.unwrap().due);
    assert!(!root.join("entries").exists(), "a pass writes no entries");
}

#[test]
fn a_link_is_kept_on_the_switch_until_the_first_record_then_on_the_record() {
    use crate::control::{self, Switch};
    use crate::session::{LinkType, SessionId, SessionLink};
    let dir = private_tempdir();
    let root = dir.path().join("root");
    let store = Store::open(&root);
    let t0 = time::macros::datetime!(2026-11-01 10:00 UTC);
    let parent: SessionId = "p-1".parse().unwrap();
    let child: SessionId = "c-1".parse().unwrap();
    store
        .update_control(|state| control::switch(state, &parent, &Switch::On, t0))
        .unwrap();
    let redactor = crate::store::Redactor::new([]);
    store
        .settle_pass(
            &parent,
            Some(crate::tests::session("p-1", t0)),
            false,
            &redactor,
        )
        .unwrap();

    let spawned = SessionLink {
        kind: LinkType::SpawnedFrom,
        session: parent.clone(),
    };
    let linked = store.link_session(&child, &spawned, true, t0).unwrap();
    assert!(linked.added && linked.switched_on && !linked.write_record);
    assert_eq!(
        store.control().unwrap().get(&child).unwrap().pending_links,
        vec![spawned.clone()]
    );

    // The first record takes the pending link, and the switch drops it.
    let written = store
        .settle_pass(
            &child,
            Some(crate::tests::session("c-1", t0)),
            false,
            &redactor,
        )
        .unwrap()
        .unwrap();
    assert_eq!(written.links, vec![spawned.clone()]);
    assert!(
        store
            .control()
            .unwrap()
            .get(&child)
            .unwrap()
            .pending_links
            .is_empty()
    );
    let view = store.session(&parent).unwrap();
    assert_eq!(view.children.len(), 1);
    assert_eq!(view.children[0].session, child);
    assert!(
        view.record.links.is_empty(),
        "the parent is never rewritten"
    );

    // A link added while a pass runs on an older copy is not lost.
    let stale = store.session(&child).unwrap().record;
    let later = t0 + time::Duration::hours(1);
    let other: SessionId = "o-1".parse().unwrap();
    store
        .update_control(|state| control::switch(state, &other, &Switch::On, t0))
        .unwrap();
    let continues = SessionLink {
        kind: LinkType::Continues,
        session: other,
    };
    let linked = store
        .link_session(&child, &continues, false, later)
        .unwrap();
    assert!(linked.added && linked.write_record);
    let on_disk = store.session(&child).unwrap().record;
    assert_eq!(on_disk.links, vec![spawned.clone(), continues.clone()]);
    assert_eq!(on_disk.updated, stale.updated, "a link is not a pass");
    let written = store
        .settle_pass(&child, Some(stale), false, &redactor)
        .unwrap()
        .unwrap();
    assert_eq!(written.links, vec![spawned, continues]);

    // A refused link writes nothing.
    let before = fs::read(root.join("control.json")).unwrap();
    let cycle = SessionLink {
        kind: LinkType::Continues,
        session: child.clone(),
    };
    let err = store
        .link_session(&parent, &cycle, false, later)
        .unwrap_err();
    assert_eq!(err.code(), "link_cycle");
    assert_eq!(fs::read(root.join("control.json")).unwrap(), before);
}
