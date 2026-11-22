use super::private_tempdir;
use crate::error::Error;
use crate::memory::legacy::{MemoryItem, MemoryStatus, Section};
use crate::memory::{Checked, Draft, ElementKind, NowKind, profile};
use crate::models::{ProfileAnswer, ProfileMilestoneAnswer};
use crate::session::{SessionId, SessionRecord};
use crate::store::memory::CRASH_AFTER_COMMIT;
use crate::store::{PassMemory, Redactor, Store, db, format};
use crate::tests::{T0, session};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use time::macros::date;
use time::{Date, Duration, OffsetDateTime};

const TURN: &str = "My sister's wedding is on december 12 and I'm still on the seating.";

fn id(n: u32) -> SessionId {
    format!("0b8ef4a6-0c6a-4d77-9c41-{n:012}").parse().unwrap()
}

fn record(n: u32, at: OffsetDateTime) -> SessionRecord {
    let mut r = session(id(n).as_str(), at);
    r.updated = at;
    r
}

fn draft(title: &str, date: Option<Date>) -> Draft {
    Draft {
        checked: Checked {
            kind: ElementKind::Now,
            title: title.to_owned(),
            text: format!("{title}."),
            quote: TURN.to_owned(),
            message: String::from("u-1"),
            date: date.map(crate::record::nullable_date::format),
            now_kind: NowKind::Event,
            update: None,
            completes: None,
        },
        date,
        turn: TURN.to_owned(),
        at: T0,
    }
}

fn pass(drafts: Vec<Draft>, final_pass: bool) -> PassMemory {
    PassMemory {
        drafts,
        signal: None,
        final_pass,
    }
}

/// Every file under `root` with its bytes, so a test can tell a read wrote
/// nothing. `scribe.db-shm` is left out: it is SQLite's shared-memory
/// index, whose read marks every WAL reader sets, not store content.
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if !path.ends_with("scribe.db-shm") {
                out.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out
}

#[test]
fn an_undated_candidate_is_pending_and_the_final_pass_drops_it() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let r = record(1, T0);
    let written = store
        .write_pass_memory(
            &pass(vec![draft("Book the venue", None)], false),
            &redactor,
            &r,
        )
        .unwrap();
    assert_eq!(written.pending, 1);
    assert!(written.written.is_empty(), "no now file without a date");
    assert_eq!(db::pending_read(store.root(), &r.id).unwrap().len(), 1);
    // The session ends: its candidate never got a date, so it is dropped.
    store
        .write_pass_memory(&pass(Vec::new(), true), &redactor, &r)
        .unwrap();
    assert!(db::pending_read(store.root(), &r.id).unwrap().is_empty());
    // An undated candidate in the final pass itself is never kept.
    let written = store
        .write_pass_memory(
            &pass(vec![draft("Pick a caterer", None)], true),
            &redactor,
            &r,
        )
        .unwrap();
    assert_eq!(written.pending, 0);
    assert!(db::pending_read(store.root(), &r.id).unwrap().is_empty());
    assert!(store.memory_view().unwrap().now.is_empty());
}

#[test]
fn a_dated_now_element_writes_its_file_once_and_a_replay_writes_nothing() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let r = record(1, T0);
    let window = pass(
        vec![draft("Sister's wedding", Some(date!(2026 - 12 - 12)))],
        false,
    );
    let written = store.write_pass_memory(&window, &redactor, &r).unwrap();
    assert_eq!(written.written, ["memory:now/2026-12-12-sister-s-wedding"]);
    let before = tree(store.root());
    let again = store.write_pass_memory(&window, &redactor, &r).unwrap();
    assert!(again.written.is_empty());
    let path = store
        .root()
        .join("memory/now/2026-12-12-sister-s-wedding.md");
    assert_eq!(fs::read(&path).unwrap(), before[&path]);
}

#[test]
fn a_crash_between_the_journal_commit_and_the_files_is_replayed_by_the_next_write() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let r = record(1, T0);
    let window = pass(
        vec![draft("Sister's wedding", Some(date!(2026 - 12 - 12)))],
        false,
    );
    CRASH_AFTER_COMMIT.with(|c| c.set(true));
    let err = store.write_pass_memory(&window, &redactor, &r).unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err:?}");
    let file = store
        .root()
        .join("memory/now/2026-12-12-sister-s-wedding.md");
    assert!(!file.exists(), "the crash came before any file");
    // A read does not replay; it says a write is pending.
    let view = store.memory_view().unwrap();
    assert!(view.pending);
    assert!(view.now.is_empty());
    // Any next write replays the journal first.
    store
        .write_pass_memory(&pass(Vec::new(), true), &redactor, &r)
        .unwrap();
    assert!(file.exists());
    let view = store.memory_view().unwrap();
    assert!(!view.pending);
    assert_eq!(view.now.len(), 1);
}

