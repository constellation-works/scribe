//! `session catalog` with a fake `Models` (2_design.md §5.8): a never-switched
//! session is switched on with `cataloged_by`, every window runs, an ended
//! transcript is marked final and off, a live one stays due, and a failure
//! resumes without moving `since`.

use super::super::catalog::{self, CatalogArgs};
use crate::app::{AppEnv, Context};
use crate::output::{Output, PassReport};
use scribe_core::control::{self, Switch};
use scribe_core::models::{
    ModelError, NewSegmentAnswer, SegmentAnswer, SessionAnswer, SessionHeader,
    SessionHeaderRequest, SessionRequest, TermAnswer,
};
use scribe_core::{AuthorKind, Models, Project, SessionId, SessionStatus};
use serde_json::json;
use std::cell::RefCell;
use std::os::unix::fs::PermissionsExt;
use time::format_description::well_known::Rfc3339;

const ID: &str = "0a1b2c3d-0000-4000-8000-0000000000c1";
const DEFINED: &str = "by graduate to level 4 I mean a crew that merges without review";

struct FakeModels {
    answer: Result<SessionAnswer, ModelError>,
    asked: RefCell<usize>,
}

impl Models for &FakeModels {
    fn session(&self, request: &SessionRequest) -> Result<SessionAnswer, ModelError> {
        *self.asked.borrow_mut() += 1;
        self.answer.clone().map(|mut answer| {
            if answer
                .new
                .first()
                .is_some_and(|segment| segment.starts_at == "auto")
            {
                if request
                    .previous
                    .as_ref()
                    .and_then(|previous| previous.open.as_ref())
                    .is_some()
                {
                    let segment = answer.new.remove(0);
                    answer.open = Some(SegmentAnswer {
                        heading: segment.heading,
                        text: segment.text,
                        topics: segment.topics,
                        related: segment.related,
                    });
                } else {
                    answer.new[0].starts_at = request.messages[0].uuid.clone();
                }
            }
            answer
        })
    }

    fn session_header(&self, _request: &SessionHeaderRequest) -> Result<SessionHeader, ModelError> {
        self.answer.clone().map(|answer| SessionHeader {
            title: answer.title,
            summary: answer.summary,
            goals: answer.goals,
            next: answer.next,
        })
    }
}

fn answer() -> SessionAnswer {
    SessionAnswer {
        candidates: Vec::new(),
        memory: Vec::new(),
        signal: None,
        title: String::from("Defining crew levels"),
        summary: String::from("Daniel defined what level 4 means for crews."),
        goals: String::from("Name the crew levels."),
        open: None,
        new: vec![NewSegmentAnswer {
            starts_at: String::from("auto"),
            heading: String::from("Crew levels"),
            text: String::from("Level 4 is defined."),
            topics: Vec::new(),
            related: Vec::new(),
        }],
        next: vec![String::from("write it into the routing doc")],
        topics: vec![String::from("crew-routing")],
        related: Vec::new(),
        terms: vec![TermAnswer {
            term: String::from("graduate to level 4"),
            said: String::from(DEFINED),
            message: String::from("u-1"),
        }],
    }
}

