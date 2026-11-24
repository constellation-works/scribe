use super::{Seed, entry_text, private_tempdir, seed};
use crate::TagVerbs;
use crate::entry::{EntryFilter, EntryStatus};
use crate::session::SessionFilter;
use crate::store::db;
use crate::store::{AuditEvent, AuditStatus, Store};
use crate::testing::StoreTestInternals;
use crate::testing::ViewTestReads;
use crate::{EntryVerbs, ProposalVerbs};
use crate::{PassVerbs, RebuildVerbs, SessionVerbs};
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
        author: None,
        instruction_session: None,
        instruction_message: None,
        issue: None,
        duration_ms: 0,
    }
}

/// A session record as stored, in the file format, so a test can tell a
/// write changed nothing.
fn stored(store: &Store, id: &crate::session::SessionId) -> Vec<u8> {
    let record = store.session(id).unwrap().record;
    crate::store::codec::encode_session(Path::new("record"), &record).unwrap()
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
    assert_eq!(tags.tags.len(), 5, "five shipped tags");
    assert!(tags.tags.iter().all(|tag| tag.entries == 0));
    let err = store.entry(&"preferences-1".parse().unwrap()).unwrap_err();
    assert_eq!(err.code(), "entry_not_found");
    assert!(
        !dir.path().join("absent").exists(),
        "reads never create the root"
    );
}

#[test]
fn a_corrupt_entry_is_skipped_in_a_list_and_refused_by_show() {
    let dir = private_tempdir();
    let at = time::macros::datetime!(2026-11-01 10:00 UTC);
    seed(
        dir.path(),
        &Seed {
            entries: vec![
                crate::tests::entry("preferences-1", at),
                crate::tests::entry("preferences-2", at),
            ],
            ..Seed::default()
        },
    );
    // A hand-edited row: Daniel's entry without the turn it cites.
    db::execute(
        dir.path(),
        "UPDATE entries SET cite = NULL WHERE id = 'preferences-2'",
    )
    .unwrap();
    let store = Store::open(dir.path());
    let list = store.list_entries(&EntryFilter::default()).unwrap();
    assert_eq!(list.listing.items.len(), 1);
    assert_eq!(list.skipped.len(), 1);
    assert!(
        list.skipped[0]
            .path
            .to_string_lossy()
            .ends_with("preferences-2"),
        "{:?}",
        list.skipped[0].path
    );
    assert_eq!(list.skipped[0].error.code(), "store_corrupt");
    let err = store.entry(&"preferences-2".parse().unwrap()).unwrap_err();
    assert_eq!(err.code(), "store_corrupt");
    let shown = store.entry(&"preferences-1".parse().unwrap()).unwrap();
    assert_eq!(shown.status, EntryStatus::Active);
}

#[test]
fn a_session_is_shown_with_its_switch() {
    let dir = private_tempdir();
    let at = time::macros::datetime!(2026-11-01 09:00 UTC);
    let mut control = crate::session::ControlState::default();
    crate::control::switch(
        &mut control,
        &"s-1".parse().unwrap(),
        &crate::control::Switch::On,
        at,
    );
    control
        .sessions
        .get_mut(&"s-1".parse().unwrap())
        .unwrap()
        .due = true;
    seed(
        dir.path(),
        &Seed {
            sessions: vec![crate::tests::session("s-1", at)],
            control,
            ..Seed::default()
        },
    );
    let store = Store::open(dir.path());
    let view = store.session(&"s-1".parse().unwrap()).unwrap();
    assert_eq!(view.record.log[0].text, "outcome");
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
fn a_linked_entries_directory_is_refused_not_followed() {
    let dir = private_tempdir();
    put(dir.path(), "elsewhere/preferences-1.md", &entry_text(""));
    std::os::unix::fs::symlink(dir.path().join("elsewhere"), dir.path().join("entries")).unwrap();
    let err = Store::open(dir.path())
        .list_entries(&EntryFilter::default())
        .unwrap_err();
    assert_eq!(err.code(), "store_symlink", "got {err:?}");
}

#[test]
fn record_audit_keeps_one_line_per_event_and_creates_the_directory() {
    let dir = private_tempdir();
    let root = dir.path().join("data");
    let store = Store::open(&root);
    store
        .record_audit(&event("hook user-prompt-submit"))
        .unwrap();
    store
        .record_audit(&event("hook user-prompt-submit"))
        .unwrap();
    let (events, torn) = store.audit_events().unwrap();
    assert_eq!(events.len(), 2, "got {events:?}");
    assert_eq!(torn, 0);
}

/// Template defect 1: a locked write waited twice, once for the write and
/// again for its audit line, and the line was lost with a warning. A line
/// recorded while another writer holds `scribe.db` now waits for nothing:
/// it is spooled, every read sees it, and the next write drains it.
#[test]
fn record_audit_never_waits_for_a_writer_and_the_next_write_drains_the_spool() {
    let dir = private_tempdir();
    let store = Store::open(dir.path()).with_lock_wait(Duration::from_secs(5));
    let on = |store: &Store, id: &str| {
        store
            .update_control(|state| {
                crate::control::switch(
                    state,
                    &id.parse().unwrap(),
                    &crate::control::Switch::On,
                    time::macros::datetime!(2026-11-01 10:00 UTC),
                )
            })
            .unwrap();
    };
    on(&store, "s-1");
    store
        .record_audit(&event("hook user-prompt-submit"))
        .unwrap();
    let holder = db::open_new_writer(dir.path(), Duration::from_secs(1)).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    store.record_audit(&event("entry add")).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the line waited {:?}",
        started.elapsed()
    );
    let spool = dir.path().join(super::super::write::AUDIT_SPOOL_DIR);
    assert_eq!(fs::read_dir(&spool).unwrap().count(), 1);
    let (events, _) = store.audit_events().unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| e.command.as_str())
            .collect::<Vec<_>>(),
        ["hook user-prompt-submit", "entry add"]
    );
    holder.execute_batch("ROLLBACK").unwrap();
    drop(holder);
    store.record_audit(&event("entry update")).unwrap();
    on(&store, "s-2");
    assert_eq!(fs::read_dir(&spool).unwrap().count(), 0, "drained");
    let (events, _) = store.audit_events().unwrap();
    assert_eq!(events.len(), 3, "each line once: {events:?}");
}

