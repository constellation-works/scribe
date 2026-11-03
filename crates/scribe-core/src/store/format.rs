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
//!   `{"format": 1, ...}`, equally strict.
//!
//! Every document's `format` is read first, so a newer one is refused by
//! version, not by whatever parse error its new shape would cause.

use crate::entry::Entry;
use crate::error::{Error, Result};
use crate::proposal::Proposal;
use crate::session::{ControlState, SECTIONS, SessionControl, SessionId, SessionRecord};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU32;
use std::path::Path;

/// The newest store format: this build reads formats 1 to `FORMAT` of every
/// document and would write `FORMAT`.
pub(super) const FORMAT: u32 = 1;

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
/// current format. Empty: no scribe store predates format 1, and the
/// template's `notes.json` is not carried over.
pub(super) const UPGRADES: &[Upgrade] = &[];

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
/// strictly.
fn decode_frontmatter<T: DeserializeOwned>(
    path: &Path,
    front: &str,
    document: Document,
    kind: &str,
) -> Result<T> {
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
        return yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e));
    }
    let json = serde_json::to_value(&value).map_err(|e| Error::corrupt(path, e))?;
    upgrade(path, document, from, json)
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
    let mut entry: Entry = decode_frontmatter(path, front, Document::Entry, ENTRY_KIND)?;
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
pub(super) fn decode_session(
    path: &Path,
    bytes: &[u8],
    expected: &str,
    year_month: (&str, &str),
) -> Result<SessionRecord> {
    let (front, body) = split_frontmatter(path, bytes)?;
    let mut record: SessionRecord =
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
    let [summary, goals, outcome, next] = split_sections(path, body)?;
    record.summary = summary;
    record.goals = goals;
    record.outcome = outcome;
    record.next = next;
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
/// between two `---` lines, then the four sections in order.
/// [`decode_session`] reads it back to the same record.
pub(super) fn encode_session(path: &Path, record: &SessionRecord) -> Result<Vec<u8>> {
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
    let bodies = [
        &record.summary,
        &record.goals,
        &record.outcome,
        &record.next,
    ];
    for (i, ((name, _), body)) in SECTIONS.iter().zip(bodies).enumerate() {
        if i > 0 {
            text.push('\n');
        }
        text.push_str("## ");
        text.push_str(name);
        text.push('\n');
        if !body.is_empty() {
            text.push_str(body);
            text.push('\n');
        }
    }
    Ok(text.into_bytes())
}

/// The four `## <name>` sections of a session body, in order. Only those
/// exact heading lines delimit; any other line, including another `##`
/// heading, is section text. Blank lines around a section's text are
/// dropped.
pub(super) fn split_sections(path: &Path, body: &str) -> Result<[String; 4]> {
    let mut texts: [Vec<&str>; 4] = Default::default();
    let mut current: Option<usize> = None;
    for line in body.lines() {
        let next = current.map_or(0, |i| i + 1);
        let heading = SECTIONS
            .get(next)
            .is_some_and(|(name, _)| line.strip_prefix("## ") == Some(name));
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
    if current != Some(SECTIONS.len() - 1) {
        let missing = SECTIONS[current.map_or(0, |i| i + 1)].0;
        return Err(shape(
            path,
            format!(
                "missing '## {missing}': a session body has ## Summary, ## Goals, ## Outcome and ## Next, in that order"
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
