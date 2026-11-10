//! The persisted documents and their format versions.
//!
//! Bytes in, a record out: no I/O (the paths below only name the file in an
//! error). Kept apart from the facade so the format rules (STD-02 R16,
//! STD-03 R10, R23) read in one place and are tested without a filesystem.
//!
//! Two shapes:
//!
//! - Markdown records, an entry or a session record: a `---` line, YAML
//!   frontmatter, a `---` line, then the body. The frontmatter carries
//!   `format` and `kind` beside the record's own keys; every key is required
//!   and an unknown one is refused.
//! - JSON documents, `control.json` and `proposals.json`:
//!   `{"format": 4, ...}`, equally strict.
//!
//! A format-4 session record's body is a header (`## Summary`, `## Goals`,
//! `## Next`) and a log (`## Log`, one `### <n>. <heading>` per segment);
//! the frontmatter's `log` list must agree with it. Older records have four
//! sections (`## Outcome` third) and are converted on read.
//!
//! Every document's `format` is read first, so a newer one is refused by
//! version, not by whatever parse error its new shape would cause.

use crate::entry::Entry;
use crate::entry::{Cite, EntryId};
use crate::error::{Error, Result};
use crate::proposal::Proposal;
use crate::proposal::{ProposalId, ProposalKind, ProposalState, Quote};
use crate::session::{
    ControlState, HEADER_SECTIONS, LOG_SECTION, SEGMENT_HEADING_CHARS, SessionControl, SessionId,
    SessionLink, SessionRecord, check_log,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU32;
use std::path::Path;

/// The newest store format: this build reads formats 1 to `FORMAT` of every
/// document and would write `FORMAT`.
pub(super) const FORMAT: u32 = 5;

/// Which document an upgrade step is applied to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Document {
    /// An entry's frontmatter (without `format` and `kind`).
    Entry,
    /// A session record's frontmatter (without `format` and `kind`).
    Session,
    /// `control.json`.
    Control,
    /// `proposals.json`.
    Proposals,
}

/// One upgrade step: a format `n` document in, the format `n + 1` document
/// out, as JSON whatever the file's syntax.
pub(super) type Upgrade = fn(Document, serde_json::Value) -> serde_json::Result<serde_json::Value>;

/// The upgrade registry, append-only (STD-03 R23). `UPGRADES[i]` turns
/// format `i + 1` into format `i + 2` for every document, so `FORMAT` is
/// always `UPGRADES.len() + 1` (a unit test checks). Every change to a
/// persisted shape bumps `FORMAT` and appends a step here, so an older build
/// refuses the new format instead of dropping fields it does not know
/// (STD-02 R16, STD-03 R10). A shipped step is never edited, reordered or
/// removed. Reads upgrade in memory only; the next write persists the
/// current format. The template's `notes.json` is not carried over.
pub(super) const UPGRADES: &[Upgrade] =
    &[retry_state, session_links, log_active_author, agent_writes];

/// Format 4 -> 5 permits agent provenance and adds retirement reasons and proposal updaters.
fn agent_writes(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Entry
        && let Some(entry) = value.as_object_mut()
    {
        entry
            .entry("retirement_reason")
            .or_insert(serde_json::Value::Null);
    }
    if document == Document::Proposals {
        for proposal in value
            .get_mut("proposals")
            .and_then(serde_json::Value::as_array_mut)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_object_mut)
        {
            proposal
                .entry("updated_by")
                .or_insert(serde_json::Value::Null);
        }
    }
    Ok(value)
}

/// Format 1 -> 2 adds bounded retry state to each session switch. Other
/// document shapes are unchanged. Reads upgrade only in memory.
fn retry_state(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Control
        && let Some(sessions) = value
            .get_mut("sessions")
            .and_then(serde_json::Value::as_object_mut)
    {
        for session in sessions
            .values_mut()
            .filter_map(serde_json::Value::as_object_mut)
        {
            session
                .entry("consecutive_failures")
                .or_insert(serde_json::json!(0));
            session
                .entry("retry_after")
                .or_insert(serde_json::Value::Null);
        }
    }
    Ok(value)
}

/// Format 2 -> 3 adds session links: an empty `links` on each session
/// record and an empty `pending_links` on each session switch. Entries and
/// proposals are unchanged. Reads upgrade only in memory.
fn session_links(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    match document {
        Document::Session => {
            if let Some(record) = value.as_object_mut() {
                record.entry("links").or_insert(serde_json::json!([]));
            }
        }
        Document::Control => {
            if let Some(sessions) = value
                .get_mut("sessions")
                .and_then(serde_json::Value::as_object_mut)
            {
                for session in sessions
                    .values_mut()
                    .filter_map(serde_json::Value::as_object_mut)
                {
                    session
                        .entry("pending_links")
                        .or_insert(serde_json::json!([]));
                }
            }
        }
        Document::Entry | Document::Proposals => {}
    }
    Ok(value)
}

