use super::{Seed, private_tempdir, seed};
use crate::MemoryVerbs;
use crate::ProposalVerbs;
use crate::Provider;
use crate::apps::memory::{
    ApplyRefreshOutcome, ApplyRefreshRequest, Command as MemoryCommand, Error as MemoryError,
    Outcome as MemoryOutcome, PreparedRefresh, WindowMemory as PassMemory, WindowMemoryTestWrites,
};
use crate::composition::Context;
use crate::error::Error;
use crate::memory::{Checked, Draft, ElementKind, NowKind, profile};
use crate::models::{ProfileAnswer, ProfileMilestoneAnswer};
use crate::runtime::Clock;
use crate::session::{SessionId, SessionRecord};
use crate::store::memory::CRASH_AFTER_COMMIT;
use crate::store::{Redactor, Store, db};
use crate::testing::StoreTestInternals;
use crate::tests::{T0, session};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use time::macros::date;
use time::{Date, Duration, OffsetDateTime};

const TURN: &str = "My sister's wedding is on december 12 and I'm still on the seating.";

const LEGACY: &str = "---\nformat: 24\nkind: memory\nid: m-1\nsection: now\nstatus: active\n\
     statement: Said the release ships on 2026-12-01.\n\
     occurrences: [{n: 1, session: 0b8ef4a6-0c6a-4d77-9c41-000000000001, quote: \"the release ships on december 1\"}]\n---\n";

struct FixedClock(OffsetDateTime);

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
    fn now_precise(&self) -> OffsetDateTime {
        self.0
    }
}

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

