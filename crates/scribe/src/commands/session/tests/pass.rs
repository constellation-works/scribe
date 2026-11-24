//! `session pass` with a fake `Models` (2_design.md §5.3): one record per
//! due session, `scribed_through` advanced, no entries, the previous record
//! kept when the answer is invalid, terms kept only when verbatim from
//! Daniel's turns, and a session never switched on never read.

use crate::app::{AppEnv, Context};
use crate::commands::session::pass::{run_with, short_reason};
use crate::output::{Output, PassPayload};
use scribe_core::PassVerbs;
use scribe_core::control::{self, Switch};
use scribe_core::models::{
    ModelError, NewSegmentAnswer, SegmentAnswer, SessionAnswer, SessionHeader,
    SessionHeaderRequest, SessionRequest, TermAnswer,
};
use scribe_core::{Models, Redactor, SessionId};
use serde_json::json;
use std::cell::RefCell;
use std::path::PathBuf;

const ON: &str = "0a1b2c3d-0000-4000-8000-00000000000a";
const NEVER: &str = "0a1b2c3d-0000-4000-8000-00000000000b";
const DEFINED: &str = "by graduate to level 4 I mean a crew that merges without review";
const ASSISTANT: &str = "Level 4 crews merge their own pull requests after green CI.";
const NEVER_SAID: &str = "this session was never switched on and must stay unread";

/// A `Models` answering the first session request with `first`, when set,
/// and every other with `answer`, keeping each request.
struct FakeModels {
    first: RefCell<Option<Result<SessionAnswer, ModelError>>>,
    answer: Result<SessionAnswer, ModelError>,
    asked: RefCell<Vec<SessionRequest>>,
    compressed: RefCell<Vec<SessionHeaderRequest>>,
}