mod cites {
    use crate::EntryVerbs;
    use crate::MarkVerbs;
    use crate::entry::{Cite, ContentFrom, EntryId, NewEntry};
    use crate::error::Error;
    use crate::store::Store;
    use crate::store::tests::private_tempdir;
    use crate::testing::StoreTestInternals;
    use crate::testing::ViewTestReads;
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
            window: None,
            quote: None,
            stated_by: None,
        }
    }

    /// Write `drafts` as entries, numbered in their tags.
    fn write(store: &Store, drafts: Vec<NewEntry>) {
        store.prepare_dir(store.root()).unwrap();
        let mut ids: Vec<EntryId> = Vec::new();
        for draft in drafts {
            let id = EntryId::next_in(&draft.tag, &ids);
            store
                .write_entry(&draft.into_entry(id.clone()), true)
                .unwrap();
            ids.push(id);
        }
    }

    #[test]
    fn an_unsafe_root_is_refused_and_nothing_is_written() {
        let dir = private_tempdir();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).unwrap();
            let err = Store::open(dir.path())
                .record_marks(
                    vec![(
                        crate::store::MarkKey {
                            session: "s-9".parse().unwrap(),
                            message: String::from("u-1"),
                            prompt_sha256: "a".repeat(64),
                            marker: 1,
                        },
                        crate::store::MarkChange::Refused(crate::store::AGENT_HANDOFF),
                    )],
                    datetime!(2026-11-02 19:06:00 UTC),
                )
                .unwrap_err();
            assert!(matches!(err, Error::Permissions { .. }), "{err:?}");
            assert!(!dir.path().join("entries").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_root_or_config_open_to_others_is_refused_on_load() {
        // Owner-only (STD-05 R9): the config's group and other read bits are
        // refused as well as its write bits, and so is a loose root.
        use std::os::unix::fs::PermissionsExt;
        let dir = private_tempdir();
        let config = dir.path().join("config.toml");
        fs::write(&config, "").unwrap();
        for loose in [0o644, 0o640] {
            fs::set_permissions(&config, fs::Permissions::from_mode(loose)).unwrap();
            let err = Store::open(dir.path()).config().unwrap_err();
            assert!(
                matches!(err, Error::Permissions { mode, fix: "600", .. } if mode == loose),
                "{loose:o}: {err:?}"
            );
        }
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Store::open(dir.path()).config().is_ok());

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let err = Store::open(dir.path()).config().unwrap_err();
        assert!(
            matches!(err, Error::Permissions { fix: "700", .. }),
            "{err:?}"
        );
    }

    #[test]
    fn cites_of_the_session_are_filled_once_and_others_are_left_alone() {
        let dir = private_tempdir();
        let store = Store::open(dir.path());
        let mut other = draft("preferences", 1, "other session");
        other.cite.session = "s-other".parse().unwrap();
        write(
            &store,
            vec![
                draft("preferences", 1, "one"),
                draft("preferences", 2, "two"),
                other,
            ],
        );
        let session = "s-9".parse().unwrap();
        assert!(store.has_unfilled_cites(&session).unwrap());
        let now = datetime!(2026-11-02 19:06:00 UTC);
        let filled = store
            .fill_cite_messages(&session, now, |cite| {
                (cite.line == 1).then(|| String::from("m-1"))
            })
            .unwrap();
        let filled: Vec<String> = filled.iter().map(ToString::to_string).collect();
        assert_eq!(filled, ["preferences-1"]);
        let one = store.entry(&"preferences-1".parse().unwrap()).unwrap();
        assert_eq!(one.cite.as_ref().unwrap().message.as_deref(), Some("m-1"));
        assert_eq!(one.updated, now);
        let two = store.entry(&"preferences-2".parse().unwrap()).unwrap();
        assert_eq!(
            two.cite.as_ref().unwrap().message,
            None,
            "no match leaves it waiting"
        );
        let three = store.entry(&"preferences-3".parse().unwrap()).unwrap();
        assert_eq!(
            three.cite.as_ref().unwrap().message,
            None,
            "another session is not touched"
        );
        // A filled cite is not filled again.
        let again = store
            .fill_cite_messages(&session, now, |_| Some(String::from("m-2")))
            .unwrap();
        let again: Vec<String> = again.iter().map(ToString::to_string).collect();
        assert_eq!(again, ["preferences-2"]);
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
        let mut other_prompt = draft("preferences", 1, "other prompt");
        other_prompt.cite.prompt_sha256 = "b".repeat(64);
        let mut other_session = draft("preferences", 1, "other session");
        other_session.cite.session = "s-other".parse().unwrap();
        let store = Store::open(dir.path());
        write(
            &store,
            vec![
                draft("preferences", 1, "one"),
                draft("rituals", 2, "two"),
                other_prompt,
                other_session,
            ],
        );
        let mut found: Vec<String> = store
            .entries_from_prompt(&session, &"a".repeat(64))
            .unwrap()
            .iter()
            .map(|e| e.id.to_string())
            .collect();
        found.sort();
        assert_eq!(found, ["preferences-1", "rituals-1"]);
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

    // Nothing changed: no switch is written.
    let off = store
        .update_control(|state| control::switch(state, &s, &Switch::Off, t0))
        .unwrap();
    assert!(!off.changed());
    assert!(store.control().unwrap().get(&s).is_none());

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
        .settle_pass(&s, Some(record.clone()), false, None, &redactor)
        .unwrap()
        .unwrap();
    assert_eq!(written.summary, "the token is [redacted:TOKEN] here");
    assert!(root.join("scribe.db").is_file());
    assert!(
        !root.join("sessions").exists(),
        "a record is a row, not a file"
    );
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
            None,
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
            None,
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
    let stale_updated = stale.updated;
    let written = store
        .settle_pass(&child, Some(stale), false, Some(stale_updated), &redactor)
        .unwrap()
        .unwrap();
    assert_eq!(written.links, vec![spawned, continues]);

    // A refused link writes nothing.
    let before = (store.control().unwrap(), stored(&store, &child));
    let cycle = SessionLink {
        kind: LinkType::Continues,
        session: child.clone(),
    };
    let err = store
        .link_session(&parent, &cycle, false, later)
        .unwrap_err();
    assert_eq!(err.code(), "link_cycle");
    assert_eq!((store.control().unwrap(), stored(&store, &child)), before);
}