struct Setup {
    dir: tempfile::TempDir,
    ctx: Context,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".claude/projects/-work")).unwrap();
        let env = AppEnv {
            root: None,
            home: Some(home.into_os_string()),
            claude_config_dir: None,
            codex_home: None,
            claude_entrypoint: None,
        };
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let ctx = Context::resolve(Some(&root), &env).unwrap();
        let setup = Self { dir, ctx };
        setup.config("[catalog]\nwindow_messages = 1\n");
        setup
    }

    fn config(&self, text: &str) {
        let path = self.ctx.store().root().join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn transcript(&self) -> std::path::PathBuf {
        self.dir
            .path()
            .join(format!("home/.claude/projects/-work/{ID}.jsonl"))
    }

    fn write_at(&self, lines: &[(&str, &str, &str)]) {
        let text: String = lines
            .iter()
            .map(|(uuid, at, text)| {
                format!(
                    "{}\n",
                    json!({
                        "type": "user",
                        "uuid": uuid,
                        "timestamp": at,
                        "cwd": "/nonexistent/work/scribe",
                        "message": {"role": "user", "content": text}
                    })
                )
            })
            .collect();
        std::fs::write(self.transcript(), text).unwrap();
    }

    fn stamp(&self, ago: time::Duration) -> String {
        (self.ctx.now() - ago).format(&Rfc3339).unwrap()
    }

    fn args(&self, project: Option<&str>) -> CatalogArgs {
        self.args_by("opus", project)
    }

    fn args_by(&self, author: &str, project: Option<&str>) -> CatalogArgs {
        CatalogArgs {
            id: ID.parse().unwrap(),
            author: author.parse().unwrap(),
            project: project.map(|slug| slug.parse::<Project>().unwrap()),
        }
    }

    fn run(
        &self,
        models: &FakeModels,
        project: Option<&str>,
    ) -> Result<PassReport, crate::error::CliError> {
        let Output::Pass(report) = catalog::run_with(&self.args(project), &self.ctx, |_| models)?
        else {
            panic!("session catalog returns a pass report");
        };
        Ok(report)
    }

    fn run_by(
        &self,
        models: &FakeModels,
        author: &str,
    ) -> Result<PassReport, crate::error::CliError> {
        let Output::Pass(report) =
            catalog::run_with(&self.args_by(author, None), &self.ctx, |_| models)?
        else {
            panic!("session catalog returns a pass report");
        };
        Ok(report)
    }

    fn id(&self) -> SessionId {
        ID.parse().unwrap()
    }
}

fn fake(answer: Result<SessionAnswer, ModelError>) -> FakeModels {
    FakeModels {
        answer,
        asked: RefCell::new(0),
    }
}

#[test]
fn an_ended_transcript_is_cataloged_final_and_off_with_no_entries() {
    let setup = Setup::new();
    let old = setup.stamp(time::Duration::hours(25));
    setup.write_at(&[
        ("u-1", &old, &format!("Crews. {DEFINED}.")),
        ("u-2", &old, "And the second window."),
    ]);
    let models = fake(Ok(answer()));
    let report = setup.run(&models, None).unwrap();
    assert!(report.switch_written);
    assert_eq!(report.passes[0].result, "written");
    assert_eq!(report.passes[0].windows, 2);
    assert!(*models.asked.borrow() >= 2);

    let view = setup.ctx.store().session(&setup.id()).unwrap();
    assert_eq!(view.record.status, SessionStatus::Final);
    assert!(
        view.record
            .log
            .iter()
            .all(|segment| segment.origin == scribe_core::SegmentOrigin::Catalog)
    );
    let by = view.record.cataloged_by.unwrap();
    assert_eq!(by.by.kind, AuthorKind::Agent);
    assert_eq!(by.by.name.as_str(), "opus");
    assert_eq!(view.record.project.as_str(), "scribe");
    assert_eq!(
        view.record.on_since,
        view.control.as_ref().unwrap().since.unwrap()
    );
    let control = view.control.unwrap();
    assert!(!control.on && !control.due);
    assert!(control.cataloged_by.is_some());
    assert!(!setup.ctx.store().root().join("entries").exists());

    let again = setup.run(&fake(Ok(answer())), None).unwrap_err();
    assert_eq!(again.code(), "session_cataloged");
}

#[test]
fn a_slash_scribe_on_switch_is_refused_before_any_model_call() {
    let setup = Setup::new();
    let recent = setup.stamp(time::Duration::hours(1));
    setup.write_at(&[("u-1", &recent, "Already switched.")]);
    setup
        .ctx
        .store()
        .update_control(|state| control::switch(state, &setup.id(), &Switch::On, setup.ctx.now()))
        .unwrap();
    let models = fake(Ok(answer()));
    let err = setup.run(&models, None).unwrap_err();
    assert_eq!(err.code(), "session_cataloged");
    assert_eq!(*models.asked.borrow(), 0);
    assert!(setup.ctx.store().session(&setup.id()).is_err());
}

