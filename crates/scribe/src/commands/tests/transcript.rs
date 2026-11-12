//! `transcript list` and `transcript show` (2_design.md §5.8): uncataloged
//! transcripts only, filters, a read-only root, and Daniel's first turns.

use crate::app::{AppEnv, Context};
use crate::commands::transcript::{ListArgs, ShowArgs, TranscriptCommand};
use crate::error::CliError;
use crate::output::{Output, Record, Records};
use scribe_core::{Project, SessionId};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use time::{Date, Month};

const LONG: &str = "sess-long-0001";
const SHORT: &str = "sess-short-0002";
const SWITCHED: &str = "sess-switched-03";
const CORRUPT: &str = "sess-corrupt-004";
const LATER: &str = "sess-later-0005";

struct Setup {
    dir: tempfile::TempDir,
    ctx: Context,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".claude/projects/-work")).unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        // The store refuses a group-writable root; umask here is 002.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let env = AppEnv {
            root: None,
            home: Some(home.into_os_string()),
            claude_config_dir: None,
            codex_home: None,
        };
        let ctx = Context::resolve(Some(&root), &env).unwrap();
        Self { dir, ctx }
    }

    fn projects_dir(&self) -> PathBuf {
        self.dir.path().join("home/.claude/projects/-work")
    }

    fn write(&self, session: &str, lines: &[serde_json::Value]) {
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::write(self.projects_dir().join(format!("{session}.jsonl")), text).unwrap();
    }

    fn config(&self, text: &str) {
        let path = self.ctx.store().root().join("config.toml");
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn list(&self, args: ListArgs) -> Output {
        crate::commands::transcript::run(TranscriptCommand::List(args), &self.ctx).unwrap()
    }
}

fn day(year: i32, month: u8, day: u8) -> Date {
    Date::from_calendar_date(year, Month::try_from(month).unwrap(), day).unwrap()
}

fn line(uuid: &str, at: &str, cwd: &str, text: &str) -> serde_json::Value {
    json!({
        "type": "user",
        "uuid": uuid,
        "timestamp": at,
        "cwd": cwd,
        "message": {"role": "user", "content": text}
    })
}

fn args(since: Date) -> ListArgs {
    ListArgs {
        since,
        until: None,
        project: Vec::new(),
        min_turns: 1,
        limit: None,
    }
}

fn rows(output: Output) -> Vec<crate::output::TranscriptPayload> {
    let Output::List {
        records: Records::Transcripts(rows),
        ..
    } = output
    else {
        panic!("a transcript list");
    };
    rows
}

#[test]
fn list_filters_by_span_project_turns_and_limit_newest_first() {
    let setup = Setup::new();
    setup.config(
        "[projects.orbit]\npaths = [\"codebases/orbit\"]\n\n[projects.scribe]\npaths = [\"codebases/scribe\"]\n",
    );
    setup.write(
        LONG,
        &[
            line(
                "u-1",
                "2026-10-15T10:00:00Z",
                "/work/constellation",
                "codebases/orbit first",
            ),
            line(
                "u-2",
                "2026-10-16T11:00:00Z",
                "/work/constellation",
                "codebases/orbit again",
            ),
        ],
    );
    setup.write(
        SHORT,
        &[line(
            "u-1",
            "2026-10-15T12:00:00Z",
            "/work/scribe",
            "one turn only",
        )],
    );
    setup.write(
        LATER,
        &[
            line(
                "a",
                "2026-10-20T09:00:00Z",
                "/work/scribe",
                "codebases/scribe one",
            ),
            line(
                "b",
                "2026-10-20T10:00:00Z",
                "/work/scribe",
                "codebases/scribe two",
            ),
        ],
    );
    // A span that ends before --since is left out; one that overlaps stays.
    let october = rows(setup.list(args(day(2026, 10, 16))));
    let ids: Vec<&str> = october.iter().map(|row| row.session.as_str()).collect();
    assert_eq!(
        ids,
        [LATER, LONG],
        "newest started first; a span that reaches the 16th overlaps"
    );
    assert_eq!(october[1].project, "orbit");
    assert_eq!(october[1].project_mentions["orbit"], 2);
    assert_eq!(october[1].project_mentions["scribe"], 0);
    assert_eq!(october[0].project, "scribe");
    assert_eq!(october[1].daniel_turns, 2);
    assert_eq!(october[1].first_prompt, "codebases/orbit first");
    assert!(october[1].daniel_first_turns.is_none());

    let mut narrow = args(day(2026, 10, 1));
    narrow.until = Some(day(2026, 10, 14));
    assert!(rows(setup.list(narrow)).is_empty());

    let mut min = args(day(2026, 10, 1));
    min.min_turns = 2;
    let long_only = rows(setup.list(min));
    assert!(long_only.iter().all(|row| row.session != SHORT));
    assert_eq!(long_only.len(), 2);

    let mut only_orbit = args(day(2026, 10, 1));
    only_orbit.project = vec![Project::sanitized("orbit")];
    assert_eq!(rows(setup.list(only_orbit)).len(), 1);

    let mut limited = args(day(2026, 10, 1));
    limited.limit = Some(std::num::NonZeroUsize::new(1).unwrap());
    let Output::List {
        records: Records::Transcripts(shown),
        total,
        notices,
        ..
    } = setup.list(limited)
    else {
        panic!("list");
    };
    assert_eq!(total, 3);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].session, LATER);
    assert!(notices.iter().any(|line| line.contains("showing 1 of 3")));
}