#[test]
fn ordinary_writes_refuse_any_change_to_a_sealed_segment() {
    use crate::{Redactor, SegmentState};
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let t0 = time::macros::datetime!(2026-11-01 10:00 UTC);
    let mut record = crate::tests::session("s-1", t0);
    record.log[0].state = SegmentState::Sealed;
    let id = record.id.clone();
    let redactor = Redactor::new([]);
    let original = store
        .settle_pass(&id, Some(record), false, None, &redactor)
        .unwrap()
        .unwrap();
    let before = stored(&store, &id);
    let mut variants = Vec::new();
    let mut changed = original.clone();
    changed.log[0].text.push_str(" edited");
    variants.push(changed);
    let mut changed = original.clone();
    changed.log[0].heading.push_str(" edited");
    variants.push(changed);
    let mut changed = original.clone();
    changed.log[0].windows = std::num::NonZeroU32::new(2).unwrap();
    variants.push(changed);
    let mut changed = original.clone();
    changed.log[0].to += time::Duration::seconds(1);
    variants.push(changed);
    let mut changed = original.clone();
    changed.log.clear();
    variants.push(changed);
    for changed in variants {
        assert_eq!(
            store
                .checkpoint_pass(changed.clone(), Some(original.updated), &redactor)
                .unwrap_err()
                .code(),
            "sealed_segment_changed"
        );
        assert_eq!(
            store
                .settle_pass(&id, Some(changed), false, Some(original.updated), &redactor)
                .unwrap_err()
                .code(),
            "sealed_segment_changed"
        );
        assert_eq!(stored(&store, &id), before);
    }
    let mut changed = original.clone();
    changed.summary = String::from("Updated header");
    let redactor = Redactor::new([(String::from("LATER"), original.log[0].text.clone())]);
    let written = store
        .settle_pass(&id, Some(changed), false, Some(original.updated), &redactor)
        .unwrap()
        .unwrap();
    assert_eq!(
        written.log, original.log,
        "new redaction settings leave sealed history intact"
    );
}