/// Seed `count` session records, one a day from `from`.
fn sessions(store: &Store, count: u32, from: OffsetDateTime) {
    let records = (1..=count)
        .map(|n| record(n, from + Duration::days(i64::from(n))))
        .collect();
    seed(
        store.root(),
        &Seed {
            sessions: records,
            ..Seed::default()
        },
    );
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

fn prepare_at(store: &Store, now: OffsetDateTime) -> PreparedRefresh {
    let host = Context::at(store.root().to_path_buf(), None);
    let mut ctx = host.core();
    let clock = FixedClock(now);
    ctx.clock = &clock;
    let MemoryOutcome::Prepared(prepared) =
        crate::apps::memory::run(&ctx, MemoryCommand::Prepare).unwrap()
    else {
        panic!("expected prepared");
    };
    *prepared
}

fn apply_at(
    store: &Store,
    prepared: &PreparedRefresh,
    answer: ProfileAnswer,
    allow_retry: bool,
    now: OffsetDateTime,
) -> std::result::Result<ApplyRefreshOutcome, MemoryError<crate::runtime::errors::CommandError>> {
    apply_request(store, request_for(prepared, answer, allow_retry), now)
}

/// The apply request a job builds from `prepared`, unchanged.
fn request_for(
    prepared: &PreparedRefresh,
    answer: ProfileAnswer,
    allow_retry: bool,
) -> ApplyRefreshRequest {
    ApplyRefreshRequest {
        basis: prepared.basis.clone().expect("basis present"),
        cursor_to: prepared.cursor_to.clone(),
        session_ids: prepared.session_ids.clone(),
        backlog: prepared.backlog,
        passed_stems: prepared.passed_stems.clone(),
        answer,
        allow_retry,
    }
}

fn apply_request(
    store: &Store,
    request: ApplyRefreshRequest,
    now: OffsetDateTime,
) -> std::result::Result<ApplyRefreshOutcome, MemoryError<crate::runtime::errors::CommandError>> {
    let host = Context::at(store.root().to_path_buf(), None);
    let mut ctx = host.core();
    let clock = FixedClock(now);
    ctx.clock = &clock;
    let MemoryOutcome::Applied(outcome) = crate::apps::memory::run(
        &ctx,
        MemoryCommand::Apply {
            provider: Provider::ClaudeCode,
            request: Box::new(request),
            routine: false,
        },
    )?
    else {
        panic!("expected applied");
    };
    Ok(*outcome)
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
    let config_path = root.join("config.toml");
    fs::write(
        &config_path,
        "[user]\nname = \"Daniel\"\n\n[instructions]\nprofile = \"Keep {user}'s profile concise.\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let store = Store::open(root);
    let redactor = Redactor::default();
    sessions(&store, 45, T0);
    let now = T0 + Duration::days(60);

    // The first run reads 40 sessions, carries [instructions], and leaves 5 for the next.
    let first = prepare_at(&store, now);
    assert!(!first.nothing_new);
    assert_eq!(first.session_ids.len(), 40);
    assert_eq!(first.backlog, 5);
    let first_inputs = first.inputs.as_ref().unwrap();
    assert_eq!(first_inputs.profile, profile::template());
    assert_eq!(first_inputs.user_noun, "Daniel");
    assert_eq!(
        first_inputs.instructions.as_deref(),
        Some("Keep Daniel's profile concise.")
    );
    let built = with_section(&profile::template(), "Projects", "- scribe.");
    let ApplyRefreshOutcome::Written(written) =
        apply_at(&store, &first, answer(built.clone()), false, now).unwrap()
    else {
        panic!("expected written");
    };
    assert_eq!(written.revision, Some(1));
    let file = store.root().join("memory/profile.md");
    assert_eq!(fs::read_to_string(&file).unwrap(), built);

    // The user pins a line by hand; the next run sees the edit.
    let pinned = with_section(&built, "Pinned", "- Call me Daniel.");
    fs::write(&file, &pinned).unwrap();
    let second = prepare_at(&store, now);
    assert!(!second.nothing_new);
    assert_eq!(second.session_ids.len(), 5, "resumes after the cursor");
    assert_eq!(second.backlog, 0);
    assert!(
        second
            .inputs
            .as_ref()
            .unwrap()
            .corrections
            .as_deref()
            .unwrap()
            .contains("+ - Call me Daniel.")
    );

    // A rebuild that drops Pinned returns retry+correction when allow_retry is true
    // and fails with profile_pinned when allow_retry is false, writing nothing.
    let retry = apply_at(&store, &second, answer(built.clone()), true, now).unwrap();
    assert!(matches!(
        retry,
        ApplyRefreshOutcome::Retry {
            code: "profile_pinned",
            ..
        }
    ));
    let err = apply_at(&store, &second, answer(built.clone()), false, now).unwrap_err();
    let MemoryError::Core(err) = err else {
        panic!("expected core refusal, got {err:?}");
    };
    assert_eq!(err.code(), "profile_pinned");
    assert_eq!(fs::read_to_string(&file).unwrap(), pinned);
    assert_eq!(prepare_at(&store, now).session_ids.len(), 5);

    let kept = with_section(&pinned, "Projects", "- orbit.");
    let ApplyRefreshOutcome::Written(written) =
        apply_at(&store, &second, answer(kept.clone()), false, now).unwrap()
    else {
        panic!("expected written");
    };
    assert_eq!(written.revision, Some(2));
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains("- Call me Daniel.")
    );

    // Nothing new now; prepare returns nothing_new and the same answer writes no revision.
    let third = prepare_at(&store, now);
    assert!(third.nothing_new);
    let third_inputs = store.profile_inputs(now).unwrap();
    assert!(third_inputs.nothing_new());
    let again = store
        .write_refresh_prepared(
            &ApplyRefreshRequest {
                basis: third_inputs.basis,
                cursor_to: None,
                session_ids: Vec::new(),
                backlog: 0,
                passed_stems: Vec::new(),
                answer: answer(kept.clone()),
                allow_retry: false,
            },
            &redactor,
            now,
        )
        .unwrap();
    assert_eq!(again.revision, None);
    assert_eq!(store.profile_revision(2).unwrap().content, kept);
    // Restoring revision 1 writes it back as revision 3.
    assert_eq!(store.restore_profile(1, now).unwrap(), Some(3));
    assert_eq!(fs::read_to_string(&file).unwrap(), built);
}

#[test]
fn a_refresh_reads_kept_turns_but_not_turns_stored_only_for_a_group() {
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
    sessions(&store, 1, T0);
    let session = id(1);
    let turn = |message: &str, text: &str| crate::message::NewMessage {
        session: session.clone(),
        message: message.to_owned(),
        role: crate::entry::Speaker::User,
        at: T0,
        text: text.to_owned(),
    };
    store
        .write_messages(&[turn("u-1", "kept through the pass")], &redactor)
        .unwrap();
    // Group capture stores a later turn on the same cataloged session.
    let conn = rusqlite::Connection::open(store.root().join(db::DB_FILE)).unwrap();
    conn.execute(
        "INSERT INTO messages (session, message, role, at, text, sha256, group_only)
         VALUES (?1, 'u-2', 'user', ?2, 'said only for the group', replace(hex(zeroblob(32)), '0', 'a'), 1)",
        (session.as_str(), T0.format(&time::format_description::well_known::Rfc3339).unwrap()),
    )
    .unwrap();
    drop(conn);

    let now = T0 + Duration::days(60);
    let prepared = prepare_at(&store, now);
    let verbatim = prepared.inputs.unwrap().sessions[0]
        .verbatim
        .as_deref()
        .unwrap()
        .to_owned();
    assert!(verbatim.contains("kept through the pass"));
    assert!(!verbatim.contains("said only for the group"));
}

#[test]
fn a_refresh_refuses_when_memory_changed_since_its_inputs() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let inputs = store.profile_inputs(T0).unwrap();
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
    let stale = PreparedRefresh {
        nothing_new: false,
        basis: Some(inputs.basis),
        cursor_to: None,
        session_ids: Vec::new(),
        backlog: 0,
        passed_stems: Vec::new(),
        inputs: None,
    };
    // MemoryChanged fails immediately even when allow_retry is true.
    let err = apply_at(&store, &stale, answer(profile::template()), true, T0).unwrap_err();
    let MemoryError::Core(err) = err else {
        panic!("expected core refusal, got {err:?}");
    };
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
    let early = prepare_at(&store, T0 + Duration::days(18));
    assert!(early.nothing_new);
    let now = T0 + Duration::days(25);
    let prepared = prepare_at(&store, now);
    assert!(!prepared.nothing_new);
    let passed: Vec<&str> = prepared.passed_stems.iter().map(|s| s.as_str()).collect();
    assert_eq!(passed, ["2026-11-05-release-ships", "2026-11-06-dentist"]);
    // A milestone with an unknown passed stem returns retry+correction on allow_retry.
    let mut bad = answer(profile::template());
    bad.milestones.push(ProfileMilestoneAnswer {
        now: Some(String::from("2026-11-01-not-a-passed-item")),
        date: String::from("2026-11-01"),
        title: String::from("Bad milestone"),
        text: String::from("Not in passed."),
    });
    assert!(matches!(
        apply_at(&store, &prepared, bad, true, now).unwrap(),
        ApplyRefreshOutcome::Retry {
            code: "profile_milestone",
            ..
        }
    ));
    let mut promoted = answer(profile::template());
    promoted.milestones.push(ProfileMilestoneAnswer {
        now: Some(String::from("2026-11-05-release-ships")),
        date: String::from("2026-11-05"),
        title: String::from("Release shipped"),
        text: String::from("The release shipped."),
    });
    let ApplyRefreshOutcome::Written(written) =
        apply_at(&store, &prepared, promoted, false, now).unwrap()
    else {
        panic!("expected written");
    };
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

fn refused_code(
    result: std::result::Result<
        ApplyRefreshOutcome,
        MemoryError<crate::runtime::errors::CommandError>,
    >,
) -> &'static str {
    match result {
        Err(MemoryError::Core(err)) => err.code(),
        other => panic!("expected a core refusal, got {other:?}"),
    }
}

