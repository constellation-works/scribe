//! The audit guard around a mutating command (STD-02 R24), the lines a
//! multi-step use case owes for writes already on disk when it returns
//! early, and the outcome a hook run records per write or refusal.
//!
//! [`AuditGuard`] wraps an app's `exec` ([`guarded`]). It is created before
//! the command runs, told the outcome with [`AuditGuard::mark_result`], and
//! writes the lines when it is dropped. It starts out recording a failure, so
//! a command that returns early or panics is logged as one; an outcome is
//! never assumed to be success (STD-02 R29).
//!
//! Which commands are audited is declared by each app, beside its
//! `Command` (`Command::audit` and `Command::subject`, built into an
//! [`Audit`] by its `exec`); read-only commands are not, because they must not write
//! (STD-01 R31). The line holds the command path, the record acted on, the
//! outcome, the stable error code, for a command that takes an `--author`,
//! that name (STD-05 R2), and for a write run on the user's instruction its
//! session and turn ids: never argument values or error text, which can carry
//! entry contents or paths (STD-05 R13).
//!
//! A hook run writes one line per outcome it reports: one per record it wrote
//! or refused (two `#scribe` lines in one prompt are two lines in the log);
//! one per `/scribe` switch it changed (`on`, `off`, a link, `catalog` or
//! `continue`, `target` `session:<id>`), and a refused `/scribe` command is one
//! failure line; one more per run that stores a group member's new typed turns
//! (`target` `session:<id>`, no error code, content-free, one per run however
//! many turns it stores); one per deferred ancestry check that a session start
//! records, and one per typed prompt that consumes one (`target` `session:<id>`,
//! no error code, whatever the prompt holds), and one failure line with its
//! code for a check that could not be recorded or consumed. A run that only
//! moves a group
//! member's capture cursor writes none, and so does a no-op switch or a status;
//! a session pass one per
//! session passed (a `session catalog`
//! also one for the switch it wrote, unless a window failed: a failed catalog is one
//! failure line) and one per retired-tag candidate closed; a consistency scan one
//! per entry re-applied, per proposal opened or closed, per session deactivated or
//! deactivation failed, per batch the model failed, and none when it changed
//! nothing; a direct write verb (`entry`, `tag` and `proposal` writes,
//! `session activate|deactivate`) one when it changed the record and none
//! when it did not: the `Stop` hook runs after
//! every reply and usually has nothing to do. A prompt with no `#scribe` line
//! adds a line only when it changes a `/scribe` switch, is a refused `/scribe`
//! command, stores a group member's turn, or consumes a deferred ancestry check.
//!
//! A command that calls no model runs, with its lines, as one `scribe.db`
//! transaction: [`AuditGuard::flush`] writes them inside it, so a write and
//! its line commit together or not at all. A refused or failed command's
//! transaction rolls back, and its failure line is written after, without
//! waiting: spooled to a file of its own when another writer holds the
//! database, so a locked store costs one wait and loses no line.
//!
//! The audit log is a side channel, so it fails open (STD-02 R31): a line that
//! cannot be written never fails the command that already ran, and is reported
//! as a warning on stderr.

use crate::composition::{Context, InstructionNote};
use crate::runtime::errors::CommandError;
use crate::{AuditEvent, AuditStatus, Store};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use time::OffsetDateTime;

/// How a command appears in the audit log, and how it commits. Declared by
/// each app's `Command::audit` for the commands that write: the hooks,
/// `session pass` and `consistency scan` (also when the plugin's routine
/// tools run them), and the direct write verbs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditMeta {
    /// The command path, such as `hook user-prompt-submit`.
    pub command: &'static str,
    /// Whether the command commits its own steps: one that may call a
    /// model, because no transaction may be held across a model call. Any
    /// other runs as one transaction with its audit lines ([`guarded`],
    /// 2_design.md §15.7).
    pub stepwise: bool,
}