#[test]
fn set_active_writes_only_the_activity_fields_and_only_on_a_change() {
    use crate::activity::{ActiveOutcome, ActiveRoute, SetActive};
    let dir = private_tempdir();
    let root = dir.path().join("root");
    let store = Store::open(&root);
    let s: crate::session::SessionId = "s-1".parse().unwrap();
    let t0 = time::macros::datetime!(2026-11-01 10:00 UTC);
    let none = crate::store::Redactor::new([]);
    store
        .settle_pass(
            &s,
            Some(crate::tests::session("s-1", t0)),
            false,
            None,
            &none,
        )
        .unwrap();
    let before = store.session(&s).unwrap().record;

    let redactor =
        crate::store::Redactor::new([(String::from("TOKEN"), String::from("s3cr3t-value-123"))]);
    let off = SetActive::Deactivate {
        by: crate::record::Author::agent("opus".parse().unwrap()),
        route: ActiveRoute::Manual,
        reason: String::from("removed; token s3cr3t-value-123"),
        cite: None,
    };
    let later = t0 + time::Duration::days(2);
    let (record, outcome) = store.set_active(&s, &off, later, &redactor).unwrap();
    assert_eq!(outcome, ActiveOutcome::Changed { unpinned: false });
    assert!(!record.active);
    assert_eq!(
        record.active_change.as_ref().unwrap().reason,
        "removed; token [redacted:TOKEN]"
    );
    let on_disk = store.session(&s).unwrap().record;
    assert_eq!(on_disk, record);
    let mut expected = before.clone();
    expected.active = false;
    expected.active_change.clone_from(&record.active_change);
    assert_eq!(on_disk, expected, "nothing else changes, updated included");

    let bytes = stored(&store, &s);
    let (_, again) = store.set_active(&s, &off, later, &redactor).unwrap();
    assert_eq!(again, ActiveOutcome::Unchanged);
    assert_eq!(stored(&store, &s), bytes, "a repeat writes nothing");

    let err = store
        .set_active(&"s-2".parse().unwrap(), &off, later, &redactor)
        .unwrap_err();
    assert_eq!(err.code(), "session_not_found");
}

#[test]
fn set_active_many_isolates_a_corrupt_record_and_continues() {
    use crate::activity::{ActiveOutcome, ActiveRoute, SetActive};
    use crate::record::Author;
    use crate::session::SessionId;

    let dir = private_tempdir();
    let root = dir.path().join("root");
    let store = Store::open(&root);
    let now = time::macros::datetime!(2026-11-10 10:00 UTC);
    let redactor = crate::store::Redactor::new([]);
    for id in ["s-1", "s-2", "s-3"] {
        let id: SessionId = id.parse().unwrap();
        store
            .settle_pass(
                &id,
                Some(crate::tests::session(id.as_str(), now)),
                false,
                None,
                &redactor,
            )
            .unwrap();
    }
    // A row that no longer decodes: its project is not a label.
    crate::store::db::execute(
        &root,
        "UPDATE sessions SET project = '' WHERE provider_session = 's-2'",
    )
    .unwrap();

    let change = SetActive::Deactivate {
        by: Author::consistency(),
        route: ActiveRoute::Age,
        reason: String::from("old"),
        cite: None,
    };
    let requests = [
        ("s-1".parse().unwrap(), change.clone()),
        ("s-2".parse().unwrap(), change.clone()),
        ("s-3".parse().unwrap(), change),
    ];
    let outcomes = store.set_active_many(&requests, now, &redactor).unwrap();

    assert_eq!(outcomes.len(), 3);
    assert_eq!(
        *outcomes[0].as_ref().unwrap(),
        ActiveOutcome::Changed { unpinned: false }
    );
    assert_eq!(outcomes[1].as_ref().unwrap_err().code(), "store_corrupt");
    assert_eq!(
        *outcomes[2].as_ref().unwrap(),
        ActiveOutcome::Changed { unpinned: false }
    );
    assert!(
        !store
            .session(&"s-1".parse().unwrap())
            .unwrap()
            .record
            .active
    );
    assert!(
        !store
            .session(&"s-3".parse().unwrap())
            .unwrap()
            .record
            .active
    );
}

#[test]
fn a_pass_write_refuses_when_the_record_changed_since_it_was_loaded() {
    use crate::control::{self, Switch};
    use crate::{Redactor, SegmentOrigin};
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let t0 = time::macros::datetime!(2026-11-01 10:00 UTC);
    let redactor = Redactor::new([]);
    let record = crate::tests::session("s-1", t0);
    let id = record.id.clone();
    store
        .update_control(|state| control::switch(state, &id, &Switch::On, t0))
        .unwrap();
    let original = store
        .settle_pass(&id, Some(record), false, None, &redactor)
        .unwrap()
        .unwrap();
    // The record a pass loaded, then a rebuild replaces it on disk.
    let stale = store.session(&id).unwrap().record;
    assert_eq!(stale, original);
    let mut rebuilt = stale.clone();
    rebuilt.summary = String::from("Rebuilt header");
    rebuilt.updated = t0 + time::Duration::hours(2);
    rebuilt.log[0].origin = SegmentOrigin::Rebuild;
    let rebuilt = store
        .finish_rebuild(rebuilt, stale.updated, &redactor)
        .unwrap()
        .unwrap();
    let rebuilt_bytes = stored(&store, &id);
    store
        .update_control(|state| control::fail(state, &id, t0))
        .unwrap();
    assert!(store.control().unwrap().get(&id).unwrap().due);

    let mut attempt = stale.clone();
    attempt.summary = String::from("Stale pass");
    attempt.updated = t0 + time::Duration::hours(3);
    assert_eq!(
        store
            .checkpoint_pass(attempt.clone(), Some(stale.updated), &redactor)
            .unwrap_err()
            .code(),
        "record_changed"
    );
    assert_eq!(
        store
            .settle_pass(&id, Some(attempt), false, Some(stale.updated), &redactor)
            .unwrap_err()
            .code(),
        "record_changed"
    );
    assert_eq!(stored(&store, &id), rebuilt_bytes);
    assert!(
        store.control().unwrap().get(&id).unwrap().due,
        "a refused settle keeps the session due"
    );

    // A pass that loaded the rebuilt record writes normally.
    let mut next = rebuilt.clone();
    next.summary = String::from("Next pass");
    next.updated = t0 + time::Duration::hours(4);
    let written = store
        .checkpoint_pass(next, Some(rebuilt.updated), &redactor)
        .unwrap()
        .unwrap();
    assert_eq!(written.summary, "Next pass");
}