#[test]
fn a_failed_catalog_resumes_without_moving_since_or_who_cataloged_it() {
    let setup = Setup::new();
    let recent = setup.stamp(time::Duration::hours(1));
    setup.write_at(&[("u-1", &recent, &format!("Crews. {DEFINED}."))]);
    let failing = fake(Err(ModelError::Malformed(String::from("not JSON"))));
    let failed = setup.run(&failing, None).unwrap();
    assert!(failed.switch_written);
    assert_eq!(failed.passes[0].result, "failed");
    let message = failed.failure().unwrap().to_string();
    assert!(message.contains("session catalog"), "{message}");
    let control = setup.ctx.store().control().unwrap();
    let first = control.get(&setup.id()).unwrap();
    let since = first.since.unwrap();
    let at = first.cataloged_by.as_ref().unwrap().at;
    assert!(first.on && first.due);
    assert!(first.retry_after.is_some());
    assert!(setup.ctx.store().session(&setup.id()).is_err());

    let models = fake(Ok(answer()));
    let resumed = setup.run(&models, Some("orbit")).unwrap();
    assert!(!resumed.switch_written);
    assert_eq!(resumed.passes[0].result, "written");
    let view = setup.ctx.store().session(&setup.id()).unwrap();
    let control = view.control.unwrap();
    assert_eq!(control.since, Some(since));
    assert_eq!(control.cataloged_by.unwrap().at, at);
    assert!(
        control.on && control.due,
        "a transcript under a day old stays due"
    );
    assert_eq!(control.consecutive_failures, 0);
    assert!(control.retry_after.is_none());
    // The first record did not exist yet, so this run's --project applies.
    assert_eq!(view.record.project.as_str(), "orbit");
    assert_eq!(view.record.cataloged_by.unwrap().at, at);
}

#[test]
fn a_live_catalog_keeps_the_recorded_project_when_resumed_with_another() {
    let setup = Setup::new();
    let recent = setup.stamp(time::Duration::hours(1));
    setup.write_at(&[("u-1", &recent, &format!("Crews. {DEFINED}."))]);
    let models = fake(Ok(answer()));
    setup.run(&models, Some("orbit")).unwrap();
    let mut text = std::fs::read_to_string(setup.transcript()).unwrap();
    text.push_str(&format!(
        "{}\n",
        json!({
            "type": "user",
            "uuid": "u-2",
            "timestamp": recent,
            "cwd": "/nonexistent/work/scribe",
            "message": {"role": "user", "content": "A second window."}
        })
    ));
    std::fs::write(setup.transcript(), text).unwrap();
    setup.run(&fake(Ok(answer())), Some("scribe")).unwrap();
    let record = setup.ctx.store().session(&setup.id()).unwrap().record;
    assert_eq!(record.project.as_str(), "orbit");
    assert_eq!(record.passes, 2);
    assert!(
        record
            .log
            .iter()
            .all(|segment| segment.origin == scribe_core::SegmentOrigin::Catalog)
    );
    let control = setup.ctx.store().control().unwrap();
    let switch = control.get(&setup.id()).unwrap();
    assert!(switch.on && switch.due);
}

#[test]
fn an_agent_catalog_is_recorded_as_an_agent_and_no_cli_catalog_records_daniel() {
    let setup = Setup::new();
    let old = setup.stamp(time::Duration::hours(25));
    setup.write_at(&[("u-1", &old, &format!("Crews. {DEFINED}."))]);

    // Every reserved name is refused before any switch or model call;
    // `--author daniel` has no terminal exception (Daniel catalogs through
    // `/scribe catalog <id>` in his prompt).
    for name in ["daniel", "scribe", "consistency"] {
        let models = fake(Ok(answer()));
        let err = setup.run_by(&models, name).unwrap_err();
        assert_eq!(err.code(), "reserved_author", "{name}");
        assert_eq!(*models.asked.borrow(), 0);
    }
    assert!(
        setup
            .ctx
            .store()
            .control()
            .unwrap()
            .get(&setup.id())
            .is_none()
    );

    // The same call under an agent's own name catalogs, recorded as agent.
    let models = fake(Ok(answer()));
    let report = setup.run_by(&models, "opus").unwrap();
    assert!(report.switch_written);
    let by = setup
        .ctx
        .store()
        .session(&setup.id())
        .unwrap()
        .record
        .cataloged_by
        .unwrap()
        .by;
    assert_eq!(by.kind, AuthorKind::Agent);
    assert_eq!(by.name.as_str(), "opus");
}

