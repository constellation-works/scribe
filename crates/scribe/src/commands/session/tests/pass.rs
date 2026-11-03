//! `session pass` with a fake `Models` (2_design.md §5.3): one record per
//! due session, `scribed_through` advanced, no entries, the previous record
//! kept when the answer is invalid, terms kept only when verbatim from
//! Daniel's turns, and a session never switched on never read.

use crate::app::{AppEnv, Context};
use crate::commands::session::pass::run_with;
use crate::output::{Output, PassPayload};
use scribe_core::control::{self, Switch};
use scribe_core::models::{
    ModelError, ReferentAnswer, ReferentRequest, SessionAnswer, SessionRequest, TermAnswer,
};
use scribe_core::{Models, SessionId};
use serde_json::json;
use std::cell::RefCell;
use std::path::PathBuf;

const ON: &str = "0a1b2c3d-0000-4000-8000-00000000000a";
const NEVER: &str = "0a1b2c3d-0000-4000-8000-00000000000b";
const DEFINED: &str = "by graduate to level 4 I mean a crew that merges without review";
const ASSISTANT: &str = "Level 4 crews merge their own pull requests after green CI.";
const NEVER_SAID: &str = "this session was never switched on and must stay unread";

/// A `Models` answering every session pass with `answer`, keeping each
/// request.
struct FakeModels {
    answer: Result<SessionAnswer, ModelError>,
    asked: RefCell<Vec<SessionRequest>>,
}

impl Models for &FakeModels {
    fn referent(&self, _request: &ReferentRequest) -> Result<ReferentAnswer, ModelError> {
        Err(ModelError::Unavailable(String::from("session tests only")))
    }

    fn session(&self, request: &SessionRequest) -> Result<SessionAnswer, ModelError> {
        self.asked.borrow_mut().push(request.clone());
        self.answer.clone()
    }
}

fn answer() -> SessionAnswer {
    SessionAnswer {
        title: String::from("Defining crew levels"),
        summary: String::from("Daniel defined what level 4 means for crews."),
        goals: String::from("Name the crew levels."),
        outcome: String::from("Level 4 is defined."),
        next: vec![String::from("write it into the routing doc")],
        topics: vec![String::from("crew-routing")],
        related: vec![String::from("task:ORB-7")],
        terms: vec![
            TermAnswer {
                term: String::from("graduate to level 4"),
                said: String::from(DEFINED),
                message: String::from("u-1"),
            },
            TermAnswer {
                term: String::from("level 4 crew"),
                said: String::from("Level 4 crews merge their own pull requests"),
                message: String::from("a-1"),
            },
        ],
    }
}

struct Setup {
    dir: tempfile::TempDir,
    ctx: Context,
}

fn line(kind: &str, uuid: &str, text: &str) -> serde_json::Value {
    json!({"type": kind, "uuid": uuid, "timestamp": "2026-11-03T06:00:00Z",
           "cwd": "/nonexistent/work/scribe",
           "message": {"role": kind, "content": text}})
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
        };
        let ctx = Context::resolve(Some(&dir.path().join("root")), &env).unwrap();
        let setup = Self { dir, ctx };
        setup.write(
            ON,
            &[
                line("user", "u-1", &format!("Crews. {DEFINED}.")),
                line("assistant", "a-1", ASSISTANT),
            ],
        );
        setup.write(NEVER, &[line("user", "n-1", NEVER_SAID)]);
        setup
    }

    fn transcript(&self, session: &str) -> PathBuf {
        self.dir
            .path()
            .join(format!("home/.claude/projects/-work/{session}.jsonl"))
    }

    fn write(&self, session: &str, lines: &[serde_json::Value]) {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(self.transcript(session), text).unwrap();
    }

    fn append(&self, lines: &[serde_json::Value]) {
        let mut text = std::fs::read_to_string(self.transcript(ON)).unwrap();
        text.extend(lines.iter().map(|l| format!("{l}\n")));
        std::fs::write(self.transcript(ON), text).unwrap();
    }

    fn switch_on(&self) {
        let id: SessionId = ON.parse().unwrap();
        self.ctx
            .store()
            .update_control(|s| control::switch(s, &id, &Switch::On, self.ctx.now()))
            .unwrap();
    }

    fn mark_due(&self) {
        let id: SessionId = ON.parse().unwrap();
        self.ctx
            .store()
            .update_control(|s| s.sessions.get_mut(&id).unwrap().due = true)
            .unwrap();
    }

    fn run(&self, models: &FakeModels) -> Vec<PassPayload> {
        let Output::Pass(report) = run_with(&self.ctx, |_| models).unwrap() else {
            panic!("a pass report");
        };
        report.passes
    }

    fn record_file(&self) -> PathBuf {
        self.dir
            .path()
            .join(format!("root/sessions/2026/11/{ON}.md"))
    }

    fn due(&self) -> bool {
        let id: SessionId = ON.parse().unwrap();
        self.ctx.store().control().unwrap().get(&id).unwrap().due
    }
}