#[test]
fn close_retired_candidates_respects_config_shipped_and_store_tags() {
    use crate::Redactor;
    use crate::entry::{Cite, Speaker};
    use crate::proposal::{ClosedReason, EntryCandidate, Proposal, ProposalKind, ProposalState};
    use crate::record::{Author, Label, Project};
    use crate::tag::StoreTag;
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let t0 = time::macros::datetime!(2026-11-15 10:00 UTC);
    let t1 = time::macros::datetime!(2026-11-21 06:00 UTC);
    let redactor = Redactor::new([]);

    // `custom` is configured in config.toml; `notes` is in tags.json;
    // `preferences` and `idea` are shipped; `decisions` is retired.
    put(
        dir.path(),
        "config.toml",
        "[tags.preferences]\naliases = [\"pref\"]\n[tags.custom]\ndescription = \"custom tag\"\n",
    );
    store
        .add_agent_tag(
            StoreTag {
                name: "notes".parse().unwrap(),
                aliases: Vec::new(),
                description: String::from("agent notes"),
                stale_after_days: None,
                author: Author::agent("opus".parse().unwrap()),
                updated_by: None,
                created: t0,
                updated: t0,
            },
            &redactor,
        )
        .unwrap();

    let mk = |n: u64, tag: &str| Proposal {
        id: format!("p-{n}").parse().unwrap(),
        kind: ProposalKind::Candidate,
        older: format!("{tag}-{n}").parse().unwrap(),
        newer: None,
        candidate: Some(EntryCandidate {
            tag: tag.parse().unwrap(),
            role: Speaker::User,
            quote: format!("I always want item {n}."),
            context: None,
            statement: None,
            subject: None,
            evidence: None,
            cite: Cite {
                session: "s-1".parse().unwrap(),
                message: Some(format!("m-{n}")),
                at: t0,
                prompt_sha256: "0".repeat(64),
                line: 1,
                cwd: String::from("/work"),
            },
            project: Project::sanitized("scribe"),
            provider: Label::sanitized("claude-code", "claude-code"),
            host: Label::sanitized("host", "host"),
            admitted: None,
        }),
        reason: format!("candidate {n}"),
        quotes: Vec::new(),
        state: ProposalState::Open,
        opened_at: t0,
        settled_at: None,
        settled_by: None,
        author: Author::session_pass(),
        updated_by: None,
        settled_author: None,
        closed_reason: None,
        instruction: None,
    };
    let proposals = vec![
        mk(1, "decisions"),
        mk(2, "preferences"),
        mk(3, "standard"),
        mk(4, "custom"),
        mk(5, "notes"),
    ];
    store.write_proposals(&proposals).unwrap();

    let closed = store.close_retired_candidates(t1).unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].id.to_string(), "p-1");
    assert_eq!(closed[0].state, ProposalState::Rejected);
    assert_eq!(closed[0].closed_reason, Some(ClosedReason::TagRetired));
    assert_eq!(
        closed[0].settled_author.as_ref().map(|a| a.to_string()),
        Some(String::from("scribe:tag-retired"))
    );

    let after = store.proposals().unwrap();
    assert_eq!(after[0].state, ProposalState::Rejected);
    for open in &after[1..] {
        assert_eq!(open.state, ProposalState::Open, "{}", open.id);
        assert_eq!(open.closed_reason, None);
    }
    assert!(store.close_retired_candidates(t1).unwrap().is_empty());
}