/// `apply` recomputes what `prepare` covers under the write lock: a request
/// naming a current `now` item as passed is refused, even with
/// `allow_retry`, and nothing is written.
#[test]
fn an_apply_naming_a_current_now_item_as_passed_is_refused() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    let redactor = Redactor::default();
    let r = record(1, T0);
    let window = pass(
        vec![
            draft("Release ships", Some(date!(2026 - 11 - 05))),
            draft("Sister's wedding", Some(date!(2026 - 12 - 12))),
        ],
        false,
    );
    store.write_pass_memory(&window, &redactor, &r).unwrap();
    let now = T0 + Duration::days(25);
    let prepared = prepare_at(&store, now);
    let current = store
        .memory_view()
        .unwrap()
        .now
        .into_iter()
        .find(|n| n.stem.as_str() == "2026-12-12-sister-s-wedding")
        .unwrap()
        .stem;
    let mut forged = request_for(&prepared, answer(profile::template()), true);
    forged.passed_stems.push(current);
    assert_eq!(
        refused_code(apply_request(&store, forged, now)),
        "memory_changed"
    );
    let view = store.memory_view().unwrap();
    assert_eq!(view.now.len(), 2, "nothing was removed");
    assert_eq!(
        prepare_at(&store, now).passed_stems,
        prepared.passed_stems,
        "the passed item is still due"
    );
    // The request as prepared still commits.
    assert!(matches!(
        apply_at(&store, &prepared, answer(profile::template()), false, now).unwrap(),
        ApplyRefreshOutcome::Written(_)
    ));
}

/// A forged `cursor_to` or `session_ids` is refused: the cursor cannot move
/// into the future or past a session the request did not read, and every id
/// must be a session after the cursor.
#[test]
fn an_apply_with_a_forged_cursor_or_session_ids_is_refused() {
    let dir = private_tempdir();
    let store = Store::open(dir.path().join("store"));
    sessions(&store, 3, T0);
    let now = T0 + Duration::days(10);
    let prepared = prepare_at(&store, now);
    assert_eq!(prepared.session_ids.len(), 3);
    let built = with_section(&profile::template(), "Projects", "- scribe.");
    let first = id(1).to_string();
    let forge = |edit: &dyn Fn(&mut ApplyRefreshRequest)| {
        let mut request = request_for(&prepared, answer(built.clone()), false);
        edit(&mut request);
        refused_code(apply_request(&store, request, now))
    };
    // A future cursor would skip every later session for good.
    assert_eq!(
        forge(&|r| r.cursor_to.as_mut().unwrap().at = now + Duration::days(365)),
        "memory_changed"
    );
    // A cursor past a session the request did not read.
    assert_eq!(
        forge(&|r| r.session_ids.retain(|s| *s != first)),
        "memory_changed"
    );
    // A session id that is not a session after the cursor.
    assert_eq!(
        forge(&|r| r.session_ids.push(id(9).to_string())),
        "memory_changed"
    );
    // Session ids without a cursor.
    assert_eq!(forge(&|r| r.cursor_to = None), "memory_changed");
    // The request as prepared commits, and its backlog is recomputed.
    let ApplyRefreshOutcome::Written(written) =
        apply_at(&store, &prepared, answer(built.clone()), false, now).unwrap()
    else {
        panic!("expected written");
    };
    assert_eq!((written.sessions, written.backlog), (3, 0));
    assert!(prepare_at(&store, now).nothing_new);
}