fn fake(answer: Result<SessionAnswer, ModelError>) -> FakeModels {
    FakeModels {
        answer,
        asked: RefCell::new(Vec::new()),
    }
}

#[test]
fn a_due_session_gets_one_record_and_its_terms_are_daniels_words_only() {
    let setup = Setup::new();
    setup.switch_on();
    let never_before = std::fs::read(setup.transcript(NEVER)).unwrap();
    let models = fake(Ok(answer()));

    let passes = setup.run(&models);
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0].session, ON);
    assert_eq!(passes[0].result, "written");
    assert_eq!(passes[0].passes, Some(1));
    assert_eq!(passes[0].dropped_terms, 1, "the assistant's term");

    let view = setup.ctx.store().session(&ON.parse().unwrap()).unwrap();
    let record = view.record;
    assert_eq!(record.scribed_through.as_deref(), Some("a-1"));
    assert_eq!(record.passes, 1);
    assert_eq!(record.cwd, "/nonexistent/work/scribe");
    assert_eq!(record.project.as_str(), "scribe");
    assert_eq!(record.next, "- write it into the routing doc");
    assert_eq!(record.terms.len(), 1);
    assert_eq!(record.terms[0].term, "graduate to level 4");
    assert_eq!(record.terms[0].said, DEFINED);
    assert_eq!(record.terms[0].message, "u-1");
    assert!(!view.control.unwrap().due, "due is cleared");
    assert!(
        !setup.dir.path().join("root/entries").exists(),
        "a pass writes no entries"
    );

    // The never-on session: not asked about, not recorded, untouched.
    let asked = models.asked.borrow();
    assert_eq!(asked.len(), 1);
    assert!(!asked[0].prompt().contains(NEVER_SAID));
    assert!(setup.ctx.store().session(&NEVER.parse().unwrap()).is_err());
    assert_eq!(
        std::fs::read(setup.transcript(NEVER)).unwrap(),
        never_before
    );
    drop(asked);

    // Nothing due: the model is not asked again.
    assert!(setup.run(&models).is_empty());
    assert_eq!(models.asked.borrow().len(), 1);
}

#[test]
fn the_next_pass_reads_only_what_came_after_scribed_through() {
    let setup = Setup::new();
    setup.switch_on();
    setup.run(&fake(Ok(answer())));
    setup.append(&[
        line("user", "u-2", "Next, the release crew."),
        line("assistant", "a-2", "Noted, the release crew next."),
    ]);
    setup.mark_due();
    let models = fake(Ok(answer()));
    let passes = setup.run(&models);
    assert_eq!(passes[0].passes, Some(2));
    let asked = models.asked.borrow();
    let ids: Vec<&str> = asked[0].messages.iter().map(|m| m.uuid.as_str()).collect();
    assert_eq!(ids, ["u-2", "a-2"]);
    assert!(asked[0].previous.is_some());
    let record = setup
        .ctx
        .store()
        .session(&ON.parse().unwrap())
        .unwrap()
        .record;
    assert_eq!(record.scribed_through.as_deref(), Some("a-2"));
    assert_eq!(record.terms.len(), 1, "the known term is not repeated");

    // Due again with nothing new: settled without asking the model.
    drop(asked);
    setup.mark_due();
    let idle = fake(Ok(answer()));
    let passes = setup.run(&idle);
    assert_eq!(passes[0].result, "unchanged");
    assert!(idle.asked.borrow().is_empty());
    assert!(!setup.due());
}

#[test]
fn an_invalid_answer_keeps_the_previous_record_and_the_pass_due() {
    let setup = Setup::new();
    setup.switch_on();
    setup.run(&fake(Ok(answer())));
    let before = std::fs::read(setup.record_file()).unwrap();
    setup.append(&[line("user", "u-2", "More on levels.")]);
    setup.mark_due();

    let mut over = answer();
    over.summary = "s".repeat(601);
    for bad in [
        Ok(over),
        Err(ModelError::Timeout {
            budget: std::time::Duration::from_secs(120),
        }),
        Err(ModelError::Malformed(String::from("not JSON"))),
    ] {
        let passes = setup.run(&fake(bad));
        assert_eq!(passes[0].result, "failed");
        assert!(passes[0].error_code.is_some());
        assert_eq!(std::fs::read(setup.record_file()).unwrap(), before);
        assert!(setup.due(), "left due for the next run");
    }
}

#[test]
fn session_end_with_nothing_new_marks_the_record_final_without_a_model() {
    let setup = Setup::new();
    setup.switch_on();
    setup.run(&fake(Ok(answer())));
    let id: SessionId = ON.parse().unwrap();
    setup
        .ctx
        .store()
        .update_control(|s| control::mark_final(s, &id))
        .unwrap();
    let models = fake(Ok(answer()));
    let passes = setup.run(&models);
    assert_eq!(passes[0].result, "written");
    assert!(models.asked.borrow().is_empty());
    let view = setup.ctx.store().session(&id).unwrap();
    assert_eq!(view.record.status, scribe_core::SessionStatus::Final);
    let switch = view.control.unwrap();
    assert!(!switch.due && !switch.final_pass);
}