/// Format 3 -> 4 (2_design.md §3; 4_decisions.md, "Store format 4 carries
/// the log, active and author in one step"): a session record becomes a
/// header over one sealed segment (`origin: upgrade`) and is `active`;
/// every entry is Daniel's (`author: {kind: daniel}`, `updated_by: null`);
/// every proposal is the consistency routine's, and a settled one was
/// settled by Daniel; every control switch has `cataloged_by: null`. The
/// segment's text, the old summary and outcome, comes from the body, which
/// [`decode_session`] reads in the old shape. Reads upgrade only in memory.
fn log_active_author(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    use serde_json::{Value, json};
    match document {
        Document::Session => {
            if let Some(record) = value.as_object_mut() {
                let get = |key: &str| record.get(key).cloned().unwrap_or(Value::Null);
                let title = record.get("title").and_then(Value::as_str).unwrap_or("");
                let windows = record
                    .get("passes")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .max(1);
                let segment = json!({
                    "n": 1,
                    "heading": upgrade_heading(title),
                    "first_message": null,
                    "last_message": get("scribed_through"),
                    "from": get("started"),
                    "to": get("updated"),
                    "state": "sealed",
                    "windows": windows,
                    "origin": "upgrade",
                    "topics": record.get("topics").cloned().unwrap_or_else(|| json!([])),
                    "related": record.get("related").cloned().unwrap_or_else(|| json!([])),
                });
                record.entry("active").or_insert(json!(true));
                record.entry("keep_active").or_insert(json!(false));
                record.entry("active_change").or_insert(Value::Null);
                record.entry("cataloged_by").or_insert(Value::Null);
                record.entry("log").or_insert(json!([segment]));
            }
        }
        Document::Entry => {
            if let Some(entry) = value.as_object_mut() {
                entry
                    .entry("author")
                    .or_insert(json!({"kind": "daniel", "name": "daniel"}));
                entry.entry("updated_by").or_insert(Value::Null);
            }
        }
        Document::Proposals => {
            for proposal in value
                .get_mut("proposals")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object_mut)
            {
                let settled = proposal.get("state").and_then(Value::as_str) != Some("open");
                proposal
                    .entry("author")
                    .or_insert(json!({"kind": "scribe", "name": "consistency"}));
                proposal.entry("settled_author").or_insert(if settled {
                    json!({"kind": "daniel", "name": "daniel"})
                } else {
                    Value::Null
                });
            }
        }
        Document::Control => {
            for session in value
                .get_mut("sessions")
                .and_then(Value::as_object_mut)
                .into_iter()
                .flat_map(|s| s.values_mut())
                .filter_map(Value::as_object_mut)
            {
                session.entry("cataloged_by").or_insert(Value::Null);
            }
        }
    }
    Ok(value)
}

/// An upgraded segment's heading: the record's title on one line, cut to
/// [`SEGMENT_HEADING_CHARS`]; `Session` when the title is empty.
fn upgrade_heading(title: &str) -> String {
    let flat = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = flat.chars().take(SEGMENT_HEADING_CHARS).collect();
    let cut = cut.trim_end().to_owned();
    if cut.is_empty() {
        String::from("Session")
    } else {
        cut
    }
}

/// Frozen format-3 `control.json` shape, validated before upgrading.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlV3 {
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    format: u32,
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    sessions: BTreeMap<SessionId, SwitchV3>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchV3 {
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    on: bool,
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    #[serde(with = "time::serde::rfc3339::option")]
    since: Option<time::OffsetDateTime>,
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    due: bool,
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    #[serde(rename = "final")]
    final_pass: bool,
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    consecutive_failures: u32,
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    #[serde(with = "time::serde::rfc3339::option")]
    retry_after: Option<time::OffsetDateTime>,
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    pending_links: Vec<SessionLink>,
}

/// Frozen format-1-to-3 `proposals.json` shape (unchanged until format 4),
/// validated before upgrading.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalsV3 {
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    format: u32,
    #[allow(dead_code, reason = "format-3 validation before upgrade")]
    proposals: Vec<ProposalV3>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "format-3 validation before upgrade")]
