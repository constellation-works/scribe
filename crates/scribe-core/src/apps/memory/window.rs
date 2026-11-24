//! A pass window's memory: the drafts a session pass hands the memory, and
//! the plan the store runs inside the window's transaction
//! ([`crate::repository::Sessions::commit_window`]).

use crate::memory::{Draft, PassChange, Refusal, plan};
use crate::repository::{MemoryCommit, MemoryFiles, MemoryRows, PendingRow};
use crate::{MILESTONES_DIR, MemoryStep, MemoryWritten, NOW_DIR, Result, SessionId, SessionRecord};

/// The memory a pass window writes with its record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowMemory {
    /// The checked elements, tied to their turns.
    pub drafts: Vec<Draft>,
    /// The window's signal: its last message id and 0–3.
    pub signal: Option<(String, u8)>,
    /// The session's last pass: its pending candidates are dropped.
    pub final_pass: bool,
}

/// What the window's write does with `memory` for the session `record`
/// (§15.5): nothing without both; with nothing to plan, only a journal a
/// crash left open is applied; otherwise the drafts (already checked
/// against the redactor, `sensitive` the refusals that check made,
/// reported first) are planned against the current files and this
/// session's pending candidates ([`plan`]) under the store's lock. A file
/// whose content would not change is not written, so a replayed window
/// writes nothing. The final pass drops the session's pending candidates.
pub fn plan_window<'a>(
    memory: Option<&'a WindowMemory>,
    sensitive: &'a [Refusal],
    record: Option<&'a SessionRecord>,
) -> MemoryStep<
    impl FnOnce(&MemoryRows<'_>, &MemoryFiles) -> Result<(MemoryCommit, MemoryWritten)> + 'a,
> {
    let (Some(pass), Some(record)) = (memory, record) else {
        return MemoryStep::Skip;
    };
    if pass.drafts.is_empty() && sensitive.is_empty() && pass.signal.is_none() && !pass.final_pass {
        return MemoryStep::Replay;
    }
    MemoryStep::Plan(move |rows: &MemoryRows<'_>, files: &MemoryFiles| {
        let session = record.id.clone();
        let mut written = MemoryWritten {
            refused: sensitive.to_vec(),
            ..MemoryWritten::default()
        };
        let drafts = &pass.drafts;
        let pending = rows.pending(&session)?;
        let (changes, refused) = plan(drafts, &files.now, &files.milestones, &pending, &session);
        written.refused.extend(refused);
        let mut commit = MemoryCommit::default();
        for change in changes {
            match change {
                PassChange::Now(item, cites) => {
                    link(&mut commit, item.stem.as_str(), &cites);
                    if !files.now.contains(&item) {
                        written
                            .written
                            .push(format!("memory:{NOW_DIR}/{}", item.stem));
                        commit.ops.push(rows.now_op(&item)?);
                    }
                }
                PassChange::Milestone(item, cites) => {
                    link(&mut commit, item.stem.as_str(), &cites);
                    if !files.milestones.contains(&item) {
                        written
                            .written
                            .push(format!("memory:{MILESTONES_DIR}/{}", item.stem));
                        commit.ops.push(rows.milestone_op(&item)?);
                    }
                }
                PassChange::AddPending {
                    message,
                    subject,
                    text,
                } => {
                    // The last pass keeps nothing it would drop at once.
                    if !pass.final_pass {
                        written.pending += 1;
                        commit.pending_add.push(PendingRow {
                            session: session.clone(),
                            message,
                            subject,
                            text,
                            created: record.updated,
                        });
                    }
                }
                PassChange::DropPending(id) => commit.pending_drop.push(id),
            }
        }
        if pass.final_pass && (!pending.is_empty() || !commit.pending_drop.is_empty()) {
            commit.pending_clear = Some(session.clone());
        }
        if let Some((through, signal)) = &pass.signal {
            commit
                .signals
                .push((session.clone(), through.clone(), (*signal).min(3)));
        }
        Ok((commit, written))
    })
}

/// Record that `item` cites `cites`, so a forget of it deletes them.
fn link(commit: &mut MemoryCommit, item: &str, cites: &[(SessionId, String)]) {
    for (session, message) in cites {
        commit
            .links
            .push((item.to_owned(), session.to_string(), message.clone()));
    }
}

/// A pass window's memory written on its own, for tests: checked against
/// the redactor and planned as the pass does, then committed through one
/// journal. Only for tests.
#[cfg(any(test, feature = "test-support"))]
pub trait WindowMemoryTestWrites {
    /// Write a pass's memory changes outside a session write.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    fn write_pass_memory(
        &self,
        pass: &WindowMemory,
        redactor: &crate::Redactor,
        record: &SessionRecord,
    ) -> Result<MemoryWritten>;
}

#[cfg(any(test, feature = "test-support"))]
impl WindowMemoryTestWrites for crate::Store {
    fn write_pass_memory(
        &self,
        pass: &WindowMemory,
        redactor: &crate::Redactor,
        record: &SessionRecord,
    ) -> Result<MemoryWritten> {
        use crate::testing::StoreTestInternals;
        let (drafts, sensitive) = super::rules::check_sensitive_drafts(&pass.drafts, redactor);
        let pass = WindowMemory {
            drafts,
            ..pass.clone()
        };
        self.write_window_memory(
            plan_window(Some(&pass), &sensitive, Some(record)),
            record.updated,
        )
    }
}