#[test]
fn corrupt_or_unreadable_config_and_tags_close_no_candidate() {
    use crate::Redactor;
    use crate::control::{self, Switch};
    use crate::entry::{Cite, Speaker};
    use crate::proposal::{EntryCandidate, Proposal, ProposalKind, ProposalState};
    use crate::record::{Author, Label, Project};
    let t0 = time::macros::datetime!(2026-11-15 10:00 UTC);
    let proposal = Proposal {
        id: "p-1".parse().unwrap(),
        kind: ProposalKind::Candidate,
        older: "decisions-1".parse().unwrap(),
        newer: None,
        candidate: Some(EntryCandidate {
            tag: "decisions".parse().unwrap(),
            role: Speaker::User,
            quote: String::from("I always want this kept."),
            context: None,
            statement: None,
            subject: None,
            evidence: None,
            cite: Cite {
                session: "s-1".parse().unwrap(),
                message: Some(String::from("m-1")),
                at: t0,
                prompt_sha256: "0".repeat(64),
                line: 1,
                cwd: String::from("/work"),
            },
            project: Project::sanitized("scribe"),
            provider: Label::sanitized("claude-code", "claude-code"),
            host: Label::sanitized("host", "host"),
            admitted: None,
        }),
        reason: String::from("candidate"),
        quotes: Vec::new(),
        state: ProposalState::Open,
        opened_at: t0,
        settled_at: None,
        settled_by: None,
        author: Author::session_pass(),
        updated_by: None,
        settled_author: None,
        closed_reason: None,
        instruction: None,
    };
    let cases: &[(&str, &str, &str)] = &[
        ("config.toml", "this is not toml\n", "config_invalid"),
        ("tags.json", "{", "store_corrupt"),
    ];
    for (rel, text, code) in cases {
        let dir = private_tempdir();
        let store = Store::open(dir.path());
        store
            .update_control(|state| {
                control::switch(state, &"s-1".parse().unwrap(), &Switch::On, t0)
            })
            .unwrap();
        store
            .write_proposals(std::slice::from_ref(&proposal))
            .unwrap();
        put(dir.path(), rel, text);
        let err = store.close_retired_candidates(t0).unwrap_err();
        assert_eq!(err.code(), *code, "{rel}: {err}");
        assert_eq!(store.proposals().unwrap()[0].state, ProposalState::Open);
        let record = crate::tests::session("s-1", t0);
        let err = store
            .open_pass_candidates(&record, &[], &Redactor::new([]))
            .unwrap_err();
        assert_eq!(err.code(), *code, "pass {rel}: {err}");
        assert_eq!(store.proposals().unwrap()[0].state, ProposalState::Open);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for rel in ["config.toml", "tags.json"] {
            let dir = private_tempdir();
            let store = Store::open(dir.path());
            store
                .update_control(|state| {
                    control::switch(state, &"s-1".parse().unwrap(), &Switch::On, t0)
                })
                .unwrap();
            store
                .write_proposals(std::slice::from_ref(&proposal))
                .unwrap();
            let body = if rel == "config.toml" {
                "[tags.preferences]\n"
            } else {
                "{\"format\":11,\"tags\":[]}\n"
            };
            put(dir.path(), rel, body);
            let path = dir.path().join(rel);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
            let scan_err = store.close_retired_candidates(t0).unwrap_err();
            let record = crate::tests::session("s-1", t0);
            let pass_err = store
                .open_pass_candidates(&record, &[], &Redactor::new([]))
                .unwrap_err();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(scan_err.code(), "io_error", "{rel}: {scan_err}");
            assert_eq!(pass_err.code(), "io_error", "pass {rel}: {pass_err}");
            assert_eq!(store.proposals().unwrap()[0].state, ProposalState::Open);
        }
    }
}

