//! `OrbitTasks` against a fake `orbit`: a shell script under a temporary
//! `HOME` that records each call's argv and environment and answers
//! `orbit.search` and `orbit.task.show` from fixtures. No test runs the
//! real `orbit`.
#![cfg(unix)]

use crate::tasks::{OrbitTasks, allowed_env, program};
use scribe_core::ask::{TaskQuery, Tasks};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// A temporary `HOME` with an `orbit` script in it.
struct FakeOrbit {
    dir: tempfile::TempDir,
}

const SEARCH: &str = r#"{"mode":"lexical","kind":"task","results":[{"kind":"task","source":"lexical","id":"T-2","title":"t","summary":"s","status":"done"},{"kind":"friction","source":"lexical","id":"F-1","title":"f","summary":"s","status":"open"}],"notes":[]}"#;
const SHOW: &str = r#"{"id":"T-2","title":"[on-call]: scribe","status":"done","tags":["orchestration"],"updated_at":"2026-11-01T10:00:00.5+00:00","description":"the brief","execution_summary":"what happened"}"#;

impl FakeOrbit {
    fn new(body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = format!(
            "#!/bin/sh\n\
             for a in \"$@\"; do printf '%s\\n' \"<$a>\"; done >> \"$HOME/argv\"\n\
             env > \"$HOME/env\"\n\
             {body}\n"
        );
        let path = dir.path().join("orbit");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir }
    }

    /// Answers search, then show, from the fixtures.
    fn answering() -> Self {
        Self::new(&format!(
            "case \"$3\" in orbit.search) echo '{SEARCH}';; orbit.task.show) echo '{SHOW}';; esac"
        ))
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    fn tasks(&self, extra: &[(&str, &str)]) -> OrbitTasks {
        let mut vars = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("HOME"), self.home().into()),
        ];
        for (k, v) in extra {
            vars.push((OsString::from(*k), OsString::from(*v)));
        }
        OrbitTasks::new(
            self.home().join("orbit").into_os_string(),
            allowed_env(vars),
        )
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.home().join(name)).unwrap_or_default()
    }
}

fn query() -> TaskQuery {
    TaskQuery {
        text: String::from("scribe"),
        tags: vec![String::from("orchestration")],
        limit: 5,
    }
}

#[test]
fn a_search_reads_each_task_hit_in_full_and_skips_other_kinds() {
    let fake = FakeOrbit::answering();
    let found = fake.tasks(&[]).search(&query()).unwrap();
    assert_eq!(found.len(), 1);
    let task = &found[0];
    assert_eq!(task.id, "T-2");
    assert_eq!(task.title, "[on-call]: scribe");
    assert_eq!(task.tags, vec![String::from("orchestration")]);
    assert_eq!(task.execution_summary, "what happened");
    assert_eq!(task.description, "the brief");
    assert!(task.updated.is_some());
    let argv = fake.read("argv");
    assert!(argv.contains("<orbit.search>"), "{argv}");
    assert!(argv.contains(r#""tag":["orchestration"]"#), "{argv}");
    assert!(argv.contains(r#""all":true"#), "{argv}");
    assert!(argv.contains("<orbit.task.show>"), "{argv}");
    assert!(argv.contains(r#""id":"T-2""#), "{argv}");
    assert!(!argv.contains("F-1"), "a friction hit was shown: {argv}");
}

#[test]
fn the_child_gets_orbit_variables_and_never_scribe_ones() {
    let fake = FakeOrbit::answering();
    fake.tasks(&[
        ("ORBIT_PLUGIN_CALLBACK", "cb"),
        ("SCRIBE_ROOT", "/secret"),
        ("ANTHROPIC_API_KEY", "key"),
    ])
    .search(&query())
    .unwrap();
    let env = fake.read("env");
    assert!(env.contains("ORBIT_PLUGIN_CALLBACK=cb"), "{env}");
    assert!(!env.contains("SCRIBE_ROOT"), "{env}");
    assert!(!env.contains("ANTHROPIC_API_KEY"), "{env}");
}

#[test]
fn a_failing_orbit_is_a_task_error() {
    let fake = FakeOrbit::new("echo nope >&2; exit 3");
    let err = fake.tasks(&[]).search(&query()).unwrap_err();
    assert!(err.0.contains("orbit.search"), "{err}");
}

#[test]
fn output_in_another_shape_is_a_task_error() {
    let fake = FakeOrbit::new("echo '{\"unexpected\":1}'");
    let err = fake.tasks(&[]).search(&query()).unwrap_err();
    assert!(err.0.contains("another shape"), "{err}");
}

#[test]
fn a_missing_program_is_a_task_error() {
    let tasks = OrbitTasks::new(OsString::from("/nonexistent/orbit"), Vec::new());
    assert!(tasks.search(&query()).is_err());
}

#[test]
fn a_hung_orbit_is_killed_at_its_budget() {
    let fake = FakeOrbit::new("sleep 30");
    let started = Instant::now();
    let err = fake
        .tasks(&[])
        .with_budgets(Duration::from_millis(300), Duration::from_secs(5))
        .search(&query())
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10), "{err}");
    assert!(err.0.contains("did not answer"), "{err}");
}

#[test]
fn the_program_is_orbit_bin_when_set() {
    assert_eq!(program(None), OsString::from("orbit"));
    assert_eq!(program(Some(OsString::new())), OsString::from("orbit"));
    assert_eq!(
        program(Some(OsString::from("/opt/orbit"))),
        OsString::from("/opt/orbit")
    );
}