struct ProposalV3 {
    id: ProposalId,
    kind: ProposalKind,
    older: EntryId,
    #[serde(deserialize_with = "crate::record::nullable")]
    newer: Option<EntryId>,
    reason: String,
    quotes: Vec<Quote>,
    state: ProposalState,
    #[serde(with = "time::serde::rfc3339")]
    opened_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    settled_at: Option<time::OffsetDateTime>,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_by: Option<Cite>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalsV4 {
    #[allow(dead_code, reason = "format-4 validation before upgrade")]
    format: u32,
    #[allow(dead_code, reason = "format-4 validation before upgrade")]
    proposals: Vec<ProposalV4>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "format-4 validation before upgrade")]
struct ProposalV4 {
    id: ProposalId,
    kind: ProposalKind,
    older: EntryId,
    #[serde(deserialize_with = "crate::record::nullable")]
    newer: Option<EntryId>,
    reason: String,
    quotes: Vec<Quote>,
    state: ProposalState,
    #[serde(with = "time::serde::rfc3339")]
    opened_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    settled_at: Option<time::OffsetDateTime>,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_by: Option<Cite>,
    author: crate::Author,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_author: Option<crate::Author>,
}

/// Frozen format-2 `control.json` shape, validated before upgrading.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlV2 {
    #[allow(dead_code, reason = "format-2 validation before upgrade")]
    format: u32,
    #[allow(dead_code, reason = "format-2 validation before upgrade")]
    sessions: BTreeMap<SessionId, SwitchV2>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchV2 {
    #[allow(dead_code, reason = "format-2 validation before upgrade")]
    on: bool,
    #[allow(dead_code, reason = "format-2 validation before upgrade")]
    #[serde(with = "time::serde::rfc3339::option")]
    since: Option<time::OffsetDateTime>,
    #[allow(dead_code, reason = "format-2 validation before upgrade")]
    due: bool,
    #[allow(dead_code, reason = "format-2 validation before upgrade")]
    #[serde(rename = "final")]
    final_pass: bool,
    #[allow(dead_code, reason = "format-2 validation before upgrade")]
    consecutive_failures: u32,
    #[allow(dead_code, reason = "format-2 validation before upgrade")]
    #[serde(with = "time::serde::rfc3339::option")]
    retry_after: Option<time::OffsetDateTime>,
}

/// Frozen format-1 shape. Validate before upgrading so unknown or repeated
/// fields remain errors rather than being hidden by a generic JSON value.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlV1 {
    #[allow(dead_code, reason = "format-1 validation before upgrade")]
    format: u32,
    #[allow(dead_code, reason = "format-1 validation before upgrade")]
    sessions: BTreeMap<SessionId, SwitchV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchV1 {
    #[allow(dead_code, reason = "format-1 validation before upgrade")]
    on: bool,
    #[allow(dead_code, reason = "format-1 validation before upgrade")]
    #[serde(with = "time::serde::rfc3339::option")]
    since: Option<time::OffsetDateTime>,
    #[allow(dead_code, reason = "format-1 validation before upgrade")]
    due: bool,
    #[allow(dead_code, reason = "format-1 validation before upgrade")]
    #[serde(rename = "final")]
    final_pass: bool,
}

/// What the frontmatter's `kind` says a Markdown record is.
const ENTRY_KIND: &str = "entry";
const SESSION_KIND: &str = "session";

/// A shape problem found after parsing: a missing delimiter, a section out
/// of order, a record whose id does not match its file.
#[derive(Debug)]
pub(super) struct ShapeError(pub(super) String);

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ShapeError {}