/// What a write verb names before it runs, so a refused write is still
/// attributable: the record it targets, resolved to its id, and the
/// `--author` it acted as. Ids and names only, never argument values
/// (STD-05 R2, R13). `None` where the command names neither.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditSubject {
    /// The resolved id of the record, or `None` when it does not resolve.
    pub target: Option<String>,
    /// The agent's `--author` name, or `None` for a command without one.
    pub author: Option<String>,
}

impl AuditSubject {
    /// Apply the subject to a guard, which keeps it for every line it writes.
    pub(crate) fn apply(&self, audit: &mut AuditGuard<'_>) {
        if let Some(target) = &self.target {
            audit.set_target(target);
        }
        if let Some(author) = &self.author {
            audit.set_author(author);
        }
    }
}

/// How an app's `exec` audits a mutating command: its path and commit mode,
/// and the subject resolved before it ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Audit {
    /// The command path and how it commits.
    pub meta: AuditMeta,
    /// The record and agent it names.
    pub subject: AuditSubject,
}

impl Audit {
    /// The audit a command declares: `meta` from its `audit`, with the
    /// subject `subject` resolves, or `None` when it is not audited. Only
    /// an audited command resolves its subject.
    pub(crate) fn declared(
        meta: Option<AuditMeta>,
        subject: impl FnOnce() -> AuditSubject,
    ) -> Option<Self> {
        meta.map(|meta| Self {
            meta,
            subject: subject(),
        })
    }
}

/// One line the guard will write: how it ended, the record, the code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) status: AuditStatus,
    pub(crate) target: Option<String>,
    pub(crate) error_code: Option<&'static str>,
}

impl Line {
    /// A success naming `target`.
    pub(crate) fn success(target: Option<String>) -> Self {
        Self {
            status: AuditStatus::Success,
            target,
            error_code: None,
        }
    }

    /// A failure naming `target`, with its stable code.
    pub(crate) fn failure(target: Option<String>, code: &'static str) -> Self {
        Self {
            status: AuditStatus::Failure,
            target,
            error_code: Some(code),
        }
    }

    /// One line for `changed` naming `target`, and none when nothing changed.
    pub(crate) fn written(target: Option<String>, changed: bool) -> Vec<Self> {
        if changed {
            vec![Self::success(target)]
        } else {
            Vec::new()
        }
    }
}

/// The lines an app's outcome gives: the record it acted on, or one per
/// write or refusal for the outcomes that report several.
pub(crate) trait Audited {
    /// The lines, in the order they are written.
    fn lines(&self) -> Vec<Line>;
}

/// Run `run` as an app's `exec` runs a command: inside the audit guard when
/// `audit` names one, and for a command that is not stepwise as one store
/// transaction with its lines (they commit together, and a refusal rolls
/// back every write it made before refusing).
///
/// The guard exists before the command runs and records when it is dropped,
/// so a command that returns early or panics is still logged, as a failure.
pub(crate) fn guarded<O: Audited>(
    ctx: &Context,
    audit: Option<Audit>,
    run: impl FnOnce() -> Result<O, CommandError>,
) -> Result<O, CommandError> {
    let Some(Audit { meta, subject }) = audit else {
        return run();
    };
    let mut guard = AuditGuard::start(ctx.store(), meta, ctx.now());
    guard.set_committed_flag(ctx.audit_committed_flag());
    subject.apply(&mut guard);
    let finish = |guard: &mut AuditGuard<'_>, result: &Result<O, CommandError>| {
        guard.mark_result(result);
        subject.apply(guard);
        guard.set_instruction(ctx.instruction_note());
    };
    if meta.stepwise {
        let result = run();
        finish(&mut guard, &result);
        return result;
    }
    let result = ctx.store().transaction(|| {
        let result = run();
        finish(&mut guard, &result);
        let outcome = result?;
        guard.flush()?;
        Ok(outcome)
    });
    if result.is_err() {
        // The lines flushed inside rolled back with the transaction: the
        // guard writes the failure after it.
        finish(&mut guard, &result);
    }
    result
}

