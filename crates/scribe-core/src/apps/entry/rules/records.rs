//! Validation and patch rules for decisions and open items.

use super::check_content;
use crate::apps::tag::check_agent_author;
use crate::entry::Entry;
use crate::store::refusal;
use crate::{
    AgentEntry, AnswerIn, Decision, EntryPatch, ItemState, OpenItem, Related, RelatedKind, Result,
    SessionId, WriteRefusal,
};

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

/// Refuse a decision too large to store.
pub fn check_decision_size(decision: &Decision, content: &str, quote: Option<&str>) -> Result<()> {
    if decision.chars(content, quote) > DECISION_MAX_CHARS {
        return Err(refusal(
            WriteRefusal::DecisionTooLong,
            "a decision holds at most 3,000 characters across its content, options, rationale and quotes",
        ));
    }
    Ok(())
}

/// Refuse a decision whose fields do not fit who decided it.
pub fn check_decision(decision: &Decision, draft: &AgentEntry) -> Result<()> {
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

/// Refuse an open item with an invalid asker, blocker or options.
pub fn check_open_item(item: &OpenItem) -> Result<()> {
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

/// Refuse a decision whose quoted text redaction would alter.
pub fn check_decision_redaction(draft: &AgentEntry, mask: &dyn Fn(&str) -> String) -> Result<()> {
    if let Some(decision) = &draft.decision
        && draft
            .quote
            .iter()
            .chain(&decision.reason_quote)
            .any(|quote| mask(quote) != *quote)
    {
        return Err(refusal(
            WriteRefusal::NeedsRedaction,
            "the decision has quoted text that redaction would alter",
        ));
    }
    Ok(())
}

/// Refuse an update that changes a decision as it may not change.
pub fn check_decision_update(before: &Entry, after: &Entry) -> Result<()> {
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

/// Apply `patch`'s decision fields to `entry`, masked by `mask`.
pub fn patch_decision(
    entry: &mut Entry,
    patch: &EntryPatch,
    mask: &dyn Fn(&str) -> String,
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
        decision.rationale = Some(mask(rationale));
    }
    if let Some(options) = &patch.decision_options {
        check_options(options, patch.decision_recommended)?;
        decision.options = options.iter().map(|option| mask(option)).collect();
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
pub fn item_mut(entry: &mut Entry) -> &mut OpenItem {
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
