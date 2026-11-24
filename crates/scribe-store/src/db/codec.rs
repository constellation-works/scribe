//! The current-format codecs of the store's records.
//!
//! Bytes in, a record out: no I/O (the paths below only name the record in
//! an error). Kept apart from the facade so the format rules (STD-02 R16,
//! STD-03 R10, R23) read in one place and are tested without a filesystem.
//!
//! Since phase 8 (docs/design/scribe/2_design.md §15.9) nothing reads a file
//! store: the records live in `scribe.db`, and the files of the file store
//! are archive rows. What is left here:
//!
//! - the export encoders: an entry or a session record as Markdown (a `---`
//!   line, YAML frontmatter carrying `format` and `kind`, a `---` line, the
//!   body) and `control.json`, `proposals.json`, `tags.json`, `marks.json`
//!   and `pickups.json` as `{"format": N, ...}`;
//! - the checks a `scribe.db` row must pass as it is read or written;
//! - the session record decoder, for a rebuild checkpoint row, whose text is
//!   a session record;
//! - the stamp check: a JSON document's `format` read first, so a store a
//!   newer build wrote is refused by version;
//! - the memory files' codec (`now` items and milestones).

use crate::Author;
use crate::entry::Entry;
use crate::error::{Error, Result};
use crate::proposal::Proposal;
use crate::proposal::{ProposalKind, ProposalState};
use crate::session::{
    ControlState, HEADER_SECTIONS, LOG_SECTION, SessionControl, SessionId, SessionRecord, check_log,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU32;
use std::path::Path;

/// The newest store format: the format this build writes in exports and
/// stamps, and the newest it accepts.
pub(crate) const FORMAT: u32 = 31;

/// What the frontmatter's `kind` says a Markdown record is.
const ENTRY_KIND: &str = "entry";
const SESSION_KIND: &str = "session";

/// A shape problem found after parsing: a missing delimiter, a section out
/// of order, a record whose id does not match its file.
#[derive(Debug)]
pub(crate) struct ShapeError(pub(crate) String);

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ShapeError {}

pub(crate) fn shape(path: &Path, message: impl Into<String>) -> Error {
    Error::corrupt(path, ShapeError(message.into()))
}

/// Just the format number, read before the full document. Format 0 never
/// existed, so it reads as corrupt.
#[derive(Deserialize)]
struct FormatProbe {
    format: NonZeroU32,
}

/// Refuse a newer format; return the one found.
fn check_format(path: &Path, probe: std::result::Result<FormatProbe, Error>) -> Result<u32> {
    let found = probe?.format.get();
    if found > FORMAT {
        return Err(Error::NewerFormat {
            path: path.to_path_buf(),
            found,
            supported: FORMAT,
        });
    }
    Ok(found)
}

/// Split a Markdown record into its frontmatter and its body. The file
/// starts with a `---` line; the next `---` line closes the frontmatter.
/// The body is everything after it, less the one line break that ends the
/// file.
pub(crate) fn split_frontmatter<'a>(path: &Path, bytes: &'a [u8]) -> Result<(&'a str, &'a str)> {
    let text = std::str::from_utf8(bytes).map_err(|e| Error::corrupt(path, e))?;
    let missing = || shape(path, "expected YAML frontmatter between two '---' lines");
    let rest = text.strip_prefix("---\n").ok_or_else(missing)?;
    let (front, body) = if let Some(body) = rest.strip_prefix("---\n") {
        ("", body)
    } else if rest == "---" {
        ("", "")
    } else {
        let close = rest.find("\n---\n").map(|at| (at, at + 5));
        let close = close.or_else(|| {
            rest.ends_with("\n---")
                .then(|| (rest.len() - 4, rest.len()))
        });
        let (end, body_start) = close.ok_or_else(missing)?;
        (&rest[..=end], &rest[body_start..])
    };
    Ok((front, body.strip_suffix('\n').unwrap_or(body)))
}

