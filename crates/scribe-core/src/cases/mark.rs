//! Mark verbs: recording each `#scribe` marker's outcome with its
//! annotation entry, and opening given-up markers again (2_design.md
//! §14.4, §14.5).

use crate::annotation::{MAX_ATTEMPTS, MarkRecord, MarkState};
use crate::cases::session::SessionVerbs;
use crate::entry::{Cite, Entry, EntryId};
use crate::pass::covering_entry;
use crate::{
    Author, COVERED, MODEL_FAILED, MarkChange, MarkKey, MarkResult, NEEDS_REDACTION, Redactor,
    Result, SessionId, Store,
};
use time::OffsetDateTime;

/// The mark verbs, implemented for [`Store`].
pub trait MarkVerbs {
    /// Open `session`'s given-up markers again ([`MarkRecord::gave_up`]):
    /// each becomes pending with no attempt spent, so the next catalog
    /// call (`scribe mark retry`, or the session's next `Stop`) asks the
    /// model again, up to [`MAX_ATTEMPTS`] more calls. Returns how many it
    /// opened; none writes nothing.
    fn reopen_marks(&self, session: &SessionId, now: OffsetDateTime) -> Result<usize>;

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
    fn record_marks(
        &self,
        updates: Vec<(MarkKey, MarkChange)>,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<Vec<MarkResult>>;
}

impl MarkVerbs for Store {
    fn reopen_marks(&self, session: &SessionId, now: OffsetDateTime) -> Result<usize> {
        if !self
            .read()
            .marks()
            .all()?
            .iter()
            .any(|m| m.session == *session && m.gave_up())
        {
            return Ok(0);
        }
        self.write(|tx| {
            self.refuse_forgotten(session)?;
            let mut marks = tx.marks().all()?;
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
                tx.marks().put_all(&marks)?;
            }
            Ok(opened)
        })
    }

    fn record_marks(
        &self,
        updates: Vec<(MarkKey, MarkChange)>,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<Vec<MarkResult>> {
        if updates.is_empty() {
            return Ok(Vec::new());
        }
        self.write(|tx| {
            let mut marks = tx.marks().all()?;
            let (mut entries, skipped) = tx.entries().all()?;
            let mut ids: Vec<EntryId> = entries.iter().map(|e| e.id.clone()).collect();
            ids.extend(
                skipped
                    .iter()
                    .filter_map(|s| s.path.file_stem()?.to_str()?.parse().ok()),
            );
            ids.extend(tx.proposals().reserved_entry_ids()?);
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
                            tx.entries().put(&entry, true)?;
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
                tx.marks().put_all(&marks)?;
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
