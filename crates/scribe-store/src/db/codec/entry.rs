//! Entry invariant checks for SQLite and the entry Markdown codec.

use super::{FORMAT, shape};
#[cfg(feature = "test-support")]
use super::{parse_frontmatter, split_frontmatter};
use crate::Author;
use crate::entry::Entry;
use crate::error::{Error, Result};
use serde::Serialize;
use std::path::Path;

const ENTRY_KIND: &str = "entry";

/// An entry row with its content `body`, checked: its id and tag match
/// `expected` and `tag_dir` (the row's id and tag), and its fields agree
/// with each other.
pub(crate) fn checked_entry(
    path: &Path,
    mut entry: Entry,
    body: &str,
    expected: &str,
    tag_dir: &str,
) -> Result<Entry> {
    validate_entry(path, &entry, body, expected, tag_dir)?;
    body.clone_into(&mut entry.content);
    Ok(entry)
}

/// Validate the same entry invariants for both reads and writes.
pub(crate) fn validate_entry(
    path: &Path,
    entry: &Entry,
    body: &str,
    expected: &str,
    tag_dir: &str,
) -> Result<()> {
    if entry.id.to_string() != expected
        || entry.tag.as_str() != tag_dir
        || entry.id.tag() != &entry.tag
    {
        return Err(shape(
            path,
            format!(
                "id {} and tag {} do not match the file entries/{tag_dir}/{expected}.md",
                entry.id, entry.tag
            ),
        ));
    }
    // A claim approval's `said`: the typed `#scribe approve p-<n>` line, or
    // (format 15) the canonical `proposal settle` command an agent ran on
    // Daniel's checked instruction, only beside its `approved_by`.
    let approval_said = |said: &str| {
        said.starts_with("#scribe approve p-")
            || (entry.approved_by.is_some()
                && said.starts_with(crate::proposal::INSTRUCTED_SAID_PREFIX))
    };
    let valid = match entry.author.kind {
        crate::AuthorKind::User => {
            entry.cite.is_some()
                && entry.content_from != crate::ContentFrom::Agent
                && match &entry.stated_by {
                    // Daniel's own words: no separate quote.
                    None => entry.quote.is_none(),
                    // Scribe's statement of Daniel's turn (format 14): an
                    // approved pass claim from his message, his words in
                    // `quote` until the entry is forgotten.
                    // Scribe's statement of a turn he marked with
                    // `#scribe` (format 22): his words in `quote` and the
                    // marked line in `said` until the entry is forgotten.
                    Some(by) if *by == Author::annotation() => {
                        matches!(
                            entry.content_from,
                            crate::ContentFrom::Message {
                                role: crate::Speaker::User,
                                ..
                            }
                        ) && entry.approved_by.is_none()
                            && match entry.status {
                                crate::EntryStatus::Forgotten => {
                                    entry.quote.is_none() && entry.said.is_empty()
                                }
                                crate::EntryStatus::Active
                                | crate::EntryStatus::Superseded
                                | crate::EntryStatus::Retired => {
                                    entry.quote.is_some() && !entry.said.is_empty()
                                }
                            }
                    }
                    Some(by) => {
                        *by == Author::session_pass()
                            && matches!(
                                entry.content_from,
                                crate::ContentFrom::Message {
                                    role: crate::Speaker::User,
                                    ..
                                }
                            )
                            && entry.window.is_none()
                            && match entry.status {
                                crate::EntryStatus::Forgotten => {
                                    entry.quote.is_none() && entry.said.is_empty()
                                }
                                crate::EntryStatus::Active
                                | crate::EntryStatus::Superseded
                                | crate::EntryStatus::Retired => {
                                    entry.quote.is_some() && approval_said(&entry.said)
                                }
                            }
                    }
                }
        }
        crate::AuthorKind::Agent => {
            // A direct CLI note, or (format 10) an assistant turn kept by
            // a typed candidate approval: the cite is that turn, `said`
            // the approval command (or blank, along with the body, once
            // forgotten), and the words are the assistant's.
            let direct = entry.cite.is_none()
                && entry.said.is_empty()
                && entry.content_from == crate::ContentFrom::Agent
                && entry.window.is_none()
                && entry.approved_by.is_none();
            let approved_said = match entry.status {
                crate::EntryStatus::Forgotten => entry.said.is_empty() && body.is_empty(),
                crate::EntryStatus::Active
                | crate::EntryStatus::Superseded
                | crate::EntryStatus::Retired => approval_said(&entry.said),
            };
            let approved_assistant = entry.cite.is_some()
                && approved_said
                && matches!(
                    entry.content_from,
                    crate::ContentFrom::Message {
                        role: crate::Speaker::Assistant,
                        ..
                    }
                )
                && entry.window.is_none();
            // A direct note may carry a quote. A spoken-instruction entry
            // records the agent's statement in content, the user's words in
            // `quote` (until forgotten), and `stated_by` as the writing
            // (or content-updating) agent. An approved assistant turn
            // keeps the assistant's words as content, no quote; or (format 24,
            // 2_design.md §15.3) scribe's context of the turn as content,
            // the assistant's words in `quote` until it is forgotten.
            let spoken_stated = entry.stated_by.as_ref().is_some_and(|by| {
                by.kind == crate::AuthorKind::Agent
                    && (by == &entry.author
                        || entry
                            .updated_by
                            .as_ref()
                            .is_some_and(|updater| updater.kind == crate::AuthorKind::Agent))
            }) && entry.evidence.is_none()
                && match entry.status {
                    crate::EntryStatus::Forgotten => entry.quote.is_none(),
                    crate::EntryStatus::Active
                    | crate::EntryStatus::Superseded
                    | crate::EntryStatus::Retired => entry.quote.is_some(),
                };
            let words = entry.stated_by.is_none() && entry.quote.is_none();
            let context = entry.stated_by.as_ref() == Some(&Author::session_pass())
                && (entry.status == crate::EntryStatus::Forgotten) == entry.quote.is_none();
            (direct && (entry.stated_by.is_none() || spoken_stated))
                || (approved_assistant && (words || context))
        }
        // An entry a write-route catcher had the session pass write itself
        // (format 16, 2_design.md §15.2): scribe's statement of one turn as
        // content, that turn's words in `quote` until it is forgotten,
        // nothing typed, and an outcome. The turn is the user's or, under a
        // catcher whose `from` reads the assistant, the assistant's.
        crate::AuthorKind::Scribe => {
            entry.author == Author::session_pass()
                && entry.stated_by.as_ref() == Some(&Author::session_pass())
                && entry.cite.is_some()
                && matches!(entry.content_from, crate::ContentFrom::Message { .. })
                && entry.said.is_empty()
                && entry.window.is_none()
                && entry.evidence.is_none()
                && entry.approved_by.is_none()
                && entry.outcome.is_some()
                && match entry.status {
                    crate::EntryStatus::Forgotten => entry.quote.is_none(),
                    crate::EntryStatus::Active
                    | crate::EntryStatus::Superseded
                    | crate::EntryStatus::Retired => entry.quote.is_some(),
                }
        }
    };
    let forgotten = entry.status == crate::EntryStatus::Forgotten;
    if entry.decision.is_some() || entry.item.is_some() {
        let typed_tag_matches = match (&entry.decision, &entry.item) {
            (Some(_), None) => entry.tag.as_str() == crate::entry::DECISION_TAG,
            (None, Some(_)) => entry.tag.as_str() == crate::entry::OPEN_ITEM_TAG,
            _ => false,
        };
        if !typed_tag_matches
            || forgotten
            || entry.author.kind != crate::AuthorKind::Agent
            || entry.content_from != crate::ContentFrom::Agent
        {
            return Err(shape(
                path,
                "decision and item objects require a direct agent entry of the matching tag",
            ));
        }
    }
    let outcome_valid = match &entry.outcome {
        None => entry.author.kind != crate::AuthorKind::Scribe,
        Some(outcome) => {
            entry.author.kind == crate::AuthorKind::Scribe && outcome.is_consistent(forgotten)
        }
    };
    if !outcome_valid {
        return Err(shape(
            path,
            "entry outcome must be set exactly on an idea the session pass wrote, with fields matching its state",
        ));
    }
    let active = entry.status == crate::EntryStatus::Active;
    if active != entry.inactive_since.is_none() {
        return Err(shape(
            path,
            "inactive_since must be null exactly while the entry is active",
        ));
    }
    if !valid {
        return Err(shape(
            path,
            "entry author, said, content_from, cite, quote, window and stated_by disagree",
        ));
    }
    if let Some(approved_by) = &entry.approved_by {
        if approved_by.kind != crate::AuthorKind::Agent {
            return Err(shape(path, "entry approved_by must have kind agent"));
        }
        if entry.instruction.is_none() {
            return Err(shape(path, "entry approved_by requires an instruction"));
        }
    }
    if entry.instruction.is_some() && entry.approved_by.is_none() {
        return Err(shape(
            path,
            "entry instruction is an approval's and requires approved_by",
        ));
    }
    if entry.removal_instruction.is_some()
        && !matches!(
            entry.status,
            crate::EntryStatus::Retired | crate::EntryStatus::Forgotten
        )
    {
        return Err(shape(
            path,
            "entry removal_instruction requires a retired or forgotten entry",
        ));
    }
    for (key, inst) in [
        ("instruction", &entry.instruction),
        ("removal_instruction", &entry.removal_instruction),
    ] {
        let Some(inst) = inst else { continue };
        if inst.message.trim().is_empty() {
            return Err(shape(
                path,
                format!("entry {key} message must not be empty"),
            ));
        }
        if inst.quote.as_deref().is_some_and(|q| q.trim().is_empty()) {
            return Err(shape(path, format!("entry {key} quote must not be empty")));
        }
    }
    Ok(())
}