#[test]
fn memory_reads_write_nothing() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let r = record(1, T0);
    let window = pass(
        vec![draft("Sister's wedding", Some(date!(2026 - 12 - 12)))],
        false,
    );
    store.write_pass_memory(&window, &redactor, &r).unwrap();
    let before = tree(store.root());
    store.memory_view().unwrap();
    store.memory_file("2026-12-12-sister-s-wedding").unwrap();
    store.memory_context(&r.id).unwrap();
    assert_eq!(tree(store.root()), before);
}

/// Write session files for `count` sessions, one a day from `from`.
fn sessions(store: &Store, count: u32, from: OffsetDateTime) {
    for n in 1..=count {
        let r = record(n, from + Duration::days(i64::from(n)));
        let path = store.session_path(&r);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut dir = path.parent().unwrap();
            while dir != store.root() {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
                dir = dir.parent().unwrap();
            }
        }
        fs::write(&path, format::encode_session(&path, &r).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}

fn with_section(text: &str, heading: &str, body: &str) -> String {
    text.replacen(
        &format!("## {heading}\n"),
        &format!("## {heading}\n{body}\n"),
        1,
    )
}

fn answer(profile: String) -> ProfileAnswer {
    ProfileAnswer {
        profile,
        milestones: Vec::new(),
    }
}

#[test]
fn a_refresh_resumes_from_its_cursor_keeps_pinned_and_is_idempotent() {
    let dir = private_tempdir();
    let root = dir.path().join("store");
    fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::open(root);
    let redactor = Redactor::default();
    sessions(&store, 45, T0);
    let now = T0 + Duration::days(60);

    // The first run reads 40 sessions and leaves 5 for the next.
    let first = store.profile_inputs(&redactor, now).unwrap();
    assert_eq!(first.sessions.len(), 40);
    assert_eq!(first.backlog, 5);
    assert_eq!(first.profile, profile::template());
    let built = with_section(&profile::template(), "Projects", "- scribe.");
    let written = store
        .write_refresh(&first, &answer(built.clone()), &redactor, now)
        .unwrap();
    assert_eq!(written.revision, Some(1));
    let file = store.root().join("memory/profile.md");
    assert_eq!(fs::read_to_string(&file).unwrap(), built);

    // The user pins a line by hand; the next run sees the edit.
    let pinned = with_section(&built, "Pinned", "- Call me Daniel.");
    fs::write(&file, &pinned).unwrap();
    let second = store.profile_inputs(&redactor, now).unwrap();
    assert_eq!(second.sessions.len(), 5, "resumes after the cursor");
    assert_eq!(second.backlog, 0);
    assert_eq!(second.pinned.trim(), "- Call me Daniel.");
    assert!(
        second
            .corrections
            .as_deref()
            .unwrap()
            .contains("+ - Call me Daniel.")
    );

    // A rebuild that drops Pinned is refused and writes nothing.
    let err = store
        .write_refresh(&second, &answer(built.clone()), &redactor, now)
        .unwrap_err();
    assert_eq!(err.code(), "profile_pinned");
    assert_eq!(fs::read_to_string(&file).unwrap(), pinned);
    assert_eq!(
        store.profile_inputs(&redactor, now).unwrap().sessions.len(),
        5
    );

    let kept = with_section(&pinned, "Projects", "- orbit.");
    let written = store
        .write_refresh(&second, &answer(kept.clone()), &redactor, now)
        .unwrap();
    assert_eq!(written.revision, Some(2));
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains("- Call me Daniel.")
    );

    // Nothing new now; the same answer again writes no revision.
    let third = store.profile_inputs(&redactor, now).unwrap();
    assert!(third.nothing_new());
    let again = store
        .write_refresh(&third, &answer(kept.clone()), &redactor, now)
        .unwrap();
    assert_eq!(again.revision, None);
    assert_eq!(store.profile_revision(2).unwrap().content, kept);
    // Restoring revision 1 writes it back as revision 3.
    assert_eq!(store.restore_profile(1, &redactor, now).unwrap(), Some(3));
    assert_eq!(fs::read_to_string(&file).unwrap(), built);
}

#[test]
fn a_refresh_refuses_when_memory_changed_since_its_inputs() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let inputs = store.profile_inputs(&redactor, T0).unwrap();
    let r = record(1, T0);
    store
        .write_pass_memory(
            &pass(
                vec![draft("Sister's wedding", Some(date!(2026 - 12 - 12)))],
                false,
            ),
            &redactor,
            &r,
        )
        .unwrap();
    let err = store
        .write_refresh(&inputs, &answer(profile::template()), &redactor, T0)
        .unwrap_err();
    assert_eq!(err.code(), "memory_changed");
}

