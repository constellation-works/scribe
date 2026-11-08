//! `user-prompt-submit` with a fake `Models`: form-3 lines resolved by a
//! scripted answer, and each failure (a timeout, a model error, a span that
//! fails the verbatim check) still writing exactly one entry, the guessed
//! fallback, with an echo that says so (2_design.md §4.3).

use crate::app::{AppEnv, Context};
use crate::commands::hook::user_prompt_submit::run_with;
use crate::output::Output;
use scribe_core::models::{
    ModelError, ReferentAnswer, ReferentRequest, SessionAnswer, SessionRequest,
};
use scribe_core::{ContentFrom, EntryFilter, Models, Speaker};
use serde_json::json;
use std::cell::Cell;
use std::time::Duration;

const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
const SPOKEN: &str = "I recommend we squash-merge agent PRs and keep merge commits for releases.";

/// A `Models` answering every referent with `answer`, counting calls.
struct FakeModels {
    answer: Result<ReferentAnswer, ModelError>,
    calls: Cell<usize>,
}

impl Models for FakeModels {
    fn referent(&self, _request: &ReferentRequest) -> Result<ReferentAnswer, ModelError> {
        self.calls.set(self.calls.get() + 1);
        self.answer.clone()
    }
    fn session(&self, _request: &SessionRequest) -> Result<SessionAnswer, ModelError> {
        Err(ModelError::Unavailable(String::from("referent tests only")))
    }
}

/// A temp home with a transcript for `SESSION`, and a context over a temp
/// store.
struct Setup {
    dir: tempfile::TempDir,
    ctx: Context,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let projects = home.join(".claude/projects/-work");
        std::fs::create_dir_all(&projects).unwrap();
        let lines = [
            json!({"type": "user", "uuid": "u-1", "timestamp": "2026-11-03T06:00:00Z",
                   "message": {"role": "user", "content": "Should we squash agent PRs?"}}),
            json!({"type": "assistant", "uuid": "a-1", "timestamp": "2026-11-03T06:00:05Z",
                   "message": {"role": "assistant", "content": [{"type": "text", "text": SPOKEN}]}}),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(projects.join(format!("{SESSION}.jsonl")), text).unwrap();
        let env = AppEnv {
            root: None,
            home: Some(home.into_os_string()),
            claude_config_dir: None,
        };
        let ctx = Context::resolve(Some(&dir.path().join("root")), &env).unwrap();
        Self { dir, ctx }
    }

    fn input(&self, prompt: &str) -> Vec<u8> {
        let transcript = self
            .dir
            .path()
            .join(format!("home/.claude/projects/-work/{SESSION}.jsonl"));
        json!({
            "session_id": SESSION,
            "transcript_path": transcript,
            "cwd": self.dir.path().join("work"),
            "prompt": prompt,
        })
        .to_string()
        .into_bytes()
    }

    /// Run the hook with `answer` as the model's: the reply text and the
    /// number of model calls.
    fn run(&self, prompt: &str, answer: Result<ReferentAnswer, ModelError>) -> (String, usize) {
        let models = FakeModels {
            answer,
            calls: Cell::new(0),
        };
        let output = run_with(&self.input(prompt), &self.ctx, |_| &models).unwrap();
        let Output::Hook(reply) = output else {
            panic!("a hook reply");
        };
        (reply.message.unwrap_or_default(), models.calls.get())
    }

    fn entries(&self) -> Vec<scribe_core::Entry> {
        self.ctx
            .store()
            .list_entries(&EntryFilter::default())
            .unwrap()
            .listing
            .items
    }
}

impl Models for &FakeModels {
    fn referent(&self, request: &ReferentRequest) -> Result<ReferentAnswer, ModelError> {
        (*self).referent(request)
    }
    fn session(&self, request: &SessionRequest) -> Result<SessionAnswer, ModelError> {
        (*self).session(request)
    }
}

fn span(message: &str, span: &str) -> Result<ReferentAnswer, ModelError> {
    Ok(ReferentAnswer {
        message: message.to_owned(),
        span: span.to_owned(),
    })
}

#[test]
fn agent_originated_prompts_record_nothing() {
    let wrappers = [
        "<agent-message from=\"other\">\n#scribe decisions: agent wrote this\n</agent-message>",
        "<task-notification>\n#scribe decisions: agent wrote this\n</task-notification>",
        "<side-sessions-event>\n#scribe decisions: agent wrote this\n</side-sessions-event>",
        "<scheduled-loop>\n#scribe decisions: agent wrote this\n</scheduled-loop>",
        "<scheduled-prompt>\n#scribe forget decisions-1\n</scheduled-prompt>",
    ];
    for prompt in wrappers {
        let setup = Setup::new();
        let (echo, calls) = setup.run(
            prompt,
            Err(ModelError::Failed(String::from("must not be called"))),
        );
        assert_eq!(echo, "", "wrapper {prompt:?}");
        assert_eq!(calls, 0, "wrapper {prompt:?}");
        assert!(setup.entries().is_empty(), "wrapper {prompt:?}");
    }
}

#[test]
fn add_this_records_the_chosen_span_citing_its_role() {
    let setup = Setup::new();
    let (echo, calls) = setup.run(
        "#scribe add this to decisions",
        span(
            "a-1",
            "squash-merge agent PRs and keep merge commits for releases",
        ),
    );
    assert_eq!(calls, 1);
    assert_eq!(
        echo,
        "scribe: added decisions-1 to decisions — \"squash-merge agent PRs and keep merge commits for releases\""
    );
    let entries = setup.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].said, "#scribe add this to decisions");
    assert_eq!(
        entries[0].content,
        "squash-merge agent PRs and keep merge commits for releases"
    );
    assert!(matches!(
        &entries[0].content_from,
        ContentFrom::Message { role: Speaker::Assistant, message, .. } if message == "a-1"
    ));

    // A replay echoes the entry without asking the model again.
    let (echo, calls) = setup.run(
        "#scribe add this to decisions",
        Err(ModelError::Failed(String::from("must not be called"))),
    );
    assert_eq!(calls, 0);
    assert!(
        echo.starts_with("scribe: already recorded decisions-1"),
        "{echo}"
    );
    assert_eq!(setup.entries().len(), 1);
}

#[test]
fn a_timeout_a_model_error_and_a_failed_check_each_write_one_guessed_entry() {
    let cases = [
        (
            Err(ModelError::Timeout {
                budget: Duration::from_secs(20),
            }),
            "did not answer within 20 s",
        ),
        (
            Err(ModelError::Failed(String::from(
                "claude ended with exit status: 1",
            ))),
            "the model failed",
        ),
        (
            span("a-1", "we always rebase agent PRs"),
            "not word for word",
        ),
    ];
    for (answer, reason) in cases {
        let setup = Setup::new();
        let (echo, calls) = setup.run("#scribe add it to my decisions", answer);
        assert_eq!(calls, 1);
        let entries = setup.entries();
        assert_eq!(entries.len(), 1, "exactly one entry: {echo}");
        assert_eq!(entries[0].content, SPOKEN, "the previous assistant message");
        assert!(matches!(
            &entries[0].content_from,
            ContentFrom::Message { role: Speaker::Assistant, message, .. } if message == "a-1"
        ));
        assert!(
            echo.starts_with("scribe: added decisions-1 to decisions (guessed: "),
            "{echo}"
        );
        assert!(echo.contains(reason), "{reason}: {echo}");
        assert!(
            echo.contains("recorded the assistant message before your prompt instead"),
            "{echo}"
        );
    }
}
