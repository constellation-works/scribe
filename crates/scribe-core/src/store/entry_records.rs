//! Validation for decision and open-item writes.

use super::{Redactor, Store};
use crate::checks::records::{check_decision, check_open_item, check_unsettled_item, item_mut};
use crate::checks::{check_agent_author, check_reason, refusal};
use crate::entry::{DECISION_TAG, OPEN_ITEM_TAG};
use crate::{
    AgentEntry, Author, AuthorName, Decision, Entry, EntryId, EntryStatus, ItemAnswer, ItemClosure,
    ItemState, RelatedKind, Result, SessionId, TagName, WriteRefusal,
};
use time::OffsetDateTime;

impl Store {
    /// Check the fields specific to the resolved tag before checking transcripts.
    pub fn check_new_entry_records(&self, tag: &TagName, draft: &AgentEntry) -> Result<()> {
        match (tag.as_str(), &draft.decision, &draft.item) {
            (DECISION_TAG, Some(decision), None) => {
                check_decision(decision, draft)?;
                if decision.by_user() {
                    for session in draft
                        .related
                        .iter()
                        .filter(|r| r.kind() == RelatedKind::Session)
                        .filter_map(|r| r.target().parse::<SessionId>().ok())
                    {
                        self.refuse_forgotten(&session)?;
                    }
                }
                Ok(())
            }
            (OPEN_ITEM_TAG, None, Some(item)) => check_open_item(item),
            (DECISION_TAG, None, None) => Err(refusal(
                WriteRefusal::DecidedByRequired,
                "a decision requires --decided-by",
            )),
            (OPEN_ITEM_TAG, None, None) => Err(refusal(
                WriteRefusal::AskedByRequired,
                "an open item requires --asked-by and --blocked",
            )),
            (_, None, None) => Ok(()),
            _ => Err(refusal(
                WriteRefusal::RecordFields,
                "use decision fields only with --tag decision and open-item fields only with --tag open-item",
            )),
        }
    }

    /// Validate decision links while holding the transaction that writes them.
    pub(super) fn decision_replacement(&self, decision: &Decision) -> Result<Option<Entry>> {
        if let Some(id) = &decision.settles {
            self.linked_entry(id, OPEN_ITEM_TAG, WriteRefusal::InvalidSettles)?;
        }
        let Some(id) = &decision.supersedes else {
            return Ok(None);
        };
        let older = self.linked_entry(id, DECISION_TAG, WriteRefusal::InvalidSupersedes)?;
        if older.status != EntryStatus::Active {
            return Err(refusal(
                WriteRefusal::InvalidSupersedes,
                "--supersedes must name an active decision",
            ));
        }
        if !decision.by_user() && older.decision.as_ref().is_none_or(Decision::by_user) {
            return Err(refusal(
                WriteRefusal::SupersedeUserDecision,
                "only a user decision may supersede a user decision or a trial decision with no typed decider",
            ));
        }
        Ok(Some(older))
    }

    fn linked_entry(&self, id: &EntryId, tag: &str, kind: WriteRefusal) -> Result<Entry> {
        let invalid = || refusal(kind, format!("the link must name an existing {tag} entry"));
        if id.tag().as_str() != tag {
            return Err(invalid());
        }
        match self.entry(id) {
            Err(crate::Error::EntryNotFound { .. }) => Err(invalid()),
            result => result,
        }
    }
}

