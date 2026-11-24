//! Validation for decision and open-item writes.

use super::agent::{check_agent_author, check_content, check_reason, refusal};
use super::{Redactor, Store};
use crate::entry::{DECISION_TAG, OPEN_ITEM_TAG};
use crate::{
    AgentEntry, AnswerIn, Author, AuthorName, Decision, Entry, EntryId, EntryPatch, EntryStatus,
    ItemAnswer, ItemClosure, ItemState, OpenItem, Related, RelatedKind, Result, SessionId, TagName,
    WriteRefusal,
};
use time::OffsetDateTime;

/// What a trial entry's synthesized `item` says waited, since the trial
/// recorded it only in prose (2_design.md §18.7).
const TRIAL_BLOCKED: &str = "not recorded (trial entry)";

const DECISION_MAX_CHARS: usize = 3_000;

fn check_options(options: &[String], recommended: Option<usize>) -> Result<()> {
    if !(options.is_empty() || (2..=6).contains(&options.len()))
        || options.iter().any(|option| option.trim().is_empty())
    {
        return Err(refusal(
            WriteRefusal::InvalidOptions,
            "supply either zero or two to six nonblank --option values",
        ));
    }
    if recommended.is_some_and(|n| !(1..=options.len()).contains(&n)) {
        return Err(refusal(
            WriteRefusal::InvalidRecommendation,
            "--recommended must be the 1-based index of a supplied option",
        ));
    }
    Ok(())
}

fn check_record_reference(related: &[Related]) -> Result<()> {
    if !related.iter().any(|r| {
        matches!(
            r.kind(),
            RelatedKind::Task | RelatedKind::Pr | RelatedKind::Repo
        )
    }) {
        return Err(refusal(
            WriteRefusal::RecordRequired,
            "a decision needs a record: pass --related task:<id>, pr:<owner>/<repo>#<n> or repo:<slug>",
        ));
    }
    Ok(())
}

pub(super) fn check_decision_size(
    decision: &Decision,
    content: &str,
    quote: Option<&str>,
) -> Result<()> {
    if decision.chars(content, quote) > DECISION_MAX_CHARS {
        return Err(refusal(
            WriteRefusal::DecisionTooLong,
            "a decision holds at most 3,000 characters across its content, options, rationale and quotes",
        ));
    }
    Ok(())
}

fn check_decision(decision: &Decision, draft: &AgentEntry) -> Result<()> {
    let quote = draft.quote.as_deref();
    if decision.by_user() {
        if decision.rationale.is_some() {
            return Err(refusal(
                WriteRefusal::RationaleNotAllowed,
                "a user decision takes only the user's own reason in --reason-quote",
            ));
        }
        if !quote.is_some_and(has_text) {
            return Err(refusal(
                WriteRefusal::QuoteRequired,
                "a user decision requires --quote with the user's words",
            ));
        }
        if decision
            .reason_quote
            .as_deref()
            .is_some_and(|reason| !has_text(reason))
        {
            return Err(refusal(
                WriteRefusal::QuoteNotFound,
                "--reason-quote must contain the user's words",
            ));
        }
        if !draft
            .related
            .iter()
            .any(|r| r.kind() == RelatedKind::Session && r.target().parse::<SessionId>().is_ok())
        {
            return Err(refusal(
                WriteRefusal::QuoteRequired,
                "pass --related session:<id> for the user's decision",
            ));
        }
    } else {
        check_agent_author(&decision.decided_by)?;
        if quote.is_some() || decision.reason_quote.is_some() {
            return Err(refusal(
                WriteRefusal::DecisionQuoteNotAllowed,
                "--quote and --reason-quote apply to decisions made by the user",
            ));
        }
        if !decision.rationale.as_deref().is_some_and(has_text) {
            return Err(refusal(
                WriteRefusal::RationaleRequired,
                "an agent decision requires --rationale",
            ));
        }
    }
    check_options(&decision.options, decision.recommended)?;
    check_record_reference(&draft.related)?;
    check_decision_size(decision, &draft.content, quote)
}

fn has_text(value: &str) -> bool {
    !value.trim().is_empty()
}

fn check_open_item(item: &OpenItem) -> Result<()> {
    check_agent_author(&item.asked_by)?;
    check_content(&item.blocked)?;
    check_options(&item.options, item.recommended)?;
    if !item.options.is_empty() && item.recommended.is_none() {
        return Err(refusal(
            WriteRefusal::InvalidRecommendation,
            "a choice needs --recommended with the 1-based index of an option",
        ));
    }
    if item.options.is_empty() && item.why.is_some()
        || !item.options.is_empty() && !item.why.as_deref().is_some_and(has_text)
    {
        return Err(refusal(
            WriteRefusal::RecommendationWhy,
            "--why requires options and must explain the recommendation for a choice",
        ));
    }
    if item.answer.is_some() || item.closed.is_some() {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "a new open item starts without an answer or closure",
        ));
    }
    Ok(())
}

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