fn shape(path: &Path, message: impl Into<String>) -> Error {
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

/// Apply the upgrade steps past `from`, then parse the result strictly.
fn upgrade<T: DeserializeOwned>(
    path: &Path,
    document: Document,
    from: u32,
    mut value: serde_json::Value,
) -> Result<T> {
    // Step `i` produces format `i + 2`; apply the ones past `from`.
    for (step, to) in UPGRADES.iter().zip(2..) {
        if to > from {
            value = step(document, value).map_err(|e| Error::corrupt(path, e))?;
        }
    }
    serde_json::from_value(value).map_err(|e| Error::corrupt(path, e))
}

/// Split a Markdown record into its frontmatter and its body. The file
/// starts with a `---` line; the next `---` line closes the frontmatter.
/// The body is everything after it, less the one line break that ends the
/// file.
pub(super) fn split_frontmatter<'a>(path: &Path, bytes: &'a [u8]) -> Result<(&'a str, &'a str)> {
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

/// Parse frontmatter: check `format` and `kind`, then the record's own keys
/// strictly. Returns the record (upgraded) and the format the file has.
fn decode_frontmatter<T: DeserializeOwned>(
    path: &Path,
    front: &str,
    document: Document,
    kind: &str,
) -> Result<(T, u32)> {
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
    let value = yaml_serde::Value::Mapping(map);
    if from == FORMAT {
        let record = yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?;
        return Ok((record, from));
    }
    let json = serde_json::to_value(&value).map_err(|e| Error::corrupt(path, e))?;
    Ok((upgrade(path, document, from, json)?, from))
}

/// Parse an entry file. `expected` is the file's stem and `tag_dir` its
/// directory name; the record's id and tag must match both, so a file
/// renamed or moved by hand is reported, not served under the wrong id.
pub(super) fn decode_entry(
    path: &Path,
    bytes: &[u8],
    expected: &str,
    tag_dir: &str,
) -> Result<Entry> {
    let (front, body) = split_frontmatter(path, bytes)?;
    let (mut entry, _): (Entry, u32) =
        decode_frontmatter(path, front, Document::Entry, ENTRY_KIND)?;
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
    let valid = match entry.author.kind {
        crate::AuthorKind::Daniel => {
            entry.cite.is_some() && entry.content_from != crate::ContentFrom::Agent
        }
        crate::AuthorKind::Agent => {
            entry.cite.is_none()
                && entry.said.is_empty()
                && entry.content_from == crate::ContentFrom::Agent
        }
        crate::AuthorKind::Scribe => false,
    };
    if !valid {
        return Err(shape(
            path,
            "entry author, said, content_from and cite disagree",
        ));
    }
    body.clone_into(&mut entry.content);
    Ok(entry)
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
pub(super) fn encode_entry(path: &Path, entry: &Entry) -> Result<Vec<u8>> {
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

/// Parse a session record. `expected` is the file's stem and `year_month`
/// its `<yyyy>/<mm>` directories, which must match the id and `started`.
/// A format-4 body is the header and the log, which must agree with the
/// frontmatter's `log`; an older one has four sections, and its summary and
/// outcome become the text of the upgraded segment.
pub(super) fn decode_session(
    path: &Path,
    bytes: &[u8],
    expected: &str,
    year_month: (&str, &str),
) -> Result<SessionRecord> {
    let (front, body) = split_frontmatter(path, bytes)?;
    let (mut record, from): (SessionRecord, u32) =
        decode_frontmatter(path, front, Document::Session, SESSION_KIND)?;
    let started = (
        format!("{:04}", record.started.year()),
        format!("{:02}", u8::from(record.started.month())),
    );
    if record.id.as_str() != expected || (started.0.as_str(), started.1.as_str()) != year_month {
        return Err(shape(
            path,
            format!(
                "id {} started {}-{} does not match the file sessions/{}/{}/{expected}.md",
                record.id, started.0, started.1, year_month.0, year_month.1
            ),
        ));
    }
    if from < 4 {
        let [summary, goals, outcome, next] = split_sections_v3(path, body)?;
        if let Some(segment) = record.log.first_mut() {
            segment.text = [summary.as_str(), outcome.as_str()]
                .into_iter()
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
        }
        record.summary = summary;
        record.goals = goals;
        record.next = next;
    } else {
        let (header, texts) = split_body(path, body, &record)?;
        let [summary, goals, next] = header;
        record.summary = summary;
        record.goals = goals;
        record.next = next;
        for (segment, text) in record.log.iter_mut().zip(texts) {
            segment.text = text;
        }
    }
    check_log(&record).map_err(|e| Error::corrupt(path, e))?;
    Ok(record)
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
pub(super) fn encode_session(path: &Path, record: &SessionRecord) -> Result<Vec<u8>> {
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

/// The four `## <name>` sections of a format-1-to-3 session body, in
/// order: Summary, Goals, Outcome, Next. Only those exact heading lines
/// delimit; any other line, including another `##` heading, is section
/// text. Blank lines around a section's text are dropped.
pub(super) fn split_sections_v3(path: &Path, body: &str) -> Result<[String; 4]> {
    const V3: [&str; 4] = ["Summary", "Goals", "Outcome", "Next"];
    let mut texts: [Vec<&str>; 4] = Default::default();
    let mut current: Option<usize> = None;
    for line in body.lines() {
        let next = current.map_or(0, |i| i + 1);
        let heading = V3
            .get(next)
            .is_some_and(|name| line.strip_prefix("## ") == Some(name));
        if heading {
            current = Some(next);
            continue;
        }
        match current {
            Some(i) => texts[i].push(line),
            None if line.trim().is_empty() => {}
            None => return Err(shape(path, "the body must start with '## Summary'")),
        }
    }
    if current != Some(V3.len() - 1) {
        let missing = V3[current.map_or(0, |i| i + 1)];
        return Err(shape(
            path,
            format!(
                "missing '## {missing}': a format-3 session body has ## Summary, ## Goals, ## Outcome and ## Next, in that order"
            ),
        ));
    }
    Ok(texts.map(|lines| lines.join("\n").trim_matches('\n').to_owned()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlDocument {
    #[allow(dead_code, reason = "checked by the format probe before this parse")]
    format: u32,
    sessions: BTreeMap<SessionId, SessionControl>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalsDocument {
    #[allow(dead_code, reason = "checked by the format probe before this parse")]
    format: u32,
    proposals: Vec<Proposal>,
}

/// Parse a JSON document: the format first, then the whole strictly.
fn decode_json<T: DeserializeOwned>(path: &Path, bytes: &[u8], document: Document) -> Result<T> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| Error::corrupt(path, e))?;
    let probe = FormatProbe::deserialize(&value).map_err(|e| Error::corrupt(path, e));
    let from = check_format(path, probe)?;
    if from == FORMAT {
        // Straight from the bytes, so a repeated key is refused too.
        return serde_json::from_slice(bytes).map_err(|e| Error::corrupt(path, e));
    }
    match document {
        Document::Control if from == 4 => {
            serde_json::from_slice::<ControlDocument>(bytes)
                .map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Proposals if from == 4 => {
            serde_json::from_slice::<ProposalsV4>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Control if from == 1 => {
            serde_json::from_slice::<ControlV1>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Control if from == 2 => {
            serde_json::from_slice::<ControlV2>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Control => {
            serde_json::from_slice::<ControlV3>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Proposals => {
            serde_json::from_slice::<ProposalsV3>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Entry | Document::Session => {}
    }
    upgrade(path, document, from, value)
}

/// Parse `control.json`.
pub(super) fn decode_control(path: &Path, bytes: &[u8]) -> Result<ControlState> {
    let doc: ControlDocument = decode_json(path, bytes, Document::Control)?;
    Ok(ControlState {
        sessions: doc.sessions,
    })
}

/// `control.json` as written.
#[derive(Serialize)]
struct ControlOut<'a> {
    format: u32,
    sessions: &'a BTreeMap<SessionId, SessionControl>,
}

/// `control.json`'s bytes at the current format, one line break at the end.
/// [`decode_control`] reads it back to the same state.
pub(super) fn encode_control(path: &Path, state: &ControlState) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(&ControlOut {
        format: FORMAT,
        sessions: &state.sessions,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Parse `proposals.json`.
pub(super) fn decode_proposals(path: &Path, bytes: &[u8]) -> Result<Vec<Proposal>> {
    let doc: ProposalsDocument = decode_json(path, bytes, Document::Proposals)?;
    Ok(doc.proposals)
}

/// `proposals.json` as written.
#[derive(Serialize)]
struct ProposalsOut<'a> {
    format: u32,
    proposals: &'a [Proposal],
}

/// `proposals.json`'s bytes at the current format, by id, one line break at
/// the end. [`decode_proposals`] reads it back to the same proposals.
pub(super) fn encode_proposals(path: &Path, proposals: &[Proposal]) -> Result<Vec<u8>> {
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

pub(super) fn decode_tags(path: &Path, bytes: &[u8]) -> Result<Vec<crate::StoreTag>> {
    check_format(
        path,
        serde_json::from_slice(bytes).map_err(|e| Error::corrupt(path, e)),
    )?;
    let doc: TagsDocument = serde_json::from_slice(bytes).map_err(|e| Error::corrupt(path, e))?;
    crate::TagTable::new(doc.tags.iter().map(crate::StoreTag::definition).collect())
        .map_err(|e| Error::corrupt(path, e))?;
    if doc.tags.iter().any(|t| {
        t.author.kind != crate::AuthorKind::Agent
            || t.updated_by
                .as_ref()
                .is_some_and(|a| a.kind != crate::AuthorKind::Agent)
    }) {
        return Err(shape(path, "store tags must have agent authors"));
    }
    Ok(doc.tags)
}

pub(super) fn encode_tags(path: &Path, tags: &[crate::StoreTag]) -> Result<Vec<u8>> {
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