impl Store {
    /// Record the user's checked answer on an open item (2_design.md §18.4).
    /// The caller has found `answer.words` in a typed turn of
    /// `answer.session`. A newer answer replaces an earlier one until the
    /// item closes; an older answer, or a replay of the stored typed turn
    /// (same words, same session, same time), changes nothing. Other equal
    /// timestamps retain submission order.
    ///
    /// # Errors
    ///
    /// `not_open_item`, `item_settled`, `needs_redaction` for words
    /// redaction would alter, `session_forgotten`, and the store's errors.
    pub fn answer_open_item(
        &self,
        id: &str,
        answer: ItemAnswer,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&answer.recorded_by)?;
        if redactor.mask(&answer.words) != answer.words {
            return Err(refusal(
                WriteRefusal::NeedsRedaction,
                "the answer has quoted text that redaction would alter; nothing written",
            ));
        }
        self.refuse_forgotten(&answer.session)?;
        self.prepare_dir(&self.root)?;
        self.write(|| {
            // Checked again inside the write: a forget committed after the
            // check above must not be followed by a fresh answer write.
            self.refuse_forgotten(&answer.session)?;
            let mut entry = self.resolve_entry(id)?;
            check_unsettled_item(&entry)?;
            let author = Author::agent(answer.recorded_by.clone());
            let item = item_mut(&mut entry);
            // Only a replay of the stored typed turn (same words, same session,
            // same time) is idempotent; a later typed repetition of the words
            // is a later choice and moves the answer's time.
            if item.answer.as_ref().is_some_and(|old| {
                answer.at < old.at
                    || (answer.at == old.at
                        && old.words == answer.words
                        && old.session == answer.session)
            }) {
                return Ok((self.resolve_entry(id)?, false));
            }
            item.answer = Some(answer);
            entry.updated = now;
            entry.updated_by = Some(author);
            self.write_entry(&entry, false)?;
            Ok((entry, true))
        })
    }

    /// Close an open item once its asker carried it out (2_design.md §18.4):
    /// retire it with `item.closed`, naming the decision that settles it or
    /// a note that none lasts.
    ///
    /// # Errors
    ///
    /// `not_open_item`, `item_settled`, `not_answered` for a choice with no
    /// answer, `invalid_decision` when `decision` is not an active decision
    /// whose `settles` names this item, `invalid_text` for a blank or long
    /// note, and the store's errors.
    pub fn close_open_item(
        &self,
        id: &str,
        decision: Option<&EntryId>,
        note: Option<&str>,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&author)?;
        let note = match (note, decision) {
            (Some(note), _) => {
                check_reason(note)?;
                redactor.mask(note)
            }
            (None, Some(decision)) => format!("settled by {decision}"),
            (None, None) => {
                return Err(refusal(
                    WriteRefusal::InvalidText,
                    "close names the decision written for the item (--decision) or says what was done (--note)",
                ));
            }
        };
        self.prepare_dir(&self.root)?;
        self.write(|| {
            let mut entry = self.resolve_entry(id)?;
            let state = check_unsettled_item(&entry)?;
            let choice = entry.item.as_ref().is_some_and(|i| !i.options.is_empty());
            if choice && state != ItemState::Answered {
                return Err(refusal(
                    WriteRefusal::NotAnswered,
                    format!(
                        "{} is a choice and has no recorded answer; record the user's answer with entry answer first",
                        entry.id
                    ),
                ));
            }
            if let Some(decision) = decision {
                self.check_settling_decision(decision, &entry.id)?;
            }
            let by = Author::agent(author.clone());
            item_mut(&mut entry).closed = Some(ItemClosure {
                at: now,
                by: author,
                decision: decision.cloned(),
                note: note.clone(),
            });
            entry.status = EntryStatus::Retired;
            entry.retirement_reason = Some(format!("closed: {note}"));
            entry.inactive_since = Some(now);
            entry.updated = now;
            entry.updated_by = Some(by);
            self.write_entry(&entry, false)?;
            Ok((entry, true))
        })
    }

    /// Refuse `decision` unless it is an active decision whose `settles`
    /// names `item`, so the link between them runs both ways.
    fn check_settling_decision(&self, decision: &EntryId, item: &EntryId) -> Result<()> {
        let invalid = || {
            refusal(
                WriteRefusal::InvalidDecision,
                format!(
                    "--decision must name an active decision written with --settles {item}; nothing written"
                ),
            )
        };
        if decision.tag().as_str() != DECISION_TAG {
            return Err(invalid());
        }
        let entry = match self.entry(decision) {
            Err(crate::Error::EntryNotFound { .. }) => return Err(invalid()),
            result => result?,
        };
        let settles = entry.decision.as_ref().and_then(|d| d.settles.as_ref());
        if entry.status != EntryStatus::Active || settles != Some(item) {
            return Err(invalid());
        }
        Ok(())
    }
}
