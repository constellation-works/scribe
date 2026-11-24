//! `marks.json`: each `#scribe` annotation marker's outcome, and the
//! locked write that records annotation entries with them
//! (docs/design/scribe/2_design.md §14.4, §14.5).

use super::{Redactor, Store};
use crate::annotation::{MAX_ATTEMPTS, MarkRecord, MarkState};
use crate::entry::{Cite, Entry, EntryId, NewEntry};
use crate::pass::covering_entry;
use crate::{Author, Result, SessionId};
use time::OffsetDateTime;

/// Which marker an update is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkKey {
    /// The session.
    pub session: SessionId,
    /// The marked turn's message uuid.
    pub message: String,
    /// The marked turn's `prompt_sha256`.
    pub prompt_sha256: String,
    /// The marker's number in the turn.
    pub marker: u32,
}

/// What to record for one marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkChange {
    /// Write this checked annotation entry (`content` the statement,
    /// `quote` his words, `stated_by` [`Author::annotation`]).
    Write(Box<NewEntry>),
    /// The model found nothing to record.
    Nothing,
    /// Refused with this code.
    Refused(&'static str),
    /// The model call failed: one more attempt spent.
    Failed,
}

/// What recording one marker came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkResult {
    /// The entry was written.
    Added(Entry),
    /// An earlier run already wrote it, or already settled the marker
    /// (`record` says how).
    Already(MarkRecord),
    /// An active entry of the same tag and project already says it:
    /// nothing written, refused `covered`.
    Covered(Entry),
    /// The quote holds text redaction would change: refused.
    NeedsRedaction,
    /// Recorded as nothing found.
    Nothing,
    /// Recorded as refused with this code.
    Refused(&'static str),
    /// The call failed; `attempts` spent so far, and whether that was the
    /// last ([`MAX_ATTEMPTS`]), so the marker is now refused `model_failed`.
    Failed {
        /// Attempts spent, this one included.
        attempts: u32,
        /// No attempt is left.
        gave_up: bool,
    },
}

/// The refusal code of a marker whose model calls all failed.
pub const MODEL_FAILED: &str = "model_failed";
/// The refusal code of an annotation an active entry already covers.
pub const COVERED: &str = "covered";
/// The refusal code of a quote redaction would change.
pub const NEEDS_REDACTION: &str = "needs_redaction";

impl Store {
    /// Every marker outcome of `session`, in file order; none for a
    /// missing file or store. Reads only.
    pub fn marks(&self, session: &SessionId) -> Result<Vec<MarkRecord>> {
        Ok(self
            .all_marks()?
            .into_iter()
            .filter(|m| m.session == *session)
            .collect())
    }

    /// The markers no entry or answer settled yet ([`MarkRecord::uncataloged`]):
    /// pending a retry or given up after [`MAX_ATTEMPTS`], of `session` or of
    /// every session, in file order. Reads only.
    pub fn uncataloged_marks(&self, session: Option<&SessionId>) -> Result<Vec<MarkRecord>> {
        Ok(self
            .all_marks()?
            .into_iter()
            .filter(|m| m.uncataloged() && session.is_none_or(|s| m.session == *s))
            .collect())
    }

    /// Open `session`'s given-up markers again ([`MarkRecord::gave_up`]):
    /// each becomes pending with no attempt spent, so the next catalog
    /// call (`scribe mark retry`, or the session's next `Stop`) asks the
    /// model again, up to [`MAX_ATTEMPTS`] more calls. Returns how many it
    /// opened; none writes nothing.
    pub fn reopen_marks(&self, session: &SessionId, now: OffsetDateTime) -> Result<usize> {
        if !self
            .all_marks()?
            .iter()
            .any(|m| m.session == *session && m.gave_up())
        {
            return Ok(0);
        }
        self.write_step(|| {
            self.refuse_forgotten(session)?;
            let mut marks = self.all_marks()?;
            let mut opened = 0;
            for mark in marks
                .iter_mut()
                .filter(|m| m.session == *session && m.gave_up())
            {
                mark.outcome = MarkState::Pending;
                mark.code = None;
                mark.attempts = 0;
                mark.at = now;
                opened += 1;
            }
            if opened > 0 {
                self.write_marks(&marks)?;
            }
            Ok(opened)
        })
    }

