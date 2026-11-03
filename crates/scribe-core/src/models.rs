//! The `Models` port: every model call scribe makes, as typed requests and
//! answers (docs/design/scribe/2_design.md §8).
//!
//! The port is a trait so the domain never knows how a model is reached.
//! The CLI crate implements it with `ClaudeCliModels`, which runs the
//! `claude` CLI; tests implement it with a fake that answers from a script.
//! Each request carries the model id from `config.toml` and the use's budget,
//! and renders its own prompt and JSON schema here, so what a model is asked
//! is pure text a test can read.
//!
//! The only use built so far is the referent of "this" or "it" in a form-3
//! `#scribe` line ([`ReferentRequest`]); the session pass, consistency
//! judgement, `ask` and distilling join with their slices.
//!
//! No I/O (STD-02 R5).

use crate::entry::Speaker;
use std::fmt::Write as _;
use std::time::Duration;

/// How long the referent model may take before the fallback is used
/// (2_design.md §4.3).
pub const REFERENT_BUDGET: Duration = Duration::from_secs(20);

/// The most characters of one message a prompt carries. A longer message is
/// cut and the cut is marked (STD-03 R15); the span the model returns is
/// still checked against the whole message.
pub const PROMPT_MESSAGE_CHARS: usize = 8_000;

/// The marker that ends a message cut to [`PROMPT_MESSAGE_CHARS`].
pub const CUT_MARK: &str = "[… cut]";

/// The JSON schema of a referent answer.
pub const REFERENT_SCHEMA: &str = r#"{"type":"object","properties":{"message":{"type":"string","description":"the id of the message the instruction points at"},"span":{"type":"string","description":"the exact passage of that message to record, copied character for character"}},"required":["message","span"],"additionalProperties":false}"#;

/// A model call failed; the caller falls back (2_design.md §4.3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// The model did not answer within its budget; the process was killed.
    #[error("the model did not answer within {} s", budget.as_secs())]
    Timeout {
        /// The budget it exceeded.
        budget: Duration,
    },
    /// The model process could not be started (no `claude` on `PATH`).
    #[error("the model could not be started: {0}")]
    Unavailable(String),
    /// The model process ended without an answer: a non-zero exit, a
    /// signal, or an error result.
    #[error("the model failed: {0}")]
    Failed(String),
    /// The model answered, but not in the requested shape.
    #[error("the model's answer was not usable: {0}")]
    Malformed(String),
}

/// One message the referent may point at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The transcript message's uuid: what the model answers with.
    pub uuid: String,
    /// Whose words they are.
    pub role: Speaker,
    /// The message's whole text.
    pub text: String,
}

/// Which earlier message a form-3 instruction points at, and the passage
/// to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferentRequest {
    /// The model id (`models.referent`).
    pub model: String,
    /// How long the call may take.
    pub budget: Duration,
    /// The `#scribe` line as typed.
    pub instruction: String,
    /// The messages before the prompt, oldest first.
    pub candidates: Vec<Candidate>,
}

impl ReferentRequest {
    /// The prompt the model reads on stdin.
    #[must_use]
    pub fn prompt(&self) -> String {
        let mut out = String::from(
            "You resolve what \"this\", \"that\", \"it\" or \"these\" refers to in a note-taking \
             instruction typed in a coding session. Below are the most recent messages of the \
             conversation, oldest first, each with its id, followed by the instruction. Pick the \
             one message the instruction points at (usually the latest one that states what the \
             user wants recorded) and the exact passage of it to record: a decision, preference \
             or fact stated in that message, copied character for character, without adding, \
             rewording or summarizing anything. Prefer a complete sentence or a few sentences \
             over the whole message. Answer with JSON: {\"message\": \"<id>\", \"span\": \
             \"<exact passage>\"}.\n",
        );
        for candidate in &self.candidates {
            let _ = write!(
                out,
                "\n<message id=\"{}\" role=\"{}\">\n{}\n</message>\n",
                candidate.uuid,
                candidate.role.as_str(),
                bounded(&candidate.text, PROMPT_MESSAGE_CHARS)
            );
        }
        let _ = write!(
            out,
            "\n<instruction>\n{}\n</instruction>\n",
            self.instruction
        );
        out
    }
}

/// `text` cut to `max` characters, with [`CUT_MARK`] when it was cut.
fn bounded(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push_str(CUT_MARK);
    cut
}

/// The model's answer to a [`ReferentRequest`], unchecked: the caller runs
/// it through the verbatim check before trusting it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferentAnswer {
    /// The uuid of the message it chose.
    pub message: String,
    /// The passage it chose.
    pub span: String,
}

/// Every model call scribe makes. One adapter reaches the real models; tests
/// substitute a fake.
pub trait Models {
    /// Resolve a form-3 referent.
    ///
    /// # Errors
    ///
    /// A [`ModelError`] when the model times out, cannot be started, fails
    /// or answers in another shape.
    fn referent(&self, request: &ReferentRequest) -> Result<ReferentAnswer, ModelError>;
}