#[test]
fn typed_forget_clears_instruction_quotes_on_disk_and_keeps_metadata() {
    use crate::consistency::{Settle, Settled};
    use crate::entry::{Cite, Instruction};
    use crate::proposal::{Proposal, ProposalKind, ProposalState};
    use crate::record::Author;
    use time::macros::datetime;

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let t0 = datetime!(2026-11-01 10:00 UTC);
    let t1 = datetime!(2026-11-02 10:00 UTC);
    let sha = "0".repeat(64);
    let secret_quote = "approve p-1 with secret phrase XYZ123";

    let inst = Instruction {
        session: "s-9".parse().unwrap(),
        message: String::from("m-9"),
        at: datetime!(2026-11-02 09:00 UTC),
        quote: Some(String::from(secret_quote)),
    };
    // Daniel's typed approval of a pass claim, settled on an instruction.
    let mut entry = crate::tests::entry("preferences-1", t0);
    entry.said = String::from("#scribe approve p-1");
    entry.content = String::from("I prefer secret phrase XYZ123");
    entry.content_from = crate::ContentFrom::Message {
        role: crate::Speaker::User,
        message: String::from("m-1"),
        at: t0,
    };
    entry.cite = Some(Cite {
        session: "s-1".parse().unwrap(),
        message: Some(String::from("m-1")),
        at: t0,
        prompt_sha256: sha.clone(),
        line: 1,
        cwd: String::from("/work"),
    });
    entry.approved_by = Some(Author::agent("aide".parse().unwrap()));
    entry.instruction = Some(inst.clone());
    seed(
        dir.path(),
        &Seed {
            entries: vec![entry],
            ..Seed::default()
        },
    );
    let inst_cite = Cite {
        session: "s-9".parse().unwrap(),
        message: Some(String::from("m-9")),
        at: datetime!(2026-11-02 09:00 UTC),
        prompt_sha256: sha.clone(),
        line: 1,
        cwd: String::from("/work"),
    };
    let proposal = Proposal {
        id: "p-1".parse().unwrap(),
        kind: ProposalKind::Candidate,
        older: "preferences-1".parse().unwrap(),
        newer: None,
        candidate: Some(crate::EntryCandidate {
            tag: "preferences".parse().unwrap(),
            role: crate::Speaker::User,
            quote: String::from("I prefer secret phrase XYZ123"),
            context: None,
            statement: None,
            subject: None,
            evidence: None,
            cite: Cite {
                session: "s-1".parse().unwrap(),
                message: Some(String::from("m-1")),
                at: t0,
                prompt_sha256: sha,
                line: 1,
                cwd: String::from("/work"),
            },
            project: "scribe".parse().unwrap(),
            provider: crate::Label::sanitized("claude-code", "claude-code"),
            host: crate::Label::sanitized("host", "host"),
            admitted: None,
        }),
        reason: String::from("Unmarked preferences statement: secret phrase XYZ123"),
        quotes: Vec::new(),
        state: ProposalState::Approved,
        opened_at: t0,
        settled_at: Some(datetime!(2026-11-02 09:00 UTC)),
        settled_by: Some(inst_cite.clone()),
        author: Author::session_pass(),
        updated_by: None,
        settled_author: Some(Author::agent("aide".parse().unwrap())),
        closed_reason: None,
        instruction: Some(inst),
    };
    store.write_proposals(&[proposal]).unwrap();

    let settled = store
        .settle(
            &Settle::Forget("preferences-1".parse().unwrap()),
            &inst_cite,
            t1,
        )
        .unwrap();
    assert_eq!(
        settled,
        Settled::Forgotten("preferences-1".parse().unwrap())
    );

    let entry = store.entry(&"preferences-1".parse().unwrap()).unwrap();
    assert_eq!(entry.status, EntryStatus::Forgotten);
    let e_inst = entry.instruction.as_ref().unwrap();
    assert_eq!(e_inst.session.as_str(), "s-9");
    assert_eq!(e_inst.message, "m-9");
    assert_eq!(e_inst.quote, None);

    let proposals = store.proposals().unwrap();
    let p_inst = proposals[0].instruction.as_ref().unwrap();
    assert_eq!(p_inst.session.as_str(), "s-9");
    assert_eq!(p_inst.message, "m-9");
    assert_eq!(p_inst.quote, None);

    // Neither the rows nor a WAL frame or free page keeps the quote.
    assert!(!super::on_disk(dir.path(), "XYZ123"));
}

mod marks {
    use crate::EntryVerbs;
    use crate::store::Store;
    use crate::store::tests::private_tempdir;

    #[test]
    fn store_agent_writes_enforce_single_immutable_handoff_session_ref() {
        let dir = private_tempdir();
        let store = Store::open(dir.path().join("root"));
        let redactor = crate::Redactor::new(std::iter::empty());
        let author: crate::AuthorName = "opus".parse().unwrap();
        let now = time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(100);
        let valid_body = "\
## State
Done.

## In flight
None.

## Next
Ship.

## Waiting on you
None.

## Do not touch
None.

## Pointers
None.
";
        let draft = |related: &[&str]| crate::AgentEntry {
            tag: "handoff".parse().unwrap(),
            content: valid_body.to_owned(),
            project: "scribe".parse().unwrap(),
            topics: crate::Topics::default(),
            related: related.iter().map(|s| s.parse().unwrap()).collect(),
            host: crate::Label::sanitized("test", "test"),
            quote: Some(String::from("Please write a handoff for this session.")),
            evidence: None,
            as_of: None,
            spoken: false,
            handoff_mode: None,
            decision: None,
            item: None,
            self_refresh_verified: false,
        };

        // Direct API call refuses multiple sessions and invalid session refs.
        let err = store
            .add_agent_entry(
                draft(&["session:s-1", "session:s-2"]),
                author.clone(),
                now,
                &redactor,
            )
            .unwrap_err();
        assert_eq!(err.code(), "handoff_session");
        let err = store
            .add_agent_entry(draft(&["session:../bad"]), author.clone(), now, &redactor)
            .unwrap_err();
        assert_eq!(err.code(), "handoff_session");

        // Single valid session succeeds.
        let added = store
            .add_agent_entry(
                draft(&["session:s-1", "task:ORB-1"]),
                author.clone(),
                now,
                &redactor,
            )
            .unwrap();
        let id = added.id.to_string();

        // Direct API update refuses removing, adding, or replacing a session ref.
        for patch in [
            crate::EntryPatch {
                remove_related: vec!["session:s-1".parse().unwrap()],
                ..Default::default()
            },
            crate::EntryPatch {
                add_related: vec!["session:s-2".parse().unwrap()],
                ..Default::default()
            },
            crate::EntryPatch {
                remove_related: vec!["session:s-1".parse().unwrap()],
                add_related: vec!["session:s-2".parse().unwrap()],
                ..Default::default()
            },
        ] {
            let err = store.check_agent_entry_update(&id, &patch).unwrap_err();
            assert_eq!(err.code(), "handoff_session");
            let err = store
                .update_agent_entry(&id, patch, author.clone(), now, &redactor)
                .unwrap_err();
            assert_eq!(err.code(), "handoff_session");
        }

        // Non-session related edits still succeed and preserve session:s-1.
        let ok_patch = crate::EntryPatch {
            remove_related: vec!["task:ORB-1".parse().unwrap()],
            add_related: vec!["repo:scribe".parse().unwrap()],
            ..Default::default()
        };
        let (updated, changed) = store
            .update_agent_entry(&id, ok_patch, author, now, &redactor)
            .unwrap();
        assert!(changed);
        assert_eq!(
            updated.related,
            vec![
                "session:s-1".parse().unwrap(),
                "repo:scribe".parse().unwrap()
            ]
        );
    }
}