/// The frontmatter as written: `format` and `kind` first, then the
/// record's own keys.
#[derive(Serialize)]
struct EntryFront<'a> {
    format: u32,
    kind: &'static str,
    #[serde(flatten)]
    entry: &'a Entry,
}

/// An entry file's bytes at the current format: the frontmatter between two
/// `---` lines, then the content and one line break. [`decode_entry`] reads
/// it back to the same entry.
pub fn encode_entry(path: &Path, entry: &Entry) -> Result<Vec<u8>> {
    let front = yaml_serde::to_string(&EntryFront {
        format: FORMAT,
        kind: ENTRY_KIND,
        entry,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    let mut text = String::with_capacity(front.len() + entry.content.len() + 10);
    text.push_str("---\n");
    text.push_str(&front);
    if !front.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("---\n");
    text.push_str(&entry.content);
    text.push('\n');
    Ok(text.into_bytes())
}

/// Parse a current-format entry's Markdown (`format: 31` frontmatter and
/// the content as the body), for the test-support seeding only: the store
/// reads entries from `scribe.db` rows, never from Markdown.
#[cfg(feature = "test-support")]
pub(crate) fn decode_entry(path: &Path, bytes: &[u8]) -> Result<Entry> {
    let (front, body) = split_frontmatter(path, bytes)?;
    let (from, value) = parse_frontmatter(path, front, ENTRY_KIND)?;
    if from != FORMAT {
        return Err(shape(
            path,
            format!("an entry has format {FORMAT}, not {from}"),
        ));
    }
    let entry: Entry = yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?;
    let (id, tag) = (entry.id.to_string(), entry.tag.as_str().to_owned());
    checked_entry(path, entry, body, &id, &tag)
}
