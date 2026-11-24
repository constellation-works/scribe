//! Audit middleware: one audit-log line per mutating command.
//!
//! [`AuditGuard`] wraps a command's execution at the one chokepoint,
//! `commands::run` (STD-02 R24). It is created before dispatch, told the
//! outcome with [`AuditGuard::mark_result`], and writes the line when it is
//! dropped. It starts out recording a failure, so a command that returns early
//! or panics is logged as one; an outcome is never assumed to be success
//! (STD-02 R29).
//!
//! Which commands are audited is declared per command in
//! `commands::Command::audit`; read-only commands are not, because they must
//! not write (STD-01 R31). The line holds the command path, the record acted
//! on, the outcome, the stable error code, for a command that takes an
//! `--author`, that name (STD-05 R2), and for a write run on the user's
//! instruction its session and turn ids: never argument values or error text,
//! which can carry entry contents or paths (STD-05 R13).
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
//! as a warning on stderr. `scripts/check-dependency-direction.sh` rejects
//! `crate::cli` and `crate::commands` imports here: dispatch depends on the
//! middleware, never the reverse.

use crate::error::CliError;
use crate::output::{Output, Record};
use scribe_core::{AuditEvent, AuditStatus, Store};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use time::OffsetDateTime;

/// How a command appears in the audit log, and how it commits. Declared by
/// the commands that write: the hooks, `session pass` and `consistency
/// scan` (also when the plugin's routine tools run them), and the direct
/// write verbs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuditMeta {
    /// The command path, such as `hook user-prompt-submit`.
    pub(crate) command: &'static str,
    /// Whether the command commits its own steps: one that may call a
    /// model, because no transaction may be held across a model call. Any
    /// other runs as one transaction with its audit lines (`commands::run`,
    /// 2_design.md §15.7).
    pub(crate) stepwise: bool,
}

/// One line the guard will write: how it ended, the record, the code.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    status: AuditStatus,
    target: Option<String>,
    error_code: Option<&'static str>,
}