/// Parse frontmatter: check `format` and `kind`, returning the file's format
/// and the remaining mapping as a YAML value.
fn parse_frontmatter(path: &Path, front: &str, kind: &str) -> Result<(u32, yaml_serde::Value)> {
    let mut map: yaml_serde::Mapping =
        yaml_serde::from_str(front).map_err(|e| Error::corrupt(path, e))?;
    let probe = yaml_serde::from_value::<FormatProbe>(yaml_serde::Value::Mapping(map.clone()))
        .map_err(|e| Error::corrupt(path, e));
    let from = check_format(path, probe)?;
    map.remove("format");
    match map.remove("kind") {
        Some(yaml_serde::Value::String(found)) if found == kind => {}
        Some(other) => {
            let found = yaml_serde::to_string(&other).unwrap_or_default();
            return Err(shape(
                path,
                format!("kind is {}, expected {kind}", found.trim()),
            ));
        }
        None => return Err(shape(path, "missing field `kind`")),
    }
    Ok((from, yaml_serde::Value::Mapping(map)))
}

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

/// Parse a session record's text, as a rebuild checkpoint row holds it:
/// `expected` is the record's id. The body is the header and the log, which
/// must agree with the frontmatter's `log`.
pub(crate) fn decode_session(path: &Path, bytes: &[u8], expected: &str) -> Result<SessionRecord> {
    let (front, body) = split_frontmatter(path, bytes)?;
    let (from, value) = parse_frontmatter(path, front, SESSION_KIND)?;
    if from < SESSION_SINCE {
        return Err(shape(
            path,
            format!("a session record has format {SESSION_SINCE} or later, not {from}"),
        ));
    }
    let mut record: SessionRecord =
        yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?;
    if record.id.as_str() != expected {
        return Err(shape(
            path,
            format!("id {} does not match the record {expected}", record.id),
        ));
    }
    let (header, texts) = split_body(path, body, &record)?;
    let [summary, goals, next] = header;
    record.summary = summary;
    record.goals = goals;
    record.next = next;
    for (segment, text) in record.log.iter_mut().zip(texts) {
        segment.text = text;
    }
    check_log(&record).map_err(|e| Error::corrupt(path, e))?;
    Ok(record)
}

/// The oldest session record [`decode_session`] reads: format 6 has the
/// current frontmatter.
const SESSION_SINCE: u32 = 6;

/// Refuse a JSON document whose `format` is newer than this build's: a
/// stamp a newer build left. Reads only the `format`.
pub(crate) fn check_stamp(path: &Path, bytes: &[u8]) -> Result<()> {
    check_format(
        path,
        serde_json::from_slice(bytes).map_err(|e| Error::corrupt(path, e)),
    )
    .map(drop)
}

/// The frontmatter of a session record as written.
#[derive(Serialize)]
struct SessionFront<'a> {
    format: u32,
    kind: &'static str,
    #[serde(flatten)]
    record: &'a SessionRecord,
}

