//! Checks on an agent entry's tag, quote, evidence and handoff fields.

use super::refusal;
use crate::entry::is_decision_or_item;
use crate::record::patch;
use crate::{AuthorName, Entry, Result, TagName, WriteRefusal};
use time::OffsetDateTime;

/// A quote belongs on an action tag or a `require_quote` tag, and any tag
/// that requires one gets one. Whether it is verbatim in the user's turns is
/// the caller's check: it needs the transcript.
pub(crate) fn check_quote(def: &crate::TagDef, quote: Option<&str>) -> Result<()> {
    if def.requires_quote() {
        match quote {
            None => Err(refusal(
                WriteRefusal::QuoteRequired,
                if def.name.as_str() == crate::handoff::HANDOFF_TAG {
                    format!(
                        "{} requires the user's request: pass --quote with their words and --related session:<id>",
                        def.name
                    )
                } else {
                    format!(
                        "{} requires the user's words: pass --quote with the user's correction and --related session:<id>; without a quote, file a friction instead",
                        def.name
                    )
                },
            )),
            Some(quote) if quote.trim().is_empty() => Err(refusal(
                WriteRefusal::QuoteRequired,
                "--quote must not be empty",
            )),
            Some(_) => Ok(()),
        }
    } else {
        match (def.action, quote) {
            (None, Some(_)) => Err(refusal(
                WriteRefusal::NotActionTag,
                format!(
                    "{} is not an action tag; --quote applies only to an action tag or a require_quote tag",
                    def.name
                ),
            )),
            (Some(_), Some(quote)) if quote.trim().is_empty() => Err(refusal(
                WriteRefusal::QuoteRequired,
                "--quote must not be empty",
            )),
            _ => Ok(()),
        }
    }
}

pub(crate) fn check_spoken_tag(def: &crate::TagDef) -> Result<()> {
    if def.name.as_str() == crate::handoff::HANDOFF_TAG || is_decision_or_item(def.name.as_str()) {
        return Err(refusal(
            WriteRefusal::NotMemoryTag,
            format!("{} has its own entry fields; omit --spoken", def.name),
        ));
    }
    if def.action.is_some() {
        return Err(refusal(
            WriteRefusal::NotMemoryTag,
            format!(
                "{} is an action tag; --spoken writes only non-action memory tags (such as preferences)",
                def.name
            ),
        ));
    }
    if def.require_evidence {
        return Err(refusal(
            WriteRefusal::NotMemoryTag,
            format!(
                "{} requires evidence; --spoken writes only non-action memory tags",
                def.name
            ),
        ));
    }
    Ok(())
}

pub(crate) fn check_spoken_quote(quote: Option<&str>) -> Result<()> {
    match quote {
        None => Err(refusal(
            WriteRefusal::QuoteRequired,
            "--spoken requires the user's words: pass --quote '<their words>' and --related session:<id>",
        )),
        Some(q) if q.trim().is_empty() => Err(refusal(
            WriteRefusal::QuoteRequired,
            "--quote must not be empty",
        )),
        Some(_) => Ok(()),
    }
}

/// A handoff entry's body must carry the six fixed headings in order and fit
/// within [`crate::handoff::MAX_CHARS`] (2_design.md §16).
pub(crate) fn check_handoff(tag: &TagName, content: &str) -> Result<()> {
    if tag.as_str() != crate::handoff::HANDOFF_TAG {
        return Ok(());
    }
    crate::handoff::check(content).map(|_| ()).map_err(|err| {
        let kind = match err {
            crate::HandoffError::TooLong { .. } => WriteRefusal::HandoffTooLong,
            crate::HandoffError::Preamble
            | crate::HandoffError::MissingHeadings(_)
            | crate::HandoffError::DuplicateHeading(_)
            | crate::HandoffError::UnknownHeading(_)
            | crate::HandoffError::OutOfOrder(_) => WriteRefusal::HandoffShape,
        };
        refusal(kind, err.to_string())
    })
}

/// Refuse a new `handoff` entry whose `related` references do not name
/// exactly one distinct valid `session:<id>` (2_design.md §16.1).
///
/// # Errors
///
/// [`WriteRefusal::QuoteRequired`] when no `session:` reference is present, or
/// [`WriteRefusal::HandoffSession`] when a `session:` target is invalid or
/// more than one distinct session is named.
pub fn check_handoff_related(tag: &TagName, related: &[crate::Related]) -> Result<()> {
    if tag.as_str() != crate::handoff::HANDOFF_TAG {
        return Ok(());
    }
    crate::handoff::check_related(related)
        .map(|_| ())
        .map_err(|err| {
            let kind = match err {
                crate::HandoffSessionError::Missing => WriteRefusal::QuoteRequired,
                crate::HandoffSessionError::Invalid { .. }
                | crate::HandoffSessionError::Multiple { .. } => WriteRefusal::HandoffSession,
            };
            refusal(kind, err.to_string())
        })
}

/// Refuse an `entry update` patch that would add, remove, or replace a
/// `handoff` entry's `session:` reference, which would attach the handoff to
/// an unverified session or orphan it (2_design.md §16.1).
pub(crate) fn check_handoff_related_update(
    entry: &Entry,
    add_related: &[crate::Related],
    remove_related: &[crate::Related],
) -> Result<()> {
    if entry.tag.as_str() != crate::handoff::HANDOFF_TAG {
        return Ok(());
    }
    let mut patched = entry.related.clone();
    patch(&mut patched, add_related, remove_related);
    if crate::handoff::session_refs_changed(&entry.related, &patched) {
        return Err(refusal(
            WriteRefusal::HandoffSession,
            "a handoff's session reference cannot be added, removed or replaced; retire this handoff and add a new one for the target session instead",
        ));
    }
    Ok(())
}

/// An evidence pointer belongs on a `require_evidence` (finding) entry,
/// and such a tag gets one: a result with nothing to check it against is
/// just an agent's memory of itself.
pub(crate) fn check_evidence(def: &crate::TagDef, evidence: Option<&str>) -> Result<()> {
    match (def.require_evidence, evidence) {
        (true, None) => Err(refusal(
            WriteRefusal::EvidenceRequired,
            format!(
                "{} records a measured or verified result: pass --evidence with a pointer to its evidence (a path, task, run, experiment or commit)",
                def.name
            ),
        )),
        (true, Some(evidence)) if evidence.trim().is_empty() => Err(refusal(
            WriteRefusal::EvidenceRequired,
            "--evidence must not be empty",
        )),
        (false, Some(_)) => Err(refusal(
            WriteRefusal::EvidenceRequired,
            format!(
                "{} does not take an evidence pointer; --evidence applies only to the finding tag",
                def.name
            ),
        )),
        _ => Ok(()),
    }
}

/// The session a direct finding write names (a `session:` reference), as
/// the per-session cap counts it.
pub(crate) fn draft_session(related: &[crate::Related]) -> Option<String> {
    related
        .iter()
        .find(|r| r.kind() == crate::RelatedKind::Session)
        .map(|r| r.target().to_owned())
}

/// Whether `entry` counts against this write's per-session finding cap:
/// same named session, or, when the write names none, the same author's
/// unnamed-session findings of the same day.
pub(crate) fn counts_toward_cap(
    entry: &Entry,
    tag: &TagName,
    session: Option<&str>,
    author: &AuthorName,
    now: OffsetDateTime,
) -> bool {
    if &entry.tag != tag || !entry.is_finding() {
        return false;
    }
    match (session, entry.session_key()) {
        (Some(named), Some(held)) => named == held,
        (None, None) => &entry.author.name == author && entry.created.date() == now.date(),
        _ => false,
    }
}