impl Models for &FakeModels {
    fn session(&self, request: &SessionRequest) -> Result<SessionAnswer, ModelError> {
        self.asked.borrow_mut().push(request.clone());
        self.first
            .take()
            .unwrap_or_else(|| self.answer.clone())
            .map(|mut answer| {
                if answer.new.first().is_some_and(|s| s.starts_at == "auto") {
                    if request
                        .previous
                        .as_ref()
                        .and_then(|p| p.open.as_ref())
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

    fn session_header(&self, request: &SessionHeaderRequest) -> Result<SessionHeader, ModelError> {
        self.compressed.borrow_mut().push(request.clone());
        self.answer.clone().map(|a| SessionHeader {
            title: a.title,
            summary: a.summary,
            goals: a.goals,
            next: a.next,
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
            codex_home: None,
            claude_entrypoint: None,
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
        use std::os::unix::fs::PermissionsExt;
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let path = self.transcript(session);
        std::fs::write(&path, text).unwrap();
        // Owner-only, as Claude Code writes it: the umask must not decide it.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
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
        let Output::Pass(report) = run_with(
            &self.ctx,
            |_| models,
            crate::commands::RunLimits::new(scribe_core::pass::MAX_PER_RUN),
            &[],
        )
        .unwrap() else {
            panic!("a pass report");
        };
        report.passes
    }

    /// The `ON` record as its file reads (`scribe export`).
    fn record_text(&self) -> String {
        let target = tempfile::tempdir().unwrap();
        let dir = target.path().join("export");
        self.ctx
            .store()
            .export(&dir, scribe_core::ExportFormat::Markdown)
            .unwrap();
        std::fs::read_to_string(dir.join(format!("sessions/2026/11/{ON}.md"))).unwrap()
    }

    fn due(&self) -> bool {
        let id: SessionId = ON.parse().unwrap();
        self.ctx.store().control().unwrap().get(&id).unwrap().due
    }
}

fn fake(answer: Result<SessionAnswer, ModelError>) -> FakeModels {
    FakeModels {
        first: RefCell::new(None),
        answer,
        asked: RefCell::new(Vec::new()),
        compressed: RefCell::new(Vec::new()),
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
    let before = setup.record_text();
    setup.append(&[line("user", "u-2", "More on levels.")]);
    setup.mark_due();

    let mut over = answer();
    over.summary = format!("{}\n## Injected", "s".repeat(1201));
    for bad in [
        Ok(over),
        Err(ModelError::Timeout {
            budget: std::time::Duration::from_secs(120),
        }),
        Err(ModelError::Malformed(String::from("not JSON"))),
    ] {
        setup
            .ctx
            .store()
            .update_control(|s| {
                s.sessions
                    .get_mut(&ON.parse().unwrap())
                    .unwrap()
                    .retry_after = None
            })
            .unwrap();
        let passes = setup.run(&fake(bad));
        assert_eq!(passes[0].result, "failed");
        assert!(passes[0].error_code.is_some());
        assert_eq!(setup.record_text(), before);
        assert!(setup.due(), "left due for the next run");
    }
}

#[test]
fn an_over_budget_header_is_compressed_once_without_resending_the_window() {
    // ORB-21800: a 626-character summary refused the whole pass.
    let setup = Setup::new();
    setup.switch_on();
    let mut over = answer();
    over.summary = "s".repeat(1201);
    let models = fake(Ok(answer()));
    *models.first.borrow_mut() = Some(Ok(over));

    let passes = setup.run(&models);

    assert_eq!(passes[0].result, "written");
    let asked = models.asked.borrow();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].correction, None);
    let compressed = models.compressed.borrow();
    assert_eq!(compressed.len(), 1);
    let correction = &compressed[0].correction;
    assert!(correction.contains("Summary has 1201 characters; the budget is 1200"));
    assert!(compressed[0].prompt().contains(correction));
    assert_eq!(compressed[0].header.summary, "s".repeat(1201));
    assert!(!compressed[0].prompt().contains(DEFINED));
    assert!(!setup.due());
}

#[test]
fn repeated_header_misses_are_repaired_after_the_fake_models_retry() {
    for field in ["summary", "goals", "next", "title"] {
        let setup = Setup::new();
        setup.switch_on();
        let mut a = answer();
        match field {
            "summary" => a.summary = format!("A sentence. {}", "é".repeat(1200)),
            "goals" => a.goals = format!("A sentence. {}", "é".repeat(600)),
            "next" => a.next = vec![String::from("Keep this bullet"), "é".repeat(1000)],
            "title" => a.title = format!("A sentence. {}", "é".repeat(120)),
            _ => unreachable!(),
        }
        let models = fake(Ok(a));

        let passes = setup.run(&models);

        assert_eq!(passes[0].result, "written", "{field}: {passes:?}");
        assert_eq!(models.asked.borrow().len(), 1);
        assert_eq!(models.compressed.borrow().len(), 1);
        let report = serde_json::to_value(&passes[0]).unwrap();
        assert!(report["warnings"][0].as_str().unwrap().contains(field));
        let record = setup
            .ctx
            .store()
            .session(&ON.parse().unwrap())
            .unwrap()
            .record;
        assert_eq!(
            match field {
                "summary" => record.summary,
                "goals" => record.goals,
                "next" => record.next,
                "title" => record.title,
                _ => unreachable!(),
            },
            if field == "next" {
                "- Keep this bullet"
            } else {
                "A sentence."
            }
        );
        assert!(!setup.due());
    }
}

#[test]
fn repeated_segment_misses_are_repaired_without_changing_sealed_history() {
    for case in [
        "open-length",
        "new-length",
        "old-start",
        "overlap-start",
        "outside-start",
        "overflow-start",
    ] {
        let setup = Setup::new();
        setup.switch_on();
        let mut first = answer();
        first.new[0].starts_at = String::from("u-1");
        let mut second = first.new[0].clone();
        second.starts_at = String::from("a-1");
        first.new.push(second);
        setup.run(&fake(Ok(first)));
        let id = ON.parse().unwrap();
        let before = setup.ctx.store().session(&id).unwrap().record;
        assert_eq!(before.log[0].state, scribe_core::SegmentState::Sealed);
        if case == "overlap-start" {
            // A retained range ahead of the cursor puts a named start inside
            // the window but not after the open segment's last message.
            let mut overlap = before.clone();
            overlap.scribed_through = Some(String::from("u-1"));
            setup
                .ctx
                .store()
                .settle_pass(
                    &id,
                    Some(overlap),
                    false,
                    Some(before.updated),
                    &Redactor::new([]),
                )
                .unwrap();
        }
        setup.append(&[
            line("user", "u-2", "More."),
            line("assistant", "a-2", "Done."),
        ]);
        setup.mark_due();
        let mut a = answer();
        a.new[0].starts_at = String::from("u-2");
        match case {
            "open-length" => {
                let segment = a.new.remove(0);
                a.open = Some(SegmentAnswer {
                    heading: segment.heading,
                    text: format!("Keep this sentence. {}", "é".repeat(900)),
                    topics: segment.topics,
                    related: segment.related,
                });
            }
            "new-length" => a.new[0].text = format!("Keep this sentence. {}", "é".repeat(900)),
            "old-start" | "overlap-start" => a.new[0].starts_at = String::from("a-1"),
            "outside-start" => a.new[0].starts_at = String::from("missing"),
            "overflow-start" => {
                a.new[0].starts_at = String::from("a-1");
                a.new[0].text = "é".repeat(890);
            }
            _ => unreachable!(),
        }
        let models = fake(Ok(a));

        let passes = setup.run(&models);

        assert_eq!(passes[0].result, "written", "{case}: {passes:?}");
        assert_eq!(models.asked.borrow().len(), 2, "one retry: {case}");
        assert!(models.compressed.borrow().is_empty());
        let report = serde_json::to_value(&passes[0]).unwrap();
        let warning = report["warnings"][0].as_str().unwrap();
        assert!(
            warning.contains(if case.ends_with("length") {
                "text"
            } else {
                "starts_at"
            }),
            "{warning}"
        );
        let record = setup.ctx.store().session(&id).unwrap().record;
        assert_eq!(record.log[0], before.log[0], "sealed history: {case}");
        assert_eq!(record.scribed_through.as_deref(), Some("a-2"));
        match case {
            "open-length" => {
                assert_eq!(record.log.len(), 2);
                assert_eq!(record.log[1].text, "Keep this sentence.");
                assert_eq!(record.log[1].state, scribe_core::SegmentState::Sealed);
            }
            "new-length" | "overflow-start" => {
                assert_eq!(record.log.len(), 3, "never drop a new segment");
                assert_eq!(record.log[2].first_message.as_deref(), Some("u-2"));
                assert_eq!(
                    record.log[2].text,
                    if case == "new-length" {
                        "Keep this sentence.".to_owned()
                    } else {
                        "é".repeat(890)
                    }
                );
            }
            "old-start" | "overlap-start" | "outside-start" => {
                assert_eq!(record.log.len(), 2);
                assert_eq!(
                    record.log[1].text,
                    "Level 4 is defined.\n\nLevel 4 is defined."
                );
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn a_fake_model_can_omit_open_without_changing_its_content() {
    let setup = Setup::new();
    setup.switch_on();
    setup.run(&fake(Ok(answer())));
    let id = ON.parse().unwrap();
    let before = setup.ctx.store().session(&id).unwrap().record.log[0].clone();
    setup.append(&[line("user", "u-2", "More on levels.")]);
    setup.mark_due();
    let mut a = answer();
    a.open = None;
    a.new.clear();
    let models = fake(Ok(a));

    assert_eq!(setup.run(&models)[0].result, "written");

    let record = setup.ctx.store().session(&id).unwrap().record;
    assert_eq!(record.log[0].text, before.text);
    assert_eq!(record.log[0].heading, before.heading);
    assert_eq!(record.log[0].last_message.as_deref(), Some("u-2"));
    assert_eq!(models.asked.borrow().len(), 1);
    assert!(models.compressed.borrow().is_empty());
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

#[test]
fn a_plugin_deadline_between_windows_preserves_the_checkpoint_for_the_next_run() {
    let setup = Setup::new();
    setup.write(
        ON,
        &(0..41)
            .map(|n| line("user", &format!("u-{n}"), "Another turn."))
            .collect::<Vec<_>>(),
    );
    setup.switch_on();
    let models = fake(Ok(answer()));
    // Expired before entry: the first window may start, no later one may.
    let limits = crate::commands::RunLimits::new(scribe_core::pass::MAX_PER_RUN)
        .start_by(std::time::Instant::now());
    let Output::Pass(report) = run_with(&setup.ctx, |_| &models, limits, &[]).unwrap() else {
        panic!("a pass report");
    };
    assert_eq!(report.remaining, 1);
    assert_eq!(report.passes[0].windows, 1);
    assert_eq!(models.asked.borrow().len(), 1);
    assert!(setup.due());
    let id = ON.parse().unwrap();
    assert_eq!(
        setup
            .ctx
            .store()
            .session(&id)
            .unwrap()
            .record
            .scribed_through
            .as_deref(),
        Some("u-39")
    );
    let passes = setup.run(&models);
    assert_eq!(passes[0].windows, 1);
    assert_eq!(models.asked.borrow()[1].messages[0].uuid, "u-40");
    assert!(!setup.due());
}

#[test]
fn surfaced_failure_reason_is_redacted_flattened_and_limited() {
    let secret = String::from("sensitive-value-123");
    let redactor = Redactor::new([(String::from("TEST_SECRET"), secret.clone())]);
    let message = format!("model refused {secret}\n{}", "x".repeat(400));

    let reason = short_reason(&redactor, &message);

    assert!(reason.contains("[redacted:TEST_SECRET]"), "{reason}");
    assert!(!reason.contains(&secret), "{reason}");
    assert!(!reason.contains('\n'), "{reason}");
    assert_eq!(reason.chars().count(), 300, "{reason}");
}

#[test]
fn invalid_segment_answers_retry_once_with_the_refusal_and_preserve_the_record() {
    let setup = Setup::new();
    setup.switch_on();
    setup.run(&fake(Ok(answer())));
    let before = setup.record_text();
    setup.append(&[
        line("user", "u-2", "New thread."),
        line("assistant", "a-2", "Another message."),
    ]);
    let valid = || {
        let mut a = answer();
        let s = a.new.remove(0);
        a.open = Some(SegmentAnswer {
            heading: s.heading,
            text: s.text,
            topics: s.topics,
            related: s.related,
        });
        a
    };
    let new = |id: &str| NewSegmentAnswer {
        starts_at: id.to_owned(),
        heading: String::from("New thread"),
        text: String::from("Built it."),
        topics: Vec::new(),
        related: Vec::new(),
    };
    let mut cases = Vec::new();
    let mut a = valid();
    a.open.as_mut().unwrap().text = format!("{}\n# Injected", "é".repeat(901));
    cases.push((a, "seal it and start a new segment"));
    let mut a = valid();
    a.open.as_mut().unwrap().heading = "h".repeat(81);
    cases.push((a, "segment heading"));
    let mut a = valid();
    a.new = vec![new("outside")];
    a.new[0].topics = vec![String::from("Invalid Topic")];
    cases.push((a, "outside the window"));
    let mut a = valid();
    a.new = vec![new("outside"), new("outside")];
    cases.push((a, "outside the window"));
    let mut a = valid();
    a.new = vec![new("a-2"), new("u-2")];
    cases.push((a, "transcript order"));
    let mut a = valid();
    a.new = vec![new("u-2"), new("u-2")];
    cases.push((a, "distinct"));
    let mut a = valid();
    a.summary = format!("{}\n## Injected", "s".repeat(1201));
    cases.push((a, "Summary"));
    let mut a = valid();
    a.goals = format!("{}\n## Injected", "g".repeat(601));
    cases.push((a, "Goals"));
    let mut a = valid();
    a.next = vec!["n".repeat(999)];
    a.topics = vec![String::from("Invalid Topic")];
    cases.push((a, "next"));
    let mut a = valid();
    a.open.as_mut().unwrap().text.clear();
    cases.push((a, "1-900"));
    let mut a = valid();
    a.open.as_mut().unwrap().text = String::from("# Injected heading");
    cases.push((a, "heading line"));
    let mut a = valid();
    a.new = vec![new("u-2"); 4];
    cases.push((a, "at most 3"));
    for (bad, reason) in cases {
        let models = fake(Ok(bad));
        let Output::Pass(report) = run_with(
            &setup.ctx,
            |_| &models,
            crate::commands::RunLimits::new(1),
            &[ON.parse().unwrap()],
        )
        .unwrap() else {
            panic!("report")
        };
        assert_eq!(report.passes[0].error_code, Some("session_answer_invalid"));
        let asked = models.asked.borrow();
        let compressed = models.compressed.borrow();
        assert_eq!(asked.len() + compressed.len(), 2);
        let correction = compressed.first().map_or_else(
            || asked[1].correction.as_deref().unwrap(),
            |request| request.correction.as_str(),
        );
        assert!(correction.contains(reason), "{correction}");
        assert_eq!(setup.record_text(), before);
        assert!(setup.due());
    }
}

#[test]
fn new_segments_derive_ranges_and_sealed_text_is_absent_from_later_requests() {
    let setup = Setup::new();
    setup.switch_on();
    setup.run(&fake(Ok(answer())));
    let mut turns = vec![
        line("user", "u-2", "Finish first thread."),
        line("assistant", "a-2", "Second thread."),
        line("user", "u-3", "Third thread."),
    ];
    for (i, turn) in turns.iter_mut().enumerate() {
        turn["timestamp"] = json!(format!("2026-11-03T06:0{}:00Z", i + 1));
    }
    setup.append(&turns);
    setup.mark_due();
    let mut a = answer();
    a.new.clear();
    a.open = Some(SegmentAnswer {
        heading: String::from("First thread"),
        text: String::from("UNIQUE SEALED FIRST TEXT"),
        topics: Vec::new(),
        related: Vec::new(),
    });
    a.new = ["a-2", "u-3"]
        .map(|id| NewSegmentAnswer {
            starts_at: id.to_owned(),
            heading: format!("Thread {id}"),
            text: format!("Body {id}"),
            topics: vec![String::from("ci")],
            related: vec![String::from("repo:scribe")],
        })
        .to_vec();
    assert_eq!(setup.run(&fake(Ok(a)))[0].result, "written");
    let record = setup
        .ctx
        .store()
        .session(&ON.parse().unwrap())
        .unwrap()
        .record;
    assert_eq!(record.log.len(), 3);
    assert_eq!(record.log[0].last_message.as_deref(), Some("u-2"));
    assert_eq!(record.log[0].windows.get(), 2);
    assert_eq!(record.log[1].first_message.as_deref(), Some("a-2"));
    assert_eq!(record.log[1].last_message.as_deref(), Some("a-2"));
    assert_eq!(record.log[1].windows.get(), 1);
    assert_eq!(
        record.log[1].from,
        time::macros::datetime!(2026-11-03 06:02 UTC)
    );
    assert_eq!(
        record.log[0].to,
        time::macros::datetime!(2026-11-03 06:01 UTC)
    );
    let file_before = setup.record_text();
    let sealed_body = file_before
        .split("## Log\n")
        .nth(1)
        .unwrap()
        .split("### 3.")
        .next()
        .unwrap();
    setup.append(&[line("assistant", "a-3", "Continue third thread.")]);
    setup.mark_due();
    let models = fake(Ok(answer()));
    setup.run(&models);
    let asked = models.asked.borrow();
    let p = asked[0].previous.as_ref().unwrap();
    assert_eq!(p.sealed.len(), 2);
    assert_eq!(p.open.as_ref().unwrap().text, "Body u-3");
    assert!(!asked[0].prompt().contains("UNIQUE SEALED FIRST TEXT"));
    assert!(!asked[0].prompt().contains("Body a-2"));
    assert!(asked[0].prompt().contains("First thread"));
    let after = setup.record_text();
    assert_eq!(
        after
            .split("## Log\n")
            .nth(1)
            .unwrap()
            .split("### 3.")
            .next()
            .unwrap(),
        sealed_body
    );
}

#[test]
fn windows_seal_at_the_configured_limit_and_the_last_final_window() {
    for (limit, final_pass) in [(3, false), (2, false), (3, true)] {
        let setup = Setup::new();
        setup.write(
            ON,
            &(0..7)
                .map(|i| line("user", &format!("m-{i}"), "Work."))
                .collect::<Vec<_>>(),
        );
        setup.switch_on();
        let config = setup.ctx.store().root().join("config.toml");
        std::fs::write(
            &config,
            format!("[catalog]\nwindow_messages = 1\nsegment_windows = {limit}\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        if final_pass {
            setup
                .ctx
                .store()
                .update_control(|s| control::mark_final(s, &ON.parse().unwrap()))
                .unwrap();
        }
        let models = fake(Ok(answer()));
        let report = setup.run(&models);
        let record = setup
            .ctx
            .store()
            .session(&ON.parse().unwrap())
            .unwrap()
            .record;
        assert_eq!(record.log.len(), if limit == 3 { 3 } else { 4 });
        for segment in &record.log[..record.log.len() - 1] {
            assert_eq!(segment.windows.get(), limit);
            assert_eq!(segment.state, scribe_core::SegmentState::Sealed);
        }
        assert_eq!(
            record.log.last().unwrap().state,
            if final_pass {
                scribe_core::SegmentState::Sealed
            } else {
                scribe_core::SegmentState::Open
            }
        );
        assert_eq!(
            report[0].segments_sealed,
            record
                .log
                .iter()
                .filter(|s| s.state == scribe_core::SegmentState::Sealed)
                .count()
        );
        assert_eq!(models.asked.borrow().len(), 7);
        assert_eq!(models.asked.borrow()[6].final_pass, final_pass);
    }
}