/// A session record file's bytes at the current format: the frontmatter
/// between two `---` lines, the header's three sections, then `## Log` with
/// one `### <n>. <heading>` and its text per segment. [`decode_session`]
/// reads it back to the same record. A record that would not read back (a
/// broken log, or text holding a line that would read as a heading) is
/// refused rather than written.
pub fn encode_session(path: &Path, record: &SessionRecord) -> Result<Vec<u8>> {
    check_log(record).map_err(|e| Error::corrupt(path, e))?;
    let front = yaml_serde::to_string(&SessionFront {
        format: FORMAT,
        kind: SESSION_KIND,
        record,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    let mut text = String::from("---\n");
    text.push_str(&front);
    if !front.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("---\n");
    let bodies = [&record.summary, &record.goals, &record.next];
    for (name, body) in HEADER_SECTIONS.iter().zip(bodies) {
        if body.lines().any(top_heading) {
            return Err(shape(
                path,
                format!("## {name} holds a line that would read as a section heading"),
            ));
        }
        text.push_str("## ");
        text.push_str(name);
        text.push('\n');
        if !body.is_empty() {
            text.push_str(body);
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str("## ");
    text.push_str(LOG_SECTION);
    text.push('\n');
    for (i, segment) in record.log.iter().enumerate() {
        if segment.text.lines().any(|l| l.starts_with("### ")) {
            return Err(shape(
                path,
                format!(
                    "segment {} holds a '### ' line, which would read as a heading",
                    segment.n
                ),
            ));
        }
        if i > 0 {
            text.push('\n');
        }
        text.push_str(&format!("### {}. {}\n", segment.n, segment.heading));
        if !segment.text.is_empty() {
            text.push_str(&segment.text);
            text.push('\n');
        }
    }
    Ok(text.into_bytes())
}

/// Whether `line` is one of the format-4 body's `## ` section headings.
fn top_heading(line: &str) -> bool {
    line.strip_prefix("## ")
        .is_some_and(|h| h == LOG_SECTION || HEADER_SECTIONS.contains(&h))
}

/// A format-4 body: the header's three sections and each segment's text,
/// in `record.log`'s order. Only the exact heading lines `## Summary`,
/// `## Goals`, `## Next` and `## Log`, in that order, delimit the header;
/// under `## Log` every `### ` line must be the next segment's heading as
/// `log` has it, and text before the first is refused. Blank lines around
/// a text are dropped.
fn split_body(
    path: &Path,
    body: &str,
    record: &SessionRecord,
) -> Result<([String; 3], Vec<String>)> {
    let names = [
        HEADER_SECTIONS[0],
        HEADER_SECTIONS[1],
        HEADER_SECTIONS[2],
        LOG_SECTION,
    ];
    let mut header: [Vec<&str>; 3] = Default::default();
    let mut current: Option<usize> = None;
    let mut lines = body.lines();
    for line in lines.by_ref() {
        let next = current.map_or(0, |i| i + 1);
        if names
            .get(next)
            .is_some_and(|name| line.strip_prefix("## ") == Some(name))
        {
            if next == 3 {
                current = Some(3);
                break;
            }
            current = Some(next);
            continue;
        }
        if line == "## Outcome" {
            return Err(shape(
                path,
                "a format-4 session body has no '## Outcome': what happened is in '## Log'",
            ));
        }
        match current {
            Some(i) => header[i].push(line),
            None if line.trim().is_empty() => {}
            None => return Err(shape(path, "the body must start with '## Summary'")),
        }
    }
    if current != Some(3) {
        let missing = names[current.map_or(0, |i| i + 1)];
        return Err(shape(
            path,
            format!(
                "missing '## {missing}': a session body has ## Summary, ## Goals, ## Next and ## Log, in that order"
            ),
        ));
    }
    let mut texts: Vec<Vec<&str>> = Vec::with_capacity(record.log.len());
    for line in lines {
        if line.starts_with("### ") {
            let Some(segment) = record.log.get(texts.len()) else {
                return Err(shape(
                    path,
                    format!(
                        "'{line}' under ## Log starts no segment: log lists {} segments",
                        record.log.len()
                    ),
                ));
            };
            let wanted = format!("### {}. {}", segment.n, segment.heading);
            if line != wanted {
                return Err(shape(
                    path,
                    format!("'{line}' under ## Log disagrees with log: expected '{wanted}'"),
                ));
            }
            texts.push(Vec::new());
            continue;
        }
        match texts.last_mut() {
            Some(text) => text.push(line),
            None if line.trim().is_empty() => {}
            None => {
                return Err(shape(
                    path,
                    "text under ## Log before the first '### <n>. <heading>' line",
                ));
            }
        }
    }
    if texts.len() != record.log.len() {
        return Err(shape(
            path,
            format!(
                "log lists {} segments but ## Log has {} headings",
                record.log.len(),
                texts.len()
            ),
        ));
    }
    let trim = |lines: Vec<&str>| lines.join("\n").trim_matches('\n').to_owned();
    Ok((header.map(trim), texts.into_iter().map(trim).collect()))
}

/// `control.json` as written.
#[derive(Serialize)]
struct ControlOut<'a> {
    codex_cursors: &'a BTreeMap<SessionId, String>,
    format: u32,
    sessions: &'a BTreeMap<SessionId, SessionControl>,
}

/// `control.json`'s bytes at the current format, one line break at the end.
/// [`decode_control`] reads it back to the same state.
pub(crate) fn encode_control(path: &Path, state: &ControlState) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(&ControlOut {
        format: FORMAT,
        sessions: &state.sessions,
        codex_cursors: &state.codex_cursors,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Refuse proposals whose fields disagree, as `proposals.json` is read: a
/// file's and a `scribe.db` row's alike.
pub(crate) fn check_proposals(path: &Path, proposals: &[Proposal]) -> Result<()> {
    for proposal in proposals {
        // A context (format 24) is scribe's paragraph of either speaker's
        // turn; a statement (formats 14 to 23) its line of the user's turn.
        // A claim carries at most one of them, with its subject beside it.
        // A forgotten candidate has neither.
        if let Some(candidate) = &proposal.candidate {
            let stated = candidate.context.is_some() || candidate.statement.is_some();
            if (candidate.context.is_some() && candidate.statement.is_some())
                || stated != candidate.subject.is_some()
                || (candidate.statement.is_some() && candidate.role != crate::Speaker::User)
            {
                return Err(shape(
                    path,
                    "candidate context or statement, never both, must be set with its subject; a statement only on a user candidate",
                ));
            }
        }
        // Only a closed proposal carries a reason scribe closed it.
        if proposal.closed_reason.is_some() && proposal.state != ProposalState::Rejected {
            return Err(shape(
                path,
                "closed_reason on a proposal that is not rejected",
            ));
        }
        // Instruction metadata belongs to a settled candidate (§12.1), and
        // `closed_reason: instructed` is exactly an agent's instructed
        // rejection. Other reasons (`tag_retired`) stay readable on any
        // rejected proposal, as the format-11 reader accepted them.
        match &proposal.instruction {
            Some(inst) => {
                if proposal.state == ProposalState::Open {
                    return Err(shape(path, "instruction on an open proposal"));
                }
                if proposal.kind != ProposalKind::Candidate {
                    return Err(shape(
                        path,
                        "instruction on a proposal that is not a candidate",
                    ));
                }
                if proposal.settled_at.is_none() {
                    return Err(shape(path, "instruction on a proposal without settled_at"));
                }
                if !has_matching_instruction(proposal, inst) {
                    return Err(shape(
                        path,
                        "proposal instruction requires an agent settlement and matching settled_by cite",
                    ));
                }
                if proposal.state == ProposalState::Rejected
                    && !matches!(
                        proposal.closed_reason,
                        Some(crate::ClosedReason::Instructed | crate::ClosedReason::Covered)
                    )
                {
                    return Err(shape(
                        path,
                        "rejected proposal with instruction requires closed_reason instructed or covered",
                    ));
                }
            }
            None if proposal.closed_reason == Some(crate::ClosedReason::Instructed) => {
                return Err(shape(
                    path,
                    "closed_reason instructed requires an agent settlement and matching instruction cite",
                ));
            }
            None => {}
        }
        // `covered` (format 15) closes an approved candidate whose entry an
        // active one covers; it keeps the approval's settlement.
        if proposal.closed_reason == Some(crate::ClosedReason::Covered)
            && (proposal.kind != ProposalKind::Candidate
                || proposal.settled_at.is_none()
                || proposal.settled_author.is_none())
        {
            return Err(shape(
                path,
                "closed_reason covered requires a settled candidate proposal",
            ));
        }
        match (&proposal.kind, &proposal.candidate) {
            (ProposalKind::Candidate, Some(candidate))
                if proposal.newer.is_none()
                    && proposal.older.tag() == &candidate.tag
                    && ((candidate.has_valid_provenance()
                        && (!candidate.quote.is_empty()
                            || proposal.state == ProposalState::Approved))
                        || (candidate.cite.is_deleted()
                            && proposal.state != ProposalState::Open
                            && candidate.quote.is_empty()
                            && candidate.stated().is_none()
                            && candidate.evidence.is_none()
                            && candidate.admitted.is_none()))
                    && candidate.quote.chars().count() <= 400 => {}
            (ProposalKind::Candidate, Some(_) | None)
            | (ProposalKind::Supersedes | ProposalKind::Stale, Some(_)) => {
                return Err(shape(path, "invalid candidate proposal payload"));
            }
            (ProposalKind::Supersedes | ProposalKind::Stale, None) => {}
        }
    }
    Ok(())
}

fn has_matching_instruction(proposal: &Proposal, inst: &crate::Instruction) -> bool {
    !inst.message.trim().is_empty()
        && inst.quote.as_deref().is_none_or(|q| !q.trim().is_empty())
        && proposal
            .settled_author
            .as_ref()
            .is_some_and(|a| a.kind == crate::AuthorKind::Agent)
        && proposal.settled_by.as_ref().is_some_and(|cite| {
            cite.session == inst.session
                && cite.message.as_deref() == Some(inst.message.as_str())
                && cite.at == inst.at
        })
}

/// `proposals.json` as written.
#[derive(Serialize)]
struct ProposalsOut<'a> {
    format: u32,
    proposals: &'a [Proposal],
}

/// `proposals.json`'s bytes at the current format, by id, one line break at
/// the end. [`decode_proposals`] reads it back to the same proposals.
pub(crate) fn encode_proposals(path: &Path, proposals: &[Proposal]) -> Result<Vec<u8>> {
    let mut sorted = proposals.to_vec();
    sorted.sort_by_key(|p| p.id);
    let mut bytes = serde_json::to_vec_pretty(&ProposalsOut {
        format: FORMAT,
        proposals: &sorted,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TagsDocument {
    format: u32,
    tags: Vec<crate::StoreTag>,
}

/// Refuse store tags that clash or lack agent authors: a file's and a
/// `scribe.db` row's alike.
pub(crate) fn check_tags(path: &Path, tags: &[crate::StoreTag]) -> Result<()> {
    crate::TagTable::new(tags.iter().map(crate::StoreTag::definition).collect())
        .map_err(|e| Error::corrupt(path, e))?;
    if tags.iter().any(|t| {
        t.author.kind != crate::AuthorKind::Agent
            || t.updated_by
                .as_ref()
                .is_some_and(|a| a.kind != crate::AuthorKind::Agent)
    }) {
        return Err(shape(path, "store tags must have agent authors"));
    }
    Ok(())
}

pub(crate) fn encode_tags(path: &Path, tags: &[crate::StoreTag]) -> Result<Vec<u8>> {
    let mut tags = tags.to_vec();
    tags.sort_by(|a, b| a.name.cmp(&b.name));
    let mut bytes = serde_json::to_vec_pretty(&TagsDocument {
        format: FORMAT,
        tags,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PickupsDocument {
    format: u32,
    pickups: Vec<crate::Pickup>,
}

/// Refuse a pickup without an agent author: a file's and a row's alike.
pub(crate) fn check_pickups(path: &Path, pickups: &[crate::Pickup]) -> Result<()> {
    if pickups
        .iter()
        .any(|p| p.author.kind != crate::AuthorKind::Agent)
    {
        return Err(shape(path, "pickups must have agent authors"));
    }
    Ok(())
}

/// `pickups.json`'s bytes at the current format, oldest first.
pub(crate) fn encode_pickups(path: &Path, pickups: &[crate::Pickup]) -> Result<Vec<u8>> {
    let mut pickups = pickups.to_vec();
    pickups.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.entry.cmp(&b.entry)));
    let mut bytes = serde_json::to_vec_pretty(&PickupsDocument {
        format: FORMAT,
        pickups,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarksDocument {
    format: u32,
    marks: Vec<crate::annotation::MarkRecord>,
}

/// Refuse a mark whose fields disagree with its outcome: a file's and a
/// row's alike.
pub(crate) fn check_marks(path: &Path, marks: &[crate::annotation::MarkRecord]) -> Result<()> {
    if let Some(bad) = marks.iter().find(|m| !m.is_consistent()) {
        return Err(shape(
            path,
            format!(
                "mark {}#{} has fields that disagree with its outcome {}",
                bad.message,
                bad.marker,
                bad.outcome.as_str()
            ),
        ));
    }
    Ok(())
}

/// `marks.json`'s bytes at the current format, oldest first.
pub(crate) fn encode_marks(
    path: &Path,
    marks: &[crate::annotation::MarkRecord],
) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(&MarksDocument {
        format: FORMAT,
        marks: marks.to_vec(),
    })
    .map_err(|e| Error::corrupt(path, e))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The body of a `now` or milestone file: `# <title>`, a blank line, then
/// its prose on one line.
fn memory_body(title: &str, text: &str) -> String {
    format!("# {title}\n\n{text}\n")
}

/// The title and prose of a `now` or milestone file's body.
fn split_memory_body(path: &Path, body: &str) -> Result<(String, String)> {
    let bad = || {
        shape(
            path,
            "a memory file's body is '# <title>', a blank line, then its text",
        )
    };
    let rest = body.strip_prefix("# ").ok_or_else(bad)?;
    let (title, text) = rest.split_once('\n').ok_or_else(bad)?;
    let title = title.trim();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty()
        || title.chars().count() > scribe_domain::memory::TITLE_CHARS
        || text.is_empty()
        || text.chars().count() > scribe_domain::memory::TEXT_CHARS
    {
        return Err(bad());
    }
    Ok((title.to_owned(), text))
}

fn memory_front<T: serde::de::DeserializeOwned>(path: &Path, bytes: &[u8]) -> Result<(T, String)> {
    let (front, body) = split_frontmatter(path, bytes)?;
    let value: T = yaml_serde::from_str(front).map_err(|e| Error::corrupt(path, e))?;
    Ok((value, body.to_owned()))
}

fn stem_of(path: &Path, stem: &str) -> Result<scribe_domain::memory::Stem> {
    stem.parse().map_err(|e: String| shape(path, e))
}

/// Parse `memory/now/<stem>.md`.
pub(crate) fn decode_now(
    path: &Path,
    bytes: &[u8],
    stem: &str,
) -> Result<scribe_domain::memory::NowItem> {
    let stem = stem_of(path, stem)?;
    let (front, body): (scribe_domain::memory::NowFront, String) = memory_front(path, bytes)?;
    let (title, text) = split_memory_body(path, &body)?;
    if front.sources.is_empty() {
        return Err(shape(path, "a now item names at least one source session"));
    }
    Ok(scribe_domain::memory::NowItem {
        stem,
        date: front.date,
        kind: front.kind,
        sources: front.sources,
        updated: front.updated,
        raised: front.raised,
        title,
        text,
    })
}

/// A `now` file's bytes.
pub(crate) fn encode_now(path: &Path, item: &scribe_domain::memory::NowItem) -> Result<String> {
    let front = yaml_serde::to_string(&scribe_domain::memory::NowFront {
        date: item.date,
        kind: item.kind,
        sources: item.sources.clone(),
        updated: item.updated,
        raised: item.raised.clone(),
    })
    .map_err(|e| Error::corrupt(path, e))?;
    Ok(format!(
        "---\n{front}---\n{}",
        memory_body(&item.title, &item.text)
    ))
}

/// Parse `memory/milestones/<stem>.md`.
pub(crate) fn decode_milestone(
    path: &Path,
    bytes: &[u8],
    stem: &str,
) -> Result<scribe_domain::memory::Milestone> {
    let stem = stem_of(path, stem)?;
    let (front, body): (scribe_domain::memory::MilestoneFront, String) = memory_front(path, bytes)?;
    let (title, text) = split_memory_body(path, &body)?;
    Ok(scribe_domain::memory::Milestone {
        stem,
        date: front.date,
        sources: front.sources,
        updated: front.updated,
        title,
        text,
    })
}

/// A milestone file's bytes.
pub(crate) fn encode_milestone(
    path: &Path,
    item: &scribe_domain::memory::Milestone,
) -> Result<String> {
    let front = yaml_serde::to_string(&scribe_domain::memory::MilestoneFront {
        date: item.date,
        sources: item.sources.clone(),
        updated: item.updated,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    Ok(format!(
        "---\n{front}---\n{}",
        memory_body(&item.title, &item.text)
    ))
}