pub(super) fn check_decision_redaction(draft: &AgentEntry, redactor: &Redactor) -> Result<()> {
    if let Some(decision) = &draft.decision
        && draft
            .quote
            .iter()
            .chain(&decision.reason_quote)
            .any(|quote| redactor.mask(quote) != *quote)
    {
        return Err(refusal(
            WriteRefusal::NeedsRedaction,
            "the decision has quoted text that redaction would alter",
        ));
    }
    Ok(())
}

pub(super) fn check_decision_update(before: &Entry, after: &Entry) -> Result<()> {
    if let Some(decision) = &after.decision {
        check_record_reference(&after.related)?;
        if decision.by_user()
            && crate::handoff::session_refs_changed(&before.related, &after.related)
        {
            return Err(refusal(
                WriteRefusal::DecisionSession,
                "the source sessions of a user decision are fixed",
            ));
        }
        check_decision_size(decision, &after.content, after.quote.as_deref())?;
    }
    Ok(())
}

pub(super) fn patch_decision(
    entry: &mut Entry,
    patch: &EntryPatch,
    redactor: &Redactor,
) -> Result<()> {
    if patch.decision_options.is_none()
        && patch.decision_recommended.is_none()
        && patch.decision_rationale.is_none()
    {
        return Ok(());
    }
    let decision = entry.decision.as_mut().ok_or_else(|| refusal(
        WriteRefusal::RecordFields,
        "--option, --clear-options, --recommended and --rationale on entry update require a structured decision",
    ))?;
    if let Some(rationale) = &patch.decision_rationale {
        if decision.by_user() {
            return Err(refusal(
                WriteRefusal::RationaleNotAllowed,
                "a user decision's reason is its checked --reason-quote",
            ));
        }
        if !has_text(rationale) {
            return Err(refusal(
                WriteRefusal::RationaleRequired,
                "an agent decision requires a nonblank rationale",
            ));
        }
        decision.rationale = Some(redactor.mask(rationale));
    }
    if let Some(options) = &patch.decision_options {
        check_options(options, patch.decision_recommended)?;
        decision.options = options.iter().map(|option| redactor.mask(option)).collect();
        decision.recommended = patch.decision_recommended;
    } else if let Some(recommended) = patch.decision_recommended {
        check_options(&decision.options, Some(recommended))?;
        decision.recommended = Some(recommended);
    }
    Ok(())
}

/// The derived state of an item `entry answer` or `entry close` may still
/// settle: refuse an entry not under `open-item`, and a closed or withdrawn
/// one, so a caller can refuse before it reads any transcript.
///
/// # Errors
///
/// `not_open_item` and `item_settled`.
pub fn check_unsettled_item(entry: &Entry) -> Result<ItemState> {
    let state = ItemState::of(entry).ok_or_else(|| {
        refusal(
            WriteRefusal::NotOpenItem,
            format!(
                "{} is not an open item; entry answer and entry close take only entries under open-item",
                entry.id
            ),
        )
    })?;
    if state.is_settled() {
        return Err(refusal(
            WriteRefusal::ItemSettled,
            format!("{} is already {state}; nothing written", entry.id),
        ));
    }
    Ok(state)
}

/// The `item` object of an open item, synthesized for a trial entry that
/// has none: an action item asked by its author, with nothing typed about
/// what waits.
fn item_mut(entry: &mut Entry) -> &mut OpenItem {
    let asked_by = entry.author.name.clone();
    entry.item.get_or_insert_with(|| OpenItem {
        asked_by,
        options: Vec::new(),
        recommended: None,
        why: None,
        blocked: TRIAL_BLOCKED.to_owned(),
        answer_in: AnswerIn::Any,
        answer: None,
        closed: None,
    })
}

impl Store {
    /// Record the user's checked answer on an open item (2_design.md §18.4).
    /// The caller has found `answer.words` in a typed turn of
    /// `answer.session`. A newer answer replaces an earlier one until the
    /// item closes; an older answer or the same words from the same session
    /// change nothing. Equal timestamps retain submission order.
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
            if item.answer.as_ref().is_some_and(|old| {
                answer.at < old.at || (old.words == answer.words && old.session == answer.session)
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