#[test]
fn list_skips_cataloged_transcripts_and_provenance_failures() {
    let setup = Setup::new();
    setup.write(
        LONG,
        &[line(
            "u-1",
            "2026-10-02T10:00:00Z",
            "/work/scribe",
            "keep me",
        )],
    );
    setup.write(
        SWITCHED,
        &[line(
            "u-1",
            "2026-10-03T10:00:00Z",
            "/work/scribe",
            "switched on",
        )],
    );
    setup.write(
        CORRUPT,
        &[line(
            "u-1",
            "2026-10-04T10:00:00Z",
            "/work/scribe",
            "has a bad record",
        )],
    );
    let link = setup.projects_dir().join("sess-link-0006.jsonl");
    std::os::unix::fs::symlink("missing-target", &link).unwrap();

    let switched: SessionId = SWITCHED.parse().unwrap();
    setup
        .ctx
        .store()
        .update_control(|state| {
            scribe_core::control::switch(
                state,
                &switched,
                &scribe_core::control::Switch::On,
                setup.ctx.now(),
            );
        })
        .unwrap();
    let record = setup
        .ctx
        .store()
        .root()
        .join(format!("sessions/2026/10/{CORRUPT}.md"));
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    std::fs::write(&record, "not a session\n").unwrap();
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
    for dir in [
        setup.ctx.store().root().join("sessions"),
        setup.ctx.store().root().join("sessions/2026"),
        setup.ctx.store().root().join("sessions/2026/10"),
    ] {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    let found = rows(setup.list(args(day(2026, 10, 1))));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].session, LONG);
}

#[test]
fn show_prints_the_first_five_turns_and_resolves_a_unique_prefix() {
    let setup = Setup::new();
    let mut lines = Vec::new();
    for n in 1..=6 {
        lines.push(line(
            &format!("u-{n}"),
            "2026-10-15T10:00:00Z",
            "/work/scribe",
            &format!("turn {n} {}", "x".repeat(350)),
        ));
    }
    setup.write(LONG, &lines);
    setup.write(
        "sess-long-9999",
        &[line("u-1", "2026-10-15T10:00:00Z", "/work/scribe", "other")],
    );

    let Output::Show {
        record: Record::Transcript(row),
    } = crate::commands::transcript::run(
        TranscriptCommand::Show(ShowArgs {
            id: "sess-long-000".parse().unwrap(),
        }),
        &setup.ctx,
    )
    .unwrap()
    else {
        panic!("show");
    };
    assert_eq!(row.session, LONG);
    let turns = row.daniel_first_turns.unwrap();
    assert_eq!(turns.len(), 5);
    assert!(turns.iter().all(|turn| turn.chars().count() == 300));
    assert!(turns[0].starts_with("turn 1 "));

    let ambiguous = crate::commands::transcript::run(
        TranscriptCommand::Show(ShowArgs {
            id: "sess-long-".parse().unwrap(),
        }),
        &setup.ctx,
    )
    .unwrap_err();
    assert_eq!(ambiguous.code(), "id_ambiguous");
    assert_eq!(ambiguous.exit_code(), 2);

    let missing = crate::commands::transcript::run(
        TranscriptCommand::Show(ShowArgs {
            id: "zzzz-missing".parse().unwrap(),
        }),
        &setup.ctx,
    )
    .unwrap_err();
    assert!(matches!(missing, CliError::TranscriptNotFound { .. }));
    assert_eq!(missing.code(), "transcript_not_found");
}

#[test]
fn list_writes_nothing_on_a_read_only_root() {
    let setup = Setup::new();
    setup.config("[projects.orbit]\npaths = [\"codebases/orbit\"]\n");
    setup.write(
        LONG,
        &[line(
            "u-1",
            "2026-10-15T10:00:00Z",
            "/work/constellation",
            "work in codebases/orbit",
        )],
    );
    let root = setup.ctx.store().root();
    let before = names(root);
    // Owner read-only: the store refuses group and other bits (STD-05 R9).
    let _guard = ModeGuard::set(root, 0o500);
    let found = rows(setup.list(args(day(2026, 10, 1))));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].project, "orbit");
    assert_eq!(names(root), before);
    assert!(!root.join("audit.jsonl").exists());
    assert!(!root.join(".lock").exists());
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|item| item.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// Restores a directory mode so the temp root can be removed.
struct ModeGuard {
    path: PathBuf,
    mode: u32,
}

impl ModeGuard {
    fn set(path: &Path, mode: u32) -> Self {
        let current = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        Self {
            path: path.to_path_buf(),
            mode: current,
        }
    }
}

impl Drop for ModeGuard {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.mode));
    }
}
