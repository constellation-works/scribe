//! Providers (interim): the [`Models`] port, its [`ModelError`], and the
//! adapters that reach a model through the `claude` and `codex` CLIs
//! (docs/design/scribe/specs/architecture.md, "Providers (interim)").
//!
//! Scribe's own model calls are being removed (ORB-27911). Until an Orbit job
//! replaces them, five flows call a model through this port: session pass,
//! catalog and rebuild, memory refresh and group close. No other flow names
//! it, and only this module names `std::process`
//! (scripts/check-dependency-direction.sh).
//!
//! The request and answer shapes the port carries ([`crate::models`]) stay in
//! `scribe-domain` for now: `scribe-store` and the domain use them, and both
//! sit below the core. They move when the domain is dissolved (ORB-27921
//! step 5).

mod cli;
pub mod models;

#[cfg(test)]
pub(crate) use cli::{BASELINE_ENV, argv};
pub use cli::{ClaudeCliModels, allowed_env};

use crate::models::{
    CloseEvidence, ProfileAnswer, ProfileRequest, SessionAnswer, SessionHeader,
    SessionHeaderRequest, SessionRequest,
};
use std::time::Duration;

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

impl ModelError {
    /// A stable `snake_case` code for the audit log (STD-01 R19).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Timeout { .. } => "model_timeout",
            Self::Unavailable(_) => "model_unavailable",
            Self::Failed(_) => "model_failed",
            Self::Malformed(_) => "model_malformed",
        }
    }
}

/// Every model call scribe makes. One adapter reaches the real models; tests
/// substitute a fake.
pub trait Models {
    /// Write a session record from a session's messages.
    ///
    /// # Errors
    ///
    /// A [`ModelError`] when the model times out, cannot be started, fails
    /// or answers in another shape.
    fn session(&self, request: &SessionRequest) -> Result<SessionAnswer, ModelError>;

    /// Compress a session header without resending its transcript window.
    /// Adapters serving session passes should implement this retry; the
    /// default reports that compression is unavailable.
    ///
    /// # Errors
    ///
    /// A [`ModelError`] when the model times out, cannot be started, fails
    /// or answers in another shape.
    fn session_header(&self, request: &SessionHeaderRequest) -> Result<SessionHeader, ModelError> {
        let _ = request;
        Err(ModelError::Unavailable(String::from(
            "this adapter does not compress session headers",
        )))
    }

    /// Summarize a group's bounded, stored evidence in one model call.
    fn group_summary(&self, model: &str, evidence: &CloseEvidence) -> Result<String, ModelError> {
        let _ = (model, evidence);
        Err(ModelError::Unavailable(
            "this adapter does not summarize groups".into(),
        ))
    }

    /// Rebuild the profile for `memory refresh` (2_design.md §15.6).
    ///
    /// The default answers [`ModelError::Unavailable`], so an adapter that
    /// serves only other uses need not implement it; `ClaudeCliModels` does.
    ///
    /// # Errors
    ///
    /// A [`ModelError`] when the model times out, cannot be started, fails
    /// or answers in another shape.
    fn profile(&self, request: &ProfileRequest) -> Result<ProfileAnswer, ModelError> {
        let _ = request;
        Err(ModelError::Unavailable(String::from(
            "this adapter does not rebuild the profile",
        )))
    }
}

#[cfg(test)]
mod tests;
