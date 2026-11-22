//! Agent-written session handoffs (docs/design/scribe/2_design.md §16).
//!
//! A handoff is an ordinary agent-authored entry under the shipped `handoff`
//! tag whose `related` names the session (`session:<id>`) and whose `quote`
//! carries the user's request verbatim from one of their typed turns in that
//! session. Its body must carry six fixed `## ` headings in exact order and
//! stay within [`MAX_CHARS`] so it fits one screen. Pure values, no I/O
//! (STD-02 R5).

use crate::entry::{Entry, EntryStatus};
use crate::record::{Related, RelatedKind};
use crate::session::SessionId;
use std::collections::BTreeSet;

/// The shipped tag name for agent-written session handoffs.
pub const HANDOFF_TAG: &str = "handoff";

/// The six required level-2 Markdown headings, in exact order.
pub const HEADINGS: [&str; 6] = [
    "State",
    "In flight",
    "Next",
    "Waiting on you",
    "Do not touch",
    "Pointers",
];

/// The longest handoff body accepted, in characters, so a handoff fits one
/// screen (~40 lines at 75 columns).
pub const MAX_CHARS: usize = 3000;

/// Why a handoff body was refused. Each variant names the exact fault.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandoffError {
    /// The body exceeds [`MAX_CHARS`].
    #[error("handoff body is {chars} characters; the size limit is {max} characters")]
    TooLong {
        /// The length of the submitted body.
        chars: usize,
        /// The limit ([`MAX_CHARS`]).
        max: usize,
    },
    /// Non-whitespace text appeared before the first `## ` heading.
    #[error(
        "handoff has text before the first heading; start with '## State' and use the six headings in order: {expected}",
        expected = HEADINGS.join(", ")
    )]
    Preamble,
    /// One or more of the six required headings are missing.
    #[error(
        "handoff is missing required heading(s): {missing}; expected in order: {expected}",
        missing = .0.join(", "),
        expected = HEADINGS.join(", ")
    )]
    MissingHeadings(Vec<&'static str>),
    /// One of the headings appears more than once.
    #[error("handoff repeats heading '{0}'; each of the six headings must appear once")]
    DuplicateHeading(String),
    /// A `## ` heading other than the six fixed ones appeared.
    #[error(
        "handoff has unknown heading '{0}'; expected only: {expected}",
        expected = HEADINGS.join(", ")
    )]
    UnknownHeading(String),
    /// All six headings are present once each, but not in the required order.
    #[error(
        "handoff headings are out of order (found: {found}; expected: {expected})",
        found = .0.join(", "),
        expected = HEADINGS.join(", ")
    )]
    OutOfOrder(Vec<String>),
}

/// Why a `handoff` entry's `session:` reference set was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandoffSessionError {
    /// No `session:<id>` reference was supplied.
    #[error("handoff requires the session it belongs to: pass --related session:<id>")]
    Missing,
    /// A `session:` reference target did not parse as a [`SessionId`].
    #[error("invalid handoff session reference '{reference}': {reason}")]
    Invalid {
        /// The rejected `related` reference.
        reference: String,
        /// Why its target is not a valid session id.
        reason: String,
    },
    /// More than one distinct `session:<id>` reference was supplied.
    #[error("a handoff belongs to one session; pass a single --related session:<id> (got {count})")]
    Multiple {
        /// How many distinct sessions were named.
        count: usize,
    },
}

/// Check a new `handoff` entry's `related` references: it must carry exactly
/// one distinct valid `session:<id>` reference, and every `session:` reference
/// must parse as a [`SessionId`]. Non-session references (`task:`, `pr:`,
/// `repo:`) are unaffected.
///
/// # Errors
///
/// [`HandoffSessionError::Missing`] when no `session:` reference is present,
/// [`HandoffSessionError::Invalid`] when a `session:` target is not a valid
/// [`SessionId`], or [`HandoffSessionError::Multiple`] when more than one
/// distinct session is named.
pub fn check_related(related: &[Related]) -> Result<SessionId, HandoffSessionError> {
    let mut sessions: Vec<SessionId> = Vec::new();
    for rel in related.iter().filter(|r| r.kind() == RelatedKind::Session) {
        let id = rel
            .target()
            .parse::<SessionId>()
            .map_err(|err| HandoffSessionError::Invalid {
                reference: rel.to_string(),
                reason: err.to_string(),
            })?;
        if !sessions.contains(&id) {
            sessions.push(id);
        }
    }
    match sessions.len() {
        0 => Err(HandoffSessionError::Missing),
        1 => Ok(sessions.remove(0)),
        count => Err(HandoffSessionError::Multiple { count }),
    }
}

/// Whether updating `before` to `after` changes a `handoff` entry's `session:`
/// reference set (adding, removing, or replacing any `session:` reference).
#[must_use]
pub fn session_refs_changed(before: &[Related], after: &[Related]) -> bool {
    fn targets(list: &[Related]) -> BTreeSet<&str> {
        list.iter()
            .filter(|r| r.kind() == RelatedKind::Session)
            .map(Related::target)
            .collect()
    }
    targets(before) != targets(after)
}