    /// Record each marker's outcome in one write, writing the
    /// checked annotation entries first (2_design.md §14.4, §14.5).
    ///
    /// A marker already settled in `marks.json` changes nothing and comes
    /// back [`MarkResult::Already`]. An entry is written once: one already
    /// written for the same cite line with the same quote (a run cut off
    /// before `marks.json` was written) is taken as the marker's. A quote
    /// redaction would change is refused, and so is a statement an active
    /// entry of the tag and project already covers. A failed call spends
    /// one attempt; the [`MAX_ATTEMPTS`]th refuses the marker
    /// [`MODEL_FAILED`]. Entries are written before `marks.json`.
    pub fn record_marks(
        &self,
        updates: Vec<(MarkKey, MarkChange)>,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<Vec<MarkResult>> {
        if updates.is_empty() {
            return Ok(Vec::new());
        }
        self.prepare_dir(&self.root)?;
        self.write_step(|| {
            let mut marks = self.all_marks()?;
            let (mut entries, skipped) = self.load_entries()?;
            let mut ids: Vec<EntryId> = entries.iter().map(|e| e.id.clone()).collect();
            ids.extend(
                skipped
                    .iter()
                    .filter_map(|s| s.path.file_stem()?.to_str()?.parse().ok()),
            );
            ids.extend(self.candidate_reserved_ids()?);
            let mut out = Vec::with_capacity(updates.len());
            let mut changed = false;
            for (key, change) in updates {
                self.refuse_forgotten(&key.session)?;
                let at = marks
                    .iter()
                    .position(|m| m.session == key.session && m.is(&key.message, key.marker));
                if let Some(found) = at.map(|i| &marks[i])
                    && found.outcome.is_final()
                {
                    out.push(MarkResult::Already(found.clone()));
                    continue;
                }
                let attempts = at.map_or(0, |i| marks[i].attempts);
                let record = |outcome, entry, code: Option<&str>, attempts| MarkRecord {
                    session: key.session.clone(),
                    message: key.message.clone(),
                    prompt_sha256: key.prompt_sha256.clone(),
                    marker: key.marker,
                    outcome,
                    entry,
                    code: code.map(str::to_owned),
                    attempts,
                    at: now,
                };
                let (next, result) = match change {
                    MarkChange::Nothing => (
                        record(MarkState::None, None, None, attempts),
                        MarkResult::Nothing,
                    ),
                    MarkChange::Refused(code) => (
                        record(MarkState::Refused, None, Some(code), attempts),
                        MarkResult::Refused(code),
                    ),
                    MarkChange::Failed => {
                        let spent = attempts.saturating_add(1);
                        let gave_up = spent >= MAX_ATTEMPTS;
                        let next = if gave_up {
                            record(MarkState::Refused, None, Some(MODEL_FAILED), spent)
                        } else {
                            record(MarkState::Pending, None, None, spent)
                        };
                        (
                            next,
                            MarkResult::Failed {
                                attempts: spent,
                                gave_up,
                            },
                        )
                    }
                    MarkChange::Write(draft) => {
                        let spent = attempts.saturating_add(1);
                        let quote = draft.quote.clone().unwrap_or_default();
                        if draft.tag.as_str() == crate::handoff::HANDOFF_TAG {
                            let code = crate::annotation::CheckRefusal::WrongTag.code();
                            (
                                record(MarkState::Refused, None, Some(code), spent),
                                MarkResult::Refused(code),
                            )
                        } else if let Some(found) = entries
                            .iter()
                            .find(|e| written_before(e, &draft.cite, &redactor.mask(&quote)))
                        {
                            let next =
                                record(MarkState::Entry, Some(found.id.clone()), None, spent);
                            (next.clone(), MarkResult::Already(next))
                        } else if redactor.mask(&quote) != quote {
                            (
                                record(MarkState::Refused, None, Some(NEEDS_REDACTION), spent),
                                MarkResult::NeedsRedaction,
                            )
                        } else if let Some(cover) = covering_entry(
                            &entries,
                            &redactor.mask(&draft.content),
                            &draft.tag,
                            &draft.project,
                        ) {
                            (
                                record(MarkState::Refused, None, Some(COVERED), spent),
                                MarkResult::Covered(cover.clone()),
                            )
                        } else {
                            let id = EntryId::next_in(&draft.tag, &ids);
                            let mut entry = draft.into_entry(id.clone());
                            redactor.entry(&mut entry);
                            self.write_entry(&entry, true)?;
                            ids.push(id.clone());
                            entries.push(entry.clone());
                            (
                                record(MarkState::Entry, Some(id), None, spent),
                                MarkResult::Added(entry),
                            )
                        }
                    }
                };
                match at {
                    Some(i) => marks[i] = next,
                    None => marks.push(next),
                }
                changed = true;
                out.push(result);
            }
            if changed {
                self.write_marks(&marks)?;
            }
            Ok(out)
        })
    }
}

/// Whether `entry` is an annotation already written for the marker at
/// `cite` with `quote`.
fn written_before(entry: &Entry, cite: &Cite, quote: &str) -> bool {
    entry.stated_by.as_ref() == Some(&Author::annotation())
        && entry.cite.as_ref().is_some_and(|c| c.same_line(cite))
        && entry.quote.as_deref() == Some(quote)
}