/// What a write verb names before it runs, so a refused write is still
/// attributable: the record it targets, resolved to its id, and the
/// `--author` it acted as. Ids and names only, never argument values
/// (STD-05 R2, R13). `None` where the command names neither.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AuditSubject {
    /// The resolved id of the record, or `None` when it does not resolve.
    pub(crate) target: Option<String>,
    /// The agent's `--author` name, or `None` for a command without one.
    pub(crate) author: Option<String>,
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
    instruction: crate::app::InstructionNote,
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
            instruction: crate::app::InstructionNote::default(),
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

    /// Record how the command ended: the record it acted on on success, the
    /// stable error code on failure (STD-01 R19), and for a hook one line
    /// per write or refusal.
    pub(crate) fn mark_result(&mut self, result: &Result<Output, CliError>) {
        if self.committed_in_write() {
            self.lines.clear();
            return;
        }
        self.lines = match result {
            Ok(Output::Init(report)) if !report.changed => Vec::new(),
            Ok(Output::Written { changed: false, .. }) => Vec::new(),
            Ok(Output::GroupBackfill(report)) if report.stored == 0 => Vec::new(),
            Ok(Output::Hook(reply)) => reply
                .outcomes
                .iter()
                .map(|outcome| Line {
                    status: if outcome.error_code.is_some() {
                        AuditStatus::Failure
                    } else {
                        AuditStatus::Success
                    },
                    target: outcome.target.clone(),
                    error_code: outcome.error_code,
                })
                .collect(),
            Ok(Output::Pass(report)) => {
                let mut lines: Vec<Line> = report
                    .passes
                    .iter()
                    .filter(|p| p.result != "waiting")
                    .map(|p| Line {
                        status: if p.error_code.is_some() {
                            AuditStatus::Failure
                        } else {
                            AuditStatus::Success
                        },
                        target: Some(format!("session:{}", p.session)),
                        error_code: p.error_code,
                    })
                    .collect();
                // A catalog that wrote its switch logs the switch as one more
                // success line, but only when no window failed: a failed
                // catalog is one failure outcome, whose line names the
                // session whose switch it left due.
                let failed = lines.iter().any(|line| line.status == AuditStatus::Failure);
                if report.switch_written && !failed {
                    let target = report
                        .passes
                        .first()
                        .map(|p| format!("session:{}", p.session));
                    lines.insert(
                        0,
                        Line {
                            status: AuditStatus::Success,
                            target,
                            error_code: None,
                        },
                    );
                }
                for id in report
                    .persisted_targets
                    .iter()
                    .chain(&report.memory_targets)
                {
                    lines.push(Line {
                        status: AuditStatus::Success,
                        target: Some(id.clone()),
                        error_code: None,
                    });
                }
                lines
            }
            Ok(Output::Scan(report)) => report
                .reapplied
                .iter()
                .map(|r| Line {
                    status: AuditStatus::Success,
                    target: Some(r.entry.clone()),
                    error_code: None,
                })
                .chain(report.opened.iter().map(|p| Line {
                    status: AuditStatus::Success,
                    target: Some(p.id.clone()),
                    error_code: None,
                }))
                .chain(report.closed.iter().map(|p| Line {
                    status: AuditStatus::Success,
                    target: Some(p.id.clone()),
                    error_code: None,
                }))
                .chain(report.covered.iter().map(|item| Line {
                    status: AuditStatus::Success,
                    target: Some(item.proposal.clone()),
                    error_code: None,
                }))
                .chain(report.deactivated.iter().map(|d| Line {
                    status: AuditStatus::Success,
                    target: Some(format!("session:{}", d.session)),
                    error_code: None,
                }))
                .chain(report.failed_deactivations.iter().map(|f| Line {
                    status: AuditStatus::Failure,
                    target: Some(format!("session:{}", f.session)),
                    error_code: Some(f.error_code),
                }))
                .chain(report.memory.targets.iter().map(|target| Line {
                    status: AuditStatus::Success,
                    target: Some(target.clone()),
                    error_code: None,
                }))
                .chain(report.memory.failed_batches.iter().map(|f| Line {
                    status: AuditStatus::Failure,
                    target: Some(format!("memory-batch:{}", f.batch)),
                    error_code: Some(f.error_code),
                }))
                .collect(),
            Ok(output) => vec![Line {
                status: AuditStatus::Success,
                target: target_of(output),
                error_code: None,
            }],
            Err(err) => vec![Line {
                status: AuditStatus::Failure,
                target: None,
                error_code: Some(err.code()),
            }],
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
    pub(crate) fn set_instruction(&mut self, note: crate::app::InstructionNote) {
        self.instruction = note;
    }

    /// Write the lines now, inside the command's transaction, so they commit
    /// with its writes; the guard then writes nothing when dropped unless
    /// [`AuditGuard::mark_result`] is called again (the commit failed).
    ///
    /// # Errors
    ///
    /// A store error writing a line, which fails the transaction.
    pub(crate) fn flush(&mut self) -> Result<(), scribe_core::Error> {
        for event in self.events() {
            self.store.read().audit().record(&event)?;
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
            if let Err(err) = self.store.read().audit().record(&event) {
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

/// The id of the record a successful command acted on, if it names one.
/// Exhaustive, so a new payload decides what it contributes (STD-02 R27).
fn target_of(output: &Output) -> Option<String> {
    match output {
        Output::Init(report) => Some(report.provider.to_string()),
        Output::Written { record, .. } => record_target(record),
        Output::GroupBackfill(report) => Some(report.group.clone()),
        Output::Show {
            record: Record::Group(g),
        } => Some(g.id.clone()),
        Output::Show {
            record: Record::Tag(tag),
        } => Some(tag.name.to_string()),
        Output::Show {
            record: Record::TagRules(tag),
        } => Some(tag.tag.clone()),
        Output::Show {
            record: Record::Proposal(proposal),
        } => Some(proposal.id.clone()),
        Output::Show {
            record: Record::Entry(entry),
        } => Some(entry.id.clone()),
        Output::Show {
            record: Record::Session(session),
        } => Some(session.id.clone()),
        Output::Show {
            record: Record::SessionHandoffs(session),
        } => Some(session.id.clone()),
        Output::Show {
            record: Record::Transcript(transcript),
        } => Some(format!("session:{}", transcript.session)),
        Output::Show {
            record: Record::Memory(item),
        } => Some(format!("memory:{}", item.id)),
        Output::Show {
            record:
                Record::MemoryBlock(_)
                | Record::MemoryRevision(_)
                | Record::MemoryRefresh(_)
                | Record::MemoryRules(_)
                | Record::StoreMigrate(_)
                | Record::Export(_)
                | Record::Brief(_),
        }
        | Output::List { .. } => None,
        // Hook replies, passes and scans are audited per outcome in
        // `mark_result`.
        Output::Hook(_) | Output::Pass(_) | Output::Scan(_) => None,
        // Read-only: never audited (`plugin call`).
        Output::Plugin(_) => None,
    }
}

fn record_target(record: &Record) -> Option<String> {
    Some(match record {
        Record::Group(g) => g.id.clone(),
        Record::Memory(m) => format!("memory:{}", m.id),
        Record::MemoryRefresh(_) => String::from("memory:profile"),
        Record::StoreMigrate(_) => String::from("store:records"),
        Record::Export(_) => return None,
        Record::MemoryBlock(_)
        | Record::MemoryRevision(_)
        | Record::MemoryRules(_)
        | Record::Brief(_) => return None,
        Record::Entry(e) => e.id.clone(),
        Record::Session(s) => format!("session:{}", s.id),
        Record::SessionHandoffs(s) => format!("session:{}", s.id),
        Record::Tag(t) => t.name.to_string(),
        Record::TagRules(t) => t.tag.clone(),
        Record::Proposal(p) => p.id.clone(),
        Record::Transcript(t) => format!("session:{}", t.session),
    })
}