/// `memory refresh` hands a model rebuild to Orbit with an error, but the
/// format-24 archive its prepare made stays committed (it is stepwise).
#[test]
fn a_refresh_that_needs_orbit_keeps_its_format_twenty_four_archive() {
    let dir = private_tempdir();
    let store = seed(&dir.path().join("store"), &Seed::default());
    sessions(&store, 1, T0);
    let mem = store.root().join("memory");
    fs::create_dir_all(&mem).unwrap();
    let path = mem.join("m-1.md");
    fs::write(&path, LEGACY).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(store.root(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&mem, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let host = Context::at(store.root().to_path_buf(), None);
    let err = crate::apps::memory::exec(&host, MemoryCommand::Refresh)
        .err()
        .expect("a new session needs a model run");
    assert!(err.to_string().contains("scribe-memory-refresh"), "{err}");
    assert!(!path.exists(), "the archive stays");
    let archived: i64 = rusqlite::Connection::open(store.root().join(db::DB_FILE))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM archive_records WHERE path = 'memory/m-1.md'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archived, 1);
}

/// A format-24 `m-<n>` item is not read: a read counts it and writes
/// nothing, and the next write archives it (2_design.md §15.9 phase 8),
/// where forgetting the session it cites erases it.
#[test]
fn format_twenty_four_items_are_counted_then_archived_by_the_next_write() {
    let dir = private_tempdir();
    let store = seed(&dir.path().join("store"), &Seed::default());
    let mem = store.root().join("memory");
    fs::create_dir_all(&mem).unwrap();
    let path = mem.join("m-1.md");
    fs::write(&path, LEGACY).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(store.root(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&mem, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let before = tree(store.root());
    assert_eq!(store.memory_view().unwrap().unmigrated, 1);
    assert_eq!(tree(store.root()), before, "a read writes nothing");

    sessions(&store, 1, T0);
    assert!(!path.exists(), "the write archived the item");
    let view = store.memory_view().unwrap();
    assert_eq!(view.unmigrated, 0);
    assert!(view.now.is_empty(), "an archived item is not migrated");
    let archived = |needle: &str| -> i64 {
        rusqlite::Connection::open(store.root().join(db::DB_FILE))
            .unwrap()
            .query_row(
                "SELECT count(*) FROM archive_records
                 WHERE kind = 'other' AND path = 'memory/m-1.md' AND instr(record, ?1) > 0",
                [needle],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(archived("the release ships on december 1"), 1);

    let cite = crate::entry::Cite {
        session: id(2),
        message: Some(String::from("m-1")),
        at: T0,
        prompt_sha256: "1".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    };
    store
        .settle(&crate::consistency::Settle::ForgetSession(id(1)), &cite, T0)
        .unwrap();
    assert_eq!(
        archived("the release ships on december 1"),
        0,
        "forget erases it"
    );
}

/// `prepare` bounds what goes to the model as the in-process prompt did
/// (STD-08 R1): the profile to `PROFILE_CHARS`.
#[test]
fn prepare_bounds_the_profile_it_hands_the_model() {
    let dir = private_tempdir();
    let store = seed(&dir.path().join("store"), &Seed::default());
    sessions(&store, 1, T0);
    let now = T0 + Duration::days(10);
    let built = with_section(&profile::template(), "Projects", "- scribe.");
    let ApplyRefreshOutcome::Written(_) = apply_at(
        &store,
        &prepare_at(&store, now),
        answer(built.clone()),
        false,
        now,
    )
    .unwrap() else {
        panic!("expected written");
    };
    // The user pastes a long block by hand.
    let long = with_section(&built, "Goals", &"x".repeat(crate::models::PROFILE_CHARS));
    let file = store.root().join("memory/profile.md");
    fs::write(&file, &long).unwrap();
    let inputs = prepare_at(&store, now).inputs.unwrap();
    let cut: String = long.chars().take(crate::models::PROFILE_CHARS).collect();
    assert_eq!(inputs.profile, format!("{cut}{}", crate::models::CUT_MARK));
}