/// A trial `open-item` entry (no `item` object) as the 2026-11-23 trial
/// wrote it, by an agent.
fn trial_item(id: &str, retired: bool) -> crate::Entry {
    let mut trial = crate::tests::entry(id, crate::tests::T0);
    trial.content = String::from("Restart the dashboard");
    trial.author = crate::Author::agent("0-chief".parse().unwrap());
    trial.content_from = crate::ContentFrom::Agent;
    trial.cite = None;
    trial.said.clear();
    if retired {
        trial.status = EntryStatus::Retired;
        trial.inactive_since = Some(trial.created);
        trial.retirement_reason = Some(String::from("closed: carried out in chat"));
    }
    trial
}

#[test]
fn trial_open_items_derive_their_state_and_take_answers_and_closes() {
    use crate::{ItemAnswer, ItemState, Redactor};

    let dir = private_tempdir();
    let store = Store::open(dir.path());
    store.prepare_dir(store.root()).unwrap();
    for (id, retired) in [
        ("open-item-1", false),
        ("open-item-2", true),
        ("open-item-3", false),
    ] {
        store.write_entry(&trial_item(id, retired), true).unwrap();
    }
    let state = |id: &str| ItemState::of(&store.resolve_entry(id).unwrap());
    assert_eq!(state("open-item-1"), Some(ItemState::Open));
    assert_eq!(state("open-item-2"), Some(ItemState::Closed));
    let by_state = |states: Vec<ItemState>| -> Vec<String> {
        store
            .list_entries(&EntryFilter {
                states,
                ..EntryFilter::default()
            })
            .unwrap()
            .listing
            .items
            .iter()
            .map(|e| e.id.to_string())
            .collect()
    };
    assert_eq!(by_state(vec![ItemState::Closed]), ["open-item-2"]);

    let now = crate::tests::T0 + time::Duration::hours(1);
    let redactor = Redactor::default();
    let answer = ItemAnswer {
        words: String::from("done"),
        session: "8dc9df2b-5808-47cf-8be4-190c9c6e0711".parse().unwrap(),
        at: now,
        recorded_by: "0-aide".parse().unwrap(),
    };
    let (answered, changed) = store
        .answer_open_item("open-item-1", answer.clone(), now, &redactor)
        .unwrap();
    assert!(changed);
    assert_eq!(ItemState::of(&answered), Some(ItemState::Answered));
    let item = answered.item.as_ref().unwrap();
    assert_eq!(item.asked_by.as_str(), "0-chief");
    assert!(item.options.is_empty(), "a trial item reads as an action");
    assert_eq!(item.answer.as_ref(), Some(&answer));
    assert_eq!(answered.content, "Restart the dashboard");

    // A trial item has no options, so it closes with or without an answer.
    for id in ["open-item-1", "open-item-3"] {
        let (closed, _) = store
            .close_open_item(
                id,
                None,
                Some("Restarted"),
                "0-chief".parse().unwrap(),
                now,
                &redactor,
            )
            .unwrap();
        assert_eq!(ItemState::of(&closed), Some(ItemState::Closed));
        assert_eq!(closed.item.unwrap().closed.unwrap().note, "Restarted");
    }
    let code = |err: crate::Error| {
        let crate::Error::AgentWrite { kind, .. } = err else {
            panic!("{err:?}");
        };
        kind.code()
    };
    let refused = store
        .close_open_item(
            "open-item-2",
            None,
            Some("Again"),
            "0-chief".parse().unwrap(),
            now,
            &redactor,
        )
        .unwrap_err();
    assert_eq!(code(refused), "item_settled");
    // Words that redaction would alter are refused before anything is written.
    let secret = Redactor::new([(String::from("KEY"), String::from("hunter2-secret-value"))]);
    let refused = store
        .answer_open_item(
            "open-item-1",
            ItemAnswer {
                words: String::from("use hunter2-secret-value"),
                ..answer
            },
            now,
            &secret,
        )
        .unwrap_err();
    assert_eq!(code(refused), "needs_redaction");
}