#[test]
fn a_passed_now_item_becomes_a_milestone_or_history_after_fourteen_days() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let r = record(1, T0);
    let window = pass(
        vec![
            draft("Release ships", Some(date!(2026 - 11 - 05))),
            draft("Dentist", Some(date!(2026 - 11 - 06))),
            draft("Sister's wedding", Some(date!(2026 - 12 - 12))),
        ],
        false,
    );
    store.write_pass_memory(&window, &redactor, &r).unwrap();
    // On 11-19 nothing has passed its 14 days.
    let early = store
        .profile_inputs(&redactor, T0 + Duration::days(18))
        .unwrap();
    assert!(early.passed.is_empty());
    let now = T0 + Duration::days(25);
    let inputs = store.profile_inputs(&redactor, now).unwrap();
    let passed: Vec<&str> = inputs.passed.iter().map(|n| n.stem.as_str()).collect();
    assert_eq!(passed, ["2026-11-05-release-ships", "2026-11-06-dentist"]);
    let mut promoted = answer(profile::template());
    promoted.milestones.push(ProfileMilestoneAnswer {
        now: Some(String::from("2026-11-05-release-ships")),
        date: String::from("2026-11-05"),
        title: String::from("Release shipped"),
        text: String::from("The release shipped."),
    });
    let written = store
        .write_refresh(&inputs, &promoted, &redactor, now)
        .unwrap();
    assert_eq!(written.milestones, ["2026-11-05-release-shipped"]);
    assert_eq!(
        written.removed,
        ["2026-11-05-release-ships", "2026-11-06-dentist"]
    );
    let view = store.memory_view().unwrap();
    let now_left: Vec<&str> = view.now.iter().map(|n| n.stem.as_str()).collect();
    assert_eq!(now_left, ["2026-12-12-sister-s-wedding"]);
    assert_eq!(view.milestones.len(), 1);
    assert_eq!(view.milestones[0].sources, std::slice::from_ref(&r.id));
}

const LEGACY: &str = "---\nformat: 24\nkind: memory\nid: m-1\nsection: now\nstatus: active\n\
     statement: Said the release ships on 2026-12-01.\nproject: null\n\
     event_date: 2026-12-01\nsensitive: false\nsensitive_kind: null\n\
     acknowledged: null\nretirement_reason: null\n\
     promoted_from: null\npromotion: null\nmerged_into: null\nsuperseded_by: null\nunrelated_to: []\npreference_proposal: null\n\
     author: {kind: scribe, name: session-pass}\n\
     updated_by: {kind: scribe, name: session-pass}\n\
     created: 2026-11-01T05:40:00Z\nupdated: 2026-11-01T05:40:00Z\n\
     occurrences: [{n: 1, date: 2026-11-01T05:40:00Z, session: 0b8ef4a6-0c6a-4d77-9c41-000000000001, message: u-1, host: mac, project: scribe, quote: \"the release ships on december 1\", author: {kind: scribe, name: session-pass}}]\n---\n";

fn legacy(
    n: u32,
    section: Section,
    status: MemoryStatus,
    statement: &str,
    event: Option<Date>,
) -> MemoryItem {
    let path = Path::new("memory/m-1.md");
    let mut item = format::decode_memory(path, LEGACY.as_bytes(), "m-1").unwrap();
    item.id = format!("m-{n}").parse().unwrap();
    item.section = section;
    item.status = status;
    item.statement = statement.to_owned();
    item.event_date = event;
    item
}

#[test]
fn format_twenty_four_items_migrate_on_the_first_write() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let items = [
        legacy(
            1,
            Section::Now,
            MemoryStatus::Active,
            "Release ships",
            Some(date!(2026 - 12 - 01)),
        ),
        legacy(
            2,
            Section::Milestones,
            MemoryStatus::Active,
            "Married",
            Some(date!(2026 - 06 - 14)),
        ),
        legacy(
            3,
            Section::Profile,
            MemoryStatus::Active,
            "Works on scribe and orbit",
            None,
        ),
        legacy(
            4,
            Section::Now,
            MemoryStatus::Active,
            "Dentist",
            Some(date!(2026 - 10 - 01)),
        ),
        legacy(5, Section::Now, MemoryStatus::Forgotten, "", None),
        legacy(
            6,
            Section::Working,
            MemoryStatus::Active,
            "Prefers short replies",
            None,
        ),
    ];
    let mem = store.root().join("memory");
    fs::create_dir_all(&mem).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(store.root(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&mem, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for item in &items {
        let path = mem.join(format!("{}.md", item.id));
        fs::write(&path, format::encode_memory(&path, item).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    // A read leaves them alone and counts them.
    let before = tree(store.root());
    assert_eq!(store.memory_view().unwrap().unmigrated, 6);
    assert_eq!(tree(store.root()), before);

    let inputs = store.profile_inputs(&redactor, T0).unwrap();
    let view = store.memory_view().unwrap();
    assert_eq!(view.unmigrated, 0);
    for n in 1..=6 {
        assert!(!mem.join(format!("m-{n}.md")).exists(), "m-{n} removed");
    }
    let now: Vec<&str> = view.now.iter().map(|n| n.stem.as_str()).collect();
    assert_eq!(
        now,
        ["2026-12-01-release-ships"],
        "the expired one is history"
    );
    let milestones: Vec<&str> = view.milestones.iter().map(|m| m.stem.as_str()).collect();
    assert_eq!(milestones, ["2026-06-14-married"]);
    // The profile item is a hint for the first rebuild.
    assert_eq!(inputs.hints, ["profile (m-3): Works on scribe and orbit"]);
}