#[test]
fn a_daniel_catalog_switch_is_finished_by_session_catalog_and_keeps_daniel() {
    let setup = Setup::new();
    let old = setup.stamp(time::Duration::hours(25));
    setup.write_at(&[("u-1", &old, &format!("Crews. {DEFINED}."))]);
    // What `/scribe catalog <id>` writes: the switch, no window.
    let begun = catalog::begin(&setup.ctx, &setup.id(), scribe_core::Author::user()).unwrap();
    assert!(begun.wrote_switch);
    assert!(setup.ctx.store().session(&setup.id()).is_err());

    // An agent running the windows resumes it; the attribution stays Daniel's.
    let report = setup.run(&fake(Ok(answer())), None).unwrap();
    assert!(!report.switch_written);
    let view = setup.ctx.store().session(&setup.id()).unwrap();
    assert_eq!(view.record.status, SessionStatus::Final);
    assert_eq!(view.record.cataloged_by.unwrap().by.kind, AuthorKind::User);
    assert!(!view.control.unwrap().on);
}

#[test]
fn a_daniel_catalog_switch_is_finished_as_a_catalog_by_an_ordinary_pass() {
    let setup = Setup::new();
    let old = setup.stamp(time::Duration::hours(25));
    setup.write_at(&[("u-1", &old, &format!("Crews. {DEFINED}."))]);
    catalog::begin(&setup.ctx, &setup.id(), scribe_core::Author::user()).unwrap();

    let models = fake(Ok(answer()));
    let Output::Pass(report) = super::super::pass::run_with(
        &setup.ctx,
        |_| &models,
        crate::commands::RunLimits::new(5),
        &[],
    )
    .unwrap() else {
        panic!("session pass returns a pass report");
    };
    assert_eq!(report.passes[0].result, "written");
    let view = setup.ctx.store().session(&setup.id()).unwrap();
    assert_eq!(view.record.status, SessionStatus::Final);
    assert!(
        view.record
            .log
            .iter()
            .all(|segment| segment.origin == scribe_core::SegmentOrigin::Catalog)
    );
    assert_eq!(view.record.cataloged_by.unwrap().by.kind, AuthorKind::User);
    let control = view.control.unwrap();
    assert!(!control.on && !control.due);
}

#[test]
fn an_empty_transcript_stays_due_and_calls_no_model() {
    let setup = Setup::new();
    std::fs::write(setup.transcript(), "").unwrap();
    let models = fake(Ok(answer()));
    let report = setup.run(&models, None).unwrap();
    assert!(report.switch_written);
    assert_eq!(report.passes[0].result, "waiting");
    assert_eq!(*models.asked.borrow(), 0);
    assert!(setup.ctx.store().session(&setup.id()).is_err());
    let control = setup.ctx.store().control().unwrap();
    let switch = control.get(&setup.id()).unwrap();
    assert!(switch.on && switch.due);
    assert!(switch.cataloged_by.is_some());
}

#[test]
fn a_slash_scribe_on_that_lands_while_the_catalog_waits_for_the_lock_is_not_overwritten() {
    let setup = Setup::new();
    let recent = setup.stamp(time::Duration::hours(1));
    setup.write_at(&[("u-1", &recent, "A live session.")]);
    let store = setup.ctx.store();
    let setup = &setup;
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();

    let (switched, begun) = std::thread::scope(|scope| {
        // Daniel's `/scribe on` holds the store lock; the catalog starts
        // while it is held and can only gate once it is released.
        let on = scope.spawn(move || {
            store
                .update_control(|state| {
                    control::switch(state, &setup.id(), &Switch::On, setup.ctx.now());
                    state
                        .sessions
                        .get_mut(&setup.id())
                        .unwrap()
                        .pending_links
                        .push(scribe_core::SessionLink {
                            kind: scribe_core::LinkType::Continues,
                            session: "0a1b2c3d-0000-4000-8000-0000000000c9".parse().unwrap(),
                        });
                    held_tx.send(()).unwrap();
                    go_rx.recv().unwrap();
                })
                .unwrap();
        });
        held_rx.recv().unwrap();
        let catalog = scope.spawn(move || {
            catalog::begin(
                &setup.ctx,
                &setup.id(),
                scribe_core::Author::agent("opus".parse().unwrap()),
            )
        });
        go_tx.send(()).unwrap();
        on.join().unwrap();
        let begun = catalog.join().unwrap();
        (store.control().unwrap().get(&setup.id()).cloned(), begun)
    });

    assert_eq!(begun.err().map(|e| e.code()), Some("session_cataloged"));
    let switch = switched.unwrap();
    assert!(
        switch.cataloged_by.is_none(),
        "the catalog overwrote /scribe on"
    );
    assert_eq!(switch.pending_links.len(), 1);
}