/// Records one command in the audit log when dropped.
#[must_use = "the audit line is written when the guard is dropped"]
#[derive(Debug)]
pub(crate) struct AuditGuard<'a> {
    store: &'a Store,
    meta: AuditMeta,
    at: OffsetDateTime,
    started: Instant,
    lines: Vec<Line>,
    author: Option<String>,
    instruction: InstructionNote,
    committed: Option<Arc<AtomicBool>>,
}

impl<'a> AuditGuard<'a> {
    /// Start auditing a command that began at `at`. Until
    /// [`AuditGuard::mark_result`] is called, the command reads as failed.
    pub(crate) fn start(store: &'a Store, meta: AuditMeta, at: OffsetDateTime) -> Self {
        Self {
            store,
            meta,
            at,
            started: Instant::now(),
            lines: vec![Line {
                status: AuditStatus::Failure,
                target: None,
                error_code: None,
            }],
            author: None,
            instruction: InstructionNote::default(),
            committed: None,
        }
    }

    /// Attach a shared flag set when a stepwise write commits its own success
    /// audit row inside its store transaction. The committed row is final
    /// even if the command returns a post-commit error.
    pub(crate) fn set_committed_flag(&mut self, committed: Arc<AtomicBool>) {
        self.committed = Some(committed);
    }

    fn committed_in_write(&self) -> bool {
        self.committed
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    }

    /// Record how the command ended: the lines its outcome gives on success,
    /// the stable error code on failure (STD-01 R19).
    pub(crate) fn mark_result<O: Audited>(&mut self, result: &Result<O, CommandError>) {
        if self.committed_in_write() {
            self.lines.clear();
            return;
        }
        self.lines = match result {
            Ok(outcome) => outcome.lines(),
            Err(err) => vec![Line::failure(None, err.code())],
        };
    }

    /// Name the known target even when a command refuses before a payload exists.
    pub(crate) fn set_target(&mut self, target: &str) {
        for line in &mut self.lines {
            line.target = Some(target.to_owned());
        }
    }

    /// Name the agent a command acted as, on every line it writes: the
    /// `--author` name, a self-reported identity kept in its own field (STD-05 R2).
    pub(crate) fn set_author(&mut self, author: &str) {
        self.author = Some(author.to_owned());
    }

    /// Name the instruction a write ran on, by its session and turn ids
    /// only (2_design.md §12.4), on every line it writes.
    pub(crate) fn set_instruction(&mut self, note: InstructionNote) {
        self.instruction = note;
    }

    /// Write the lines now, inside the command's transaction, so they commit
    /// with its writes; the guard then writes nothing when dropped unless
    /// [`AuditGuard::mark_result`] is called again (the commit failed).
    ///
    /// # Errors
    ///
    /// A store error writing a line, which fails the transaction.
    pub(crate) fn flush(&mut self) -> Result<(), crate::Error> {
        for event in self.events() {
            self.store.record_audit(&event)?;
        }
        self.lines.clear();
        Ok(())
    }

    /// The lines this guard will write, in order.
    pub(crate) fn events(&self) -> Vec<AuditEvent> {
        let duration_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.lines
            .iter()
            .map(|line| AuditEvent {
                at: self.at,
                command: self.meta.command.to_owned(),
                status: line.status,
                target: line.target.clone(),
                error_code: line.error_code.map(str::to_owned),
                author: self.author.clone(),
                instruction_session: self.instruction.session.clone(),
                instruction_message: self.instruction.message.clone(),
                issue: None,
                duration_ms,
            })
            .collect()
    }
}

impl Drop for AuditGuard<'_> {
    fn drop(&mut self) {
        if self.committed_in_write() {
            return;
        }
        // Fail open, and never panic in a destructor (STD-03 R4).
        for event in self.events() {
            if let Err(err) = self.store.record_audit(&event) {
                tracing::warn!(
                    command = self.meta.command,
                    error = %err,
                    "could not write the audit log"
                );
                return;
            }
        }
    }
}

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
            if let Err(err) = self.store.record_audit(&event) {
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
