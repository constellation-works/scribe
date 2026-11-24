//! The audit lines a multi-step use case owes for writes already on disk
//! when it returns early, and the outcome a hook run records per write or
//! refusal.

use crate::{AuditEvent, AuditStatus, Store};
use time::OffsetDateTime;

/// Durable writes already on disk when a multi-step command returns `Err`
/// before reaching `AuditGuard::mark_result`. Disarm right before returning
/// `Ok` so the middleware writes those lines with the command's real duration;
/// on an early `?` return or unwind, `Drop` writes one success line per target
/// (`duration_ms: 0`) and fails open (STD-02 R31, STD-03 R4).
pub struct PersistedAudit<'a> {
    store: &'a Store,
    command: &'static str,
    author: Option<String>,
    at: OffsetDateTime,
    targets: Vec<String>,
    armed: bool,
}

impl<'a> PersistedAudit<'a> {
    /// Armed, with no target yet: `command`'s lines are dated `at`.
    #[must_use]
    pub fn new(store: &'a Store, command: &'static str, at: OffsetDateTime) -> Self {
        Self {
            store,
            command,
            author: None,
            at,
            targets: Vec::new(),
            armed: true,
        }
    }

    /// Lines naming `author`.
    #[must_use]
    pub fn with_author(mut self, author: &str) -> Self {
        self.author = Some(author.to_owned());
        self
    }

    /// One more target already persisted.
    pub fn record(&mut self, target: impl Into<String>) {
        self.targets.push(target.into());
    }

    /// More targets already persisted.
    pub fn extend(&mut self, targets: impl IntoIterator<Item = String>) {
        self.targets.extend(targets);
    }

    /// Write no line on drop: the command reached its own audit.
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PersistedAudit<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for target in &self.targets {
            let event = AuditEvent {
                at: self.at,
                command: self.command.to_owned(),
                status: AuditStatus::Success,
                target: Some(target.clone()),
                error_code: None,
                author: self.author.clone(),
                instruction_session: None,
                instruction_message: None,
                issue: None,
                duration_ms: 0,
            };
            if let Err(err) = self.store.read().audit().record(&event) {
                tracing::warn!(
                    command = self.command,
                    %err,
                    target = %target,
                    "cannot audit a write already persisted"
                );
            }
        }
    }
}

/// One write or refusal a hook run made: what the audit guard writes one
/// line for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookOutcome {
    /// The record it acted on: an entry id, or `session:<id>` for a refusal.
    pub target: Option<String>,
    /// The stable code of a refusal; `None` for a write.
    pub error_code: Option<&'static str>,
}