/// Check a handoff body: at most [`MAX_CHARS`] characters, no text before
/// `## State`, and exactly the six [`HEADINGS`] in order (ignoring `## `
/// lines inside fenced code blocks). Returns `(heading, body)` sections.
///
/// # Errors
///
/// [`HandoffError`] naming the exact fault: size limit, preamble, missing
/// heading(s), duplicate heading, unknown heading, or out-of-order headings.
pub fn check(text: &str) -> Result<Vec<(&str, &str)>, HandoffError> {
    let chars = text.chars().count();
    if chars > MAX_CHARS {
        return Err(HandoffError::TooLong {
            chars,
            max: MAX_CHARS,
        });
    }
    let sections = parse_sections(text)?;
    let found: Vec<&str> = sections.iter().map(|(h, _)| *h).collect();
    let missing: Vec<&'static str> = HEADINGS
        .iter()
        .copied()
        .filter(|h| !found.contains(h))
        .collect();
    if !missing.is_empty() {
        return Err(HandoffError::MissingHeadings(missing));
    }
    let mut seen: Vec<&str> = Vec::with_capacity(found.len());
    for heading in &found {
        if !HEADINGS.contains(heading) {
            return Err(HandoffError::UnknownHeading((*heading).to_owned()));
        }
        if seen.contains(heading) {
            return Err(HandoffError::DuplicateHeading((*heading).to_owned()));
        }
        seen.push(heading);
    }
    if found.as_slice() != HEADINGS {
        return Err(HandoffError::OutOfOrder(
            found.into_iter().map(str::to_owned).collect(),
        ));
    }
    Ok(sections)
}

/// Whether `text` looks like a handoff (all six [`HEADINGS`] appear as `## `
/// lines in order, outside fenced code blocks), even when preceded by a
/// framing line in an assistant reply. Used so the session pass never turns
/// an echoed handoff into a claim or memory item.
#[must_use]
pub fn is_handoff(text: &str) -> bool {
    let mut fence: Option<(char, usize)> = None;
    let mut next = 0usize;
    for line in text.lines() {
        let bare = line.trim_end_matches('\r');
        if update_fence(&mut fence, bare) {
            continue;
        }
        if let Some(heading) = bare.strip_prefix("## ")
            && HEADINGS.get(next).is_some_and(|h| *h == heading.trim_end())
        {
            next += 1;
            if next == HEADINGS.len() {
                return true;
            }
        }
    }
    false
}

/// Active `handoff` entries referencing `session:<id>`, newest first (by
/// `created`, then highest entry id).
#[must_use]
pub fn for_session<'a>(entries: &'a [Entry], id: &SessionId) -> Vec<&'a Entry> {
    let mut found: Vec<&'a Entry> = entries
        .iter()
        .filter(|e| {
            e.status == EntryStatus::Active
                && e.tag.as_str() == HANDOFF_TAG
                && references_session(e, id)
        })
        .collect();
    found.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
    found
}

/// Every `session:<id>` reference an entry carries that parses as a
/// [`SessionId`].
pub fn session_ids(entry: &Entry) -> Vec<SessionId> {
    let mut ids = Vec::new();
    for rel in &entry.related {
        if rel.kind() == RelatedKind::Session
            && let Ok(id) = rel.target().parse::<SessionId>()
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

fn references_session(entry: &Entry, id: &SessionId) -> bool {
    entry
        .related
        .iter()
        .any(|r| r.kind() == RelatedKind::Session && r.target() == id.as_str())
}

/// Split `text` into `(heading, body)` sections on top-level `## ` lines
/// outside fenced code blocks. Refuses non-blank text before the first
/// heading.
fn parse_sections(text: &str) -> Result<Vec<(&str, &str)>, HandoffError> {
    let mut out: Vec<(&str, &str)> = Vec::new();
    let mut head: Option<&str> = None;
    let mut body_start = 0usize;
    let mut offset = 0usize;
    let mut fence: Option<(char, usize)> = None;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if !update_fence(&mut fence, bare)
            && let Some(heading) = bare.strip_prefix("## ")
        {
            match head {
                Some(h) => out.push((h, &text[body_start..offset])),
                None => {
                    if !text[..offset].trim().is_empty() {
                        return Err(HandoffError::Preamble);
                    }
                }
            }
            head = Some(heading.trim_end());
            body_start = offset + line.len();
        }
        offset += line.len();
    }
    if let Some(h) = head {
        out.push((h, &text[body_start..]));
    }
    Ok(out)
}

/// Track opening/closing Markdown code fences (` ``` ` or `~~~`). Returns
/// `true` when `line` is inside a fence or is a fence delimiter itself.
fn update_fence(fence: &mut Option<(char, usize)>, line: &str) -> bool {
    let trimmed = line.trim_start();
    let marker = trimmed
        .chars()
        .next()
        .filter(|c| matches!(c, '`' | '~'))
        .and_then(|c| {
            let n = trimmed.chars().take_while(|m| *m == c).count();
            (n >= 3).then_some((c, n))
        });
    if let Some((mark, len)) = *fence {
        if marker.is_some_and(|(m, n)| m == mark && n >= len) {
            *fence = None;
        }
        true
    } else if let Some(marker) = marker {
        *fence = Some(marker);
        true
    } else {
        false
    }
}
