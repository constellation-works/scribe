//! Validation for decision and open-item writes.

use super::agent::{check_agent_author, check_content, refusal};
use super::{Redactor, Store};
use crate::entry::{DECISION_TAG, OPEN_ITEM_TAG};
use crate::{
    AgentEntry, Decision, Entry, EntryId, EntryPatch, EntryStatus, OpenItem, Related, RelatedKind,
    Result, SessionId, TagName, WriteRefusal,
};

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
