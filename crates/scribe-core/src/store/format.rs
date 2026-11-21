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

use crate::Author;
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
use time::OffsetDateTime;

/// The newest store format: this build reads formats 1 to `FORMAT` of every
/// document and would write `FORMAT`.
pub(super) const FORMAT: u32 = 19;

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
pub(super) const UPGRADES: &[Upgrade] = &[
    retry_state,
    session_links,
    log_active_author,
    agent_writes,
    session_rebuilt_by,
    codex_controls,
    action_entries,
    entry_candidates,
    finding_entries,
    capture_admission,
    entry_inactive_since,
    instructed_settlement,
    claim_statement,
    instructed_approval,
    idea_outcome,
    issue_kind,
    removal_instruction,
    memory_items,
];

/// The first format a memory item exists at: `memory/<id>.md` has no older
/// form.
pub(super) const MEMORY_SINCE: u32 = 19;

/// Format 18 -> 19 changes no stored value. It adds the `memory/`
/// directory, one `memory/<id>.md` per memory item (2_design.md §13.2),
/// which has no older form. A memory write also rewrites `control.json` at
/// this format, so a format-18 build refuses a store that holds a memory
/// item rather than reading the store without it.
fn memory_items(
    _document: Document,
    value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    Ok(value)
}

/// Format 16 -> 17 changes no stored value. It adds a record kind: the
/// `issues/` directory of `issue` records (2_design.md §11.4), which have
/// no older form. An issue file carries format 17, so a format-16 build
/// that reads one refuses it as too new rather than as corrupt; every other
/// document is unchanged.
fn issue_kind(
    _document: Document,
    value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    Ok(value)
}

/// Format 17 -> 18 adds `removal_instruction` to each entry: the checked
/// instruction that retired or forgot it, kept apart from `instruction`,
/// which from now on is only the instruction `approved_by` approved on
/// (2_design.md §12.3, §12.4). Through format 17 one `instruction` key held
/// whichever came last, so an entry without `approved_by` moves its
/// `instruction` to `removal_instruction`; one with `approved_by` keeps it as
/// the approval's. Other documents are unchanged.
fn removal_instruction(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Entry
        && let Some(entry) = value.as_object_mut()
    {
        let approved = entry.get("approved_by").is_some_and(|by| !by.is_null());
        let removal = if approved {
            serde_json::Value::Null
        } else {
            entry
                .insert(String::from("instruction"), serde_json::Value::Null)
                .unwrap_or(serde_json::Value::Null)
        };
        entry.entry("removal_instruction").or_insert(removal);
    }
    Ok(value)
}

/// The 17 -> 18 split of an older entry's one `instruction` into the
/// approval's and the removal's, as [`removal_instruction`] does in JSON:
/// `(instruction, removal_instruction)`.
fn split_instruction(
    approved_by: Option<&Author>,
    instruction: Option<crate::Instruction>,
) -> (Option<crate::Instruction>, Option<crate::Instruction>) {
    if approved_by.is_some() {
        (instruction, None)
    } else {
        (None, instruction)
    }
}

/// Format 14 -> 15 changes no stored value. It admits two shapes an agent's
/// approval of a claim on Daniel's checked instruction writes (2_design.md
/// §12.3): an entry with `approved_by` whose `said` is the canonical
/// `scribe proposal settle p-<n> --approve ...` command rather than a
/// `#scribe approve` line, and `closed_reason: covered` on an approved
/// candidate closed because an active entry covers it. A format-14 reader
/// refuses both, so the bump makes an older build report the store as too
/// new rather than as corrupt.
fn instructed_approval(
    _document: Document,
    value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    Ok(value)
}

/// Format 15 -> 16 adds `outcome` to each entry, `null`: only an idea the
/// session pass writes itself carries one (2_design.md §11.6), and no
/// older entry was written that way. Other documents are unchanged.
fn idea_outcome(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Entry
        && let Some(entry) = value.as_object_mut()
    {
        entry.entry("outcome").or_insert(serde_json::Value::Null);
    }
    Ok(value)
}

/// Format 11 -> 12 adds `inactive_since` to each entry: `null` for an
/// active one, and `updated` for one already retired, superseded or
/// forgotten. That is the best time an older record holds; a metadata
/// write after it left `active` may have moved `updated` later, which
/// only makes Stop recovery count a few more turns as covered. Other
/// documents are unchanged.
fn entry_inactive_since(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Entry
        && let Some(entry) = value.as_object_mut()
    {
        let active = entry.get("status").and_then(serde_json::Value::as_str) == Some("active");
        let since = if active {
            serde_json::Value::Null
        } else {
            entry
                .get("updated")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        };
        entry.entry("inactive_since").or_insert(since);
    }
    Ok(value)
}

/// Format 12 -> 13 adds `approved_by` and `instruction` to each entry and
/// `instruction` to each proposal, all `null`, and lets `closed_reason`
/// carry `instructed`. Other documents are unchanged.
fn instructed_settlement(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    match document {
        Document::Entry => {
            if let Some(entry) = value.as_object_mut() {
                entry
                    .entry("approved_by")
                    .or_insert(serde_json::Value::Null);
                entry
                    .entry("instruction")
                    .or_insert(serde_json::Value::Null);
            }
        }
        Document::Proposals => {
            for proposal in value
                .get_mut("proposals")
                .and_then(serde_json::Value::as_array_mut)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_object_mut)
            {
                proposal
                    .entry("instruction")
                    .or_insert(serde_json::Value::Null);
            }
        }
        Document::Session | Document::Control => {}
    }
    Ok(value)
}

/// Format 13 -> 14 adds `statement` and `subject` to each proposal
/// candidate and `stated_by` to each entry, all `null`: every claim opened
/// before statements approves with its quote as content, and every entry's
/// content is its author's own words (2_design.md §11.1). Other documents
/// are unchanged.
fn claim_statement(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    match document {
        Document::Entry => {
            if let Some(entry) = value.as_object_mut() {
                entry.entry("stated_by").or_insert(serde_json::Value::Null);
            }
        }
        Document::Proposals => {
            for candidate in value
                .get_mut("proposals")
                .and_then(serde_json::Value::as_array_mut)
                .into_iter()
                .flatten()
                .filter_map(|p| p.get_mut("candidate"))
                .filter_map(serde_json::Value::as_object_mut)
            {
                candidate
                    .entry("statement")
                    .or_insert(serde_json::Value::Null);
                candidate
                    .entry("subject")
                    .or_insert(serde_json::Value::Null);
            }
        }
        Document::Session | Document::Control => {}
    }
    Ok(value)
}

/// Format 10 -> 11 adds `admitted` (what trigger and criteria admitted it)
/// to each proposal candidate and `closed_reason` (why scribe closed it) to
/// each proposal, both `null` for a proposal opened before capture rules.
/// Other documents are unchanged.
fn capture_admission(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Proposals {
        for proposal in value
            .get_mut("proposals")
            .and_then(serde_json::Value::as_array_mut)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_object_mut)
        {
            proposal
                .entry("closed_reason")
                .or_insert(serde_json::Value::Null);
            if let Some(candidate) = proposal
                .get_mut("candidate")
                .and_then(serde_json::Value::as_object_mut)
            {
                candidate
                    .entry("admitted")
                    .or_insert(serde_json::Value::Null);
            }
        }
    }
    Ok(value)
}

/// Format 9 -> 10 adds `evidence` and `as_of` to each entry, both `null`
/// for an entry written before findings, and `role` (`user`) and
/// `evidence` (`null`) to each proposal candidate, which until now could
/// only quote Daniel. Other documents are unchanged.
fn finding_entries(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Entry
        && let Some(entry) = value.as_object_mut()
    {
        entry.entry("evidence").or_insert(serde_json::Value::Null);
        entry.entry("as_of").or_insert(serde_json::Value::Null);
    }
    if document == Document::Proposals {
        for candidate in value
            .get_mut("proposals")
            .and_then(serde_json::Value::as_array_mut)
            .into_iter()
            .flatten()
            .filter_map(|p| p.get_mut("candidate"))
            .filter_map(serde_json::Value::as_object_mut)
        {
            candidate.entry("role").or_insert(serde_json::json!("user"));
            candidate
                .entry("evidence")
                .or_insert(serde_json::Value::Null);
        }
    }
    Ok(value)
}

/// Format 8 -> 9 adds candidate payloads to proposals. Older proposals
/// retain their subjects and provenance; only the new field is null.
fn entry_candidates(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Proposals {
        for proposal in value
            .get_mut("proposals")
            .and_then(serde_json::Value::as_array_mut)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_object_mut)
        {
            proposal
                .entry("candidate")
                .or_insert(serde_json::Value::Null);
        }
    }
    Ok(value)
}

/// Format 7 -> 8 adds `quote` and `window` to each entry, both `null` for
/// an entry written before action tags. `pickups.json` is new at format 8
/// and has no older form. Other documents are unchanged.
fn action_entries(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Entry
        && let Some(entry) = value.as_object_mut()
    {
        entry.entry("quote").or_insert(serde_json::Value::Null);
        entry.entry("window").or_insert(serde_json::Value::Null);
    }
    Ok(value)
}

/// Format 6 -> 7 adds transcript provider identity and Codex ingestion cursors.
fn codex_controls(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Control
        && let Some(doc) = value.as_object_mut()
    {
        doc.entry("codex_cursors").or_insert(serde_json::json!({}));
        if let Some(sessions) = doc
            .get_mut("sessions")
            .and_then(serde_json::Value::as_object_mut)
        {
            for switch in sessions.values_mut() {
                if let Some(switch) = switch.as_object_mut() {
                    switch
                        .entry("provider")
                        .or_insert(serde_json::json!("claude-code"));
                }
            }
        }
    }
    Ok(value)
}

/// Format 5 -> 6 adds `rebuilt_by` to each session record: the agent that
/// ran `session rebuild`, or `null` for a record no rebuild wrote. Other
/// documents are unchanged. Reads upgrade only in memory.
fn session_rebuilt_by(
    document: Document,
    mut value: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    if document == Document::Session
        && let Some(record) = value.as_object_mut()
    {
        record
            .entry("rebuilt_by")
            .or_insert(serde_json::Value::Null);
    }
    Ok(value)
}

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

/// Frozen format-4-through-6 control shape. Typed validation runs again after
/// the append-only upgrade; these strict structs preserve duplicate/key checks.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ControlV6 {
    format: u32,
    sessions: BTreeMap<SessionId, SwitchV6>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct SwitchV6 {
    on: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    since: Option<time::OffsetDateTime>,
    due: bool,
    #[serde(rename = "final")]
    final_pass: bool,
    consecutive_failures: u32,
    #[serde(with = "time::serde::rfc3339::option")]
    retry_after: Option<time::OffsetDateTime>,
    pending_links: Vec<SessionLink>,
    #[serde(deserialize_with = "crate::record::nullable")]
    cataloged_by: Option<crate::activity::CatalogedBy>,
}

/// Frozen format-7-through-9 control shape: format 6 -> 7 (`codex_controls`)
/// adds `codex_cursors` and a per-session `provider`. Formats 8 and 9 leave
/// `control.json` unchanged, so all three check against this shape.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ControlV7 {
    format: u32,
    codex_cursors: BTreeMap<SessionId, String>,
    sessions: BTreeMap<SessionId, SwitchV7>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct SwitchV7 {
    provider: crate::Provider,
    on: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    since: Option<time::OffsetDateTime>,
    due: bool,
    #[serde(rename = "final")]
    final_pass: bool,
    consecutive_failures: u32,
    #[serde(with = "time::serde::rfc3339::option")]
    retry_after: Option<time::OffsetDateTime>,
    pending_links: Vec<SessionLink>,
    #[serde(deserialize_with = "crate::record::nullable")]
    cataloged_by: Option<crate::activity::CatalogedBy>,
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

/// Frozen format-5-to-8 proposal shape, checked before upgrade.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ProposalsV8 {
    format: u32,
    proposals: Vec<ProposalV8>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ProposalV8 {
    /// `p-<n>`.
    id: ProposalId,
    /// What it would change.
    kind: ProposalKind,
    /// The entry that would be superseded or retired.
    older: EntryId,
    /// The entry that supersedes it; `null` for a stale proposal.
    #[serde(deserialize_with = "crate::record::nullable")]
    newer: Option<EntryId>,
    /// Why, in the consistency model's words (at most 300 characters).
    reason: String,
    /// Verbatim spans from the entries involved.
    quotes: Vec<Quote>,
    /// Whether it is settled.
    state: ProposalState,
    /// When the routine opened it.
    #[serde(with = "time::serde::rfc3339")]
    opened_at: OffsetDateTime,
    /// When Daniel settled it.
    #[serde(with = "time::serde::rfc3339::option")]
    settled_at: Option<OffsetDateTime>,
    /// The turn Daniel settled it in.
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_by: Option<Cite>,
    /// Who opened it: `{kind: scribe, name: consistency}` for the routine.
    author: Author,
    /// Agent who last revised the reason, or null.
    #[serde(deserialize_with = "crate::record::nullable")]
    updated_by: Option<Author>,
    /// Who settled it, set exactly when `state` is not `open`.
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_author: Option<Author>,
}

/// Frozen format-9 proposal shape (a candidate payload without `role` or
/// `evidence`), checked before the 9 -> 10 upgrade.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ProposalsV9 {
    format: u32,
    proposals: Vec<ProposalV9>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ProposalV9 {
    id: ProposalId,
    kind: ProposalKind,
    older: EntryId,
    #[serde(deserialize_with = "crate::record::nullable")]
    candidate: Option<CandidateV9>,
    #[serde(deserialize_with = "crate::record::nullable")]
    newer: Option<EntryId>,
    reason: String,
    quotes: Vec<Quote>,
    state: ProposalState,
    #[serde(with = "time::serde::rfc3339")]
    opened_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    settled_at: Option<OffsetDateTime>,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_by: Option<Cite>,
    author: Author,
    #[serde(deserialize_with = "crate::record::nullable")]
    updated_by: Option<Author>,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_author: Option<Author>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct CandidateV9 {
    tag: crate::TagName,
    quote: String,
    cite: Cite,
    project: crate::Project,
    provider: crate::Label,
    host: crate::Label,
}

/// Frozen format-10 proposal shape (no `closed_reason`, and a candidate
/// without `admitted`), checked before the 10 -> 11 upgrade.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ProposalsV10 {
    format: u32,
    proposals: Vec<ProposalV10>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ProposalV10 {
    id: ProposalId,
    kind: ProposalKind,
    older: EntryId,
    #[serde(deserialize_with = "crate::record::nullable")]
    candidate: Option<CandidateV10>,
    #[serde(deserialize_with = "crate::record::nullable")]
    newer: Option<EntryId>,
    reason: String,
    quotes: Vec<Quote>,
    state: ProposalState,
    #[serde(with = "time::serde::rfc3339")]
    opened_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    settled_at: Option<OffsetDateTime>,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_by: Option<Cite>,
    author: Author,
    #[serde(deserialize_with = "crate::record::nullable")]
    updated_by: Option<Author>,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_author: Option<Author>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct CandidateV10 {
    tag: crate::TagName,
    role: crate::Speaker,
    quote: String,
    #[serde(deserialize_with = "crate::record::nullable")]
    evidence: Option<String>,
    cite: Cite,
    project: crate::Project,
    provider: crate::Label,
    host: crate::Label,
}

/// Frozen format-10 to 12 entry shape (no `approved_by` or `instruction`),
/// migrated in typed YAML so local YAML tags stay intact. `inactive_since`
/// exists from format 12 on; `decode_entry` refuses it in an older file and
/// `into_entry` derives it for one, as the 11 -> 12 step does.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryV12 {
    id: EntryId,
    tag: crate::TagName,
    status: crate::EntryStatus,
    project: crate::Project,
    topics: crate::Topics,
    related: Vec<crate::Related>,
    provider: crate::Label,
    host: crate::Label,
    author: Author,
    #[serde(deserialize_with = "crate::record::nullable")]
    updated_by: Option<Author>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    inactive_since: Option<OffsetDateTime>,
    said: String,
    content_from: crate::ContentFrom,
    #[serde(deserialize_with = "crate::record::nullable")]
    cite: Option<Cite>,
    #[serde(deserialize_with = "crate::record::nullable")]
    retirement_reason: Option<String>,
    #[serde(deserialize_with = "crate::record::nullable")]
    superseded_by: Option<EntryId>,
    #[serde(with = "crate::record::nullable_date")]
    review_after: Option<time::Date>,
    #[serde(deserialize_with = "crate::record::nullable")]
    quote: Option<String>,
    #[serde(deserialize_with = "crate::record::nullable")]
    window: Option<crate::Window>,
    #[serde(deserialize_with = "crate::record::nullable")]
    evidence: Option<String>,
    #[serde(with = "crate::record::nullable_date")]
    as_of: Option<time::Date>,
}

impl EntryV12 {
    fn into_entry(self, from: u32) -> Entry {
        let inactive_since = if from >= 12 {
            self.inactive_since
        } else if self.status == crate::EntryStatus::Active {
            None
        } else {
            Some(self.updated)
        };
        Entry {
            id: self.id,
            tag: self.tag,
            status: self.status,
            project: self.project,
            topics: self.topics,
            related: self.related,
            provider: self.provider,
            host: self.host,
            author: self.author,
            updated_by: self.updated_by,
            created: self.created,
            updated: self.updated,
            inactive_since,
            said: self.said,
            content_from: self.content_from,
            cite: self.cite,
            retirement_reason: self.retirement_reason,
            superseded_by: self.superseded_by,
            review_after: self.review_after,
            quote: self.quote,
            window: self.window,
            evidence: self.evidence,
            as_of: self.as_of,
            approved_by: None,
            instruction: None,
            removal_instruction: None,
            stated_by: None,
            outcome: None,
            content: String::new(),
        }
    }
}

/// Frozen format-13 entry shape (no `stated_by`), migrated in typed YAML
/// like [`EntryV12`] so local YAML tags stay intact.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryV13 {
    id: EntryId,
    tag: crate::TagName,
    status: crate::EntryStatus,
    project: crate::Project,
    topics: crate::Topics,
    related: Vec<crate::Related>,
    provider: crate::Label,
    host: crate::Label,
    author: Author,
    #[serde(deserialize_with = "crate::record::nullable")]
    updated_by: Option<Author>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    inactive_since: Option<OffsetDateTime>,
    said: String,
    content_from: crate::ContentFrom,
    #[serde(deserialize_with = "crate::record::nullable")]
    cite: Option<Cite>,
    #[serde(deserialize_with = "crate::record::nullable")]
    retirement_reason: Option<String>,
    #[serde(deserialize_with = "crate::record::nullable")]
    superseded_by: Option<EntryId>,
    #[serde(with = "crate::record::nullable_date")]
    review_after: Option<time::Date>,
    #[serde(deserialize_with = "crate::record::nullable")]
    quote: Option<String>,
    #[serde(deserialize_with = "crate::record::nullable")]
    window: Option<crate::Window>,
    #[serde(deserialize_with = "crate::record::nullable")]
    evidence: Option<String>,
    #[serde(with = "crate::record::nullable_date")]
    as_of: Option<time::Date>,
    #[serde(deserialize_with = "crate::record::nullable")]
    approved_by: Option<Author>,
    #[serde(deserialize_with = "crate::record::nullable")]
    instruction: Option<crate::Instruction>,
}

impl EntryV13 {
    fn into_entry(self) -> Entry {
        let (instruction, removal_instruction) =
            split_instruction(self.approved_by.as_ref(), self.instruction);
        Entry {
            id: self.id,
            tag: self.tag,
            status: self.status,
            project: self.project,
            topics: self.topics,
            related: self.related,
            provider: self.provider,
            host: self.host,
            author: self.author,
            updated_by: self.updated_by,
            created: self.created,
            updated: self.updated,
            inactive_since: self.inactive_since,
            said: self.said,
            content_from: self.content_from,
            cite: self.cite,
            retirement_reason: self.retirement_reason,
            superseded_by: self.superseded_by,
            review_after: self.review_after,
            quote: self.quote,
            window: self.window,
            evidence: self.evidence,
            as_of: self.as_of,
            approved_by: self.approved_by,
            instruction,
            removal_instruction,
            stated_by: None,
            outcome: None,
            content: String::new(),
        }
    }
}

/// Frozen format-14 entry shape (no `outcome`), which format 15 kept,
/// migrated in typed YAML like [`EntryV13`] so local YAML tags stay intact.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryV14 {
    id: EntryId,
    tag: crate::TagName,
    status: crate::EntryStatus,
    project: crate::Project,
    topics: crate::Topics,
    related: Vec<crate::Related>,
    provider: crate::Label,
    host: crate::Label,
    author: Author,
    #[serde(deserialize_with = "crate::record::nullable")]
    updated_by: Option<Author>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    inactive_since: Option<OffsetDateTime>,
    said: String,
    content_from: crate::ContentFrom,
    #[serde(deserialize_with = "crate::record::nullable")]
    cite: Option<Cite>,
    #[serde(deserialize_with = "crate::record::nullable")]
    retirement_reason: Option<String>,
    #[serde(deserialize_with = "crate::record::nullable")]
    superseded_by: Option<EntryId>,
    #[serde(with = "crate::record::nullable_date")]
    review_after: Option<time::Date>,
    #[serde(deserialize_with = "crate::record::nullable")]
    quote: Option<String>,
    #[serde(deserialize_with = "crate::record::nullable")]
    window: Option<crate::Window>,
    #[serde(deserialize_with = "crate::record::nullable")]
    evidence: Option<String>,
    #[serde(with = "crate::record::nullable_date")]
    as_of: Option<time::Date>,
    #[serde(deserialize_with = "crate::record::nullable")]
    approved_by: Option<Author>,
    #[serde(deserialize_with = "crate::record::nullable")]
    instruction: Option<crate::Instruction>,
    #[serde(deserialize_with = "crate::record::nullable")]
    stated_by: Option<Author>,
}

impl EntryV14 {
    fn into_entry(self) -> Entry {
        let (instruction, removal_instruction) =
            split_instruction(self.approved_by.as_ref(), self.instruction);
        Entry {
            id: self.id,
            tag: self.tag,
            status: self.status,
            project: self.project,
            topics: self.topics,
            related: self.related,
            provider: self.provider,
            host: self.host,
            author: self.author,
            updated_by: self.updated_by,
            created: self.created,
            updated: self.updated,
            inactive_since: self.inactive_since,
            said: self.said,
            content_from: self.content_from,
            cite: self.cite,
            retirement_reason: self.retirement_reason,
            superseded_by: self.superseded_by,
            review_after: self.review_after,
            quote: self.quote,
            window: self.window,
            evidence: self.evidence,
            as_of: self.as_of,
            approved_by: self.approved_by,
            instruction,
            removal_instruction,
            stated_by: self.stated_by,
            outcome: None,
            content: String::new(),
        }
    }
}

/// Frozen format-16 entry shape (no `removal_instruction`), which format 17
/// kept, migrated in typed YAML like [`EntryV14`] so local YAML tags stay intact.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryV16 {
    id: EntryId,
    tag: crate::TagName,
    status: crate::EntryStatus,
    project: crate::Project,
    topics: crate::Topics,
    related: Vec<crate::Related>,
    provider: crate::Label,
    host: crate::Label,
    author: Author,
    #[serde(deserialize_with = "crate::record::nullable")]
    updated_by: Option<Author>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    inactive_since: Option<OffsetDateTime>,
    said: String,
    content_from: crate::ContentFrom,
    #[serde(deserialize_with = "crate::record::nullable")]
    cite: Option<Cite>,
    #[serde(deserialize_with = "crate::record::nullable")]
    retirement_reason: Option<String>,
    #[serde(deserialize_with = "crate::record::nullable")]
    superseded_by: Option<EntryId>,
    #[serde(with = "crate::record::nullable_date")]
    review_after: Option<time::Date>,
    #[serde(deserialize_with = "crate::record::nullable")]
    quote: Option<String>,
    #[serde(deserialize_with = "crate::record::nullable")]
    window: Option<crate::Window>,
    #[serde(deserialize_with = "crate::record::nullable")]
    evidence: Option<String>,
    #[serde(with = "crate::record::nullable_date")]
    as_of: Option<time::Date>,
    #[serde(deserialize_with = "crate::record::nullable")]
    approved_by: Option<Author>,
    #[serde(deserialize_with = "crate::record::nullable")]
    instruction: Option<crate::Instruction>,
    #[serde(deserialize_with = "crate::record::nullable")]
    stated_by: Option<Author>,
    #[serde(deserialize_with = "crate::record::nullable")]
    outcome: Option<crate::Outcome>,
}

impl EntryV16 {
    fn into_entry(self) -> Entry {
        let (instruction, removal_instruction) =
            split_instruction(self.approved_by.as_ref(), self.instruction);
        Entry {
            id: self.id,
            tag: self.tag,
            status: self.status,
            project: self.project,
            topics: self.topics,
            related: self.related,
            provider: self.provider,
            host: self.host,
            author: self.author,
            updated_by: self.updated_by,
            created: self.created,
            updated: self.updated,
            inactive_since: self.inactive_since,
            said: self.said,
            content_from: self.content_from,
            cite: self.cite,
            retirement_reason: self.retirement_reason,
            superseded_by: self.superseded_by,
            review_after: self.review_after,
            quote: self.quote,
            window: self.window,
            evidence: self.evidence,
            as_of: self.as_of,
            approved_by: self.approved_by,
            instruction,
            removal_instruction,
            stated_by: self.stated_by,
            outcome: self.outcome,
            content: String::new(),
        }
    }
}

/// Frozen format-13 proposal shape (candidates without `statement` or
/// `subject`, which have the format-11 candidate shape), checked before
/// the 13 -> 14 upgrade.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ProposalsV13 {
    format: u32,
    proposals: Vec<ProposalV13>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
enum ClosedReasonV13 {
    TagRetired,
    Instructed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct InstructionV13 {
    session: crate::SessionId,
    message: String,
    #[serde(with = "time::serde::rfc3339")]
    at: OffsetDateTime,
    #[serde(deserialize_with = "crate::record::nullable")]
    quote: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ProposalV13 {
    id: ProposalId,
    kind: ProposalKind,
    older: EntryId,
    #[serde(deserialize_with = "crate::record::nullable")]
    candidate: Option<CandidateV11>,
    #[serde(deserialize_with = "crate::record::nullable")]
    newer: Option<EntryId>,
    reason: String,
    quotes: Vec<Quote>,
    state: ProposalState,
    #[serde(with = "time::serde::rfc3339")]
    opened_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    settled_at: Option<OffsetDateTime>,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_by: Option<Cite>,
    author: Author,
    #[serde(deserialize_with = "crate::record::nullable")]
    updated_by: Option<Author>,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_author: Option<Author>,
    #[serde(deserialize_with = "crate::record::nullable")]
    closed_reason: Option<ClosedReasonV13>,
    #[serde(deserialize_with = "crate::record::nullable")]
    instruction: Option<InstructionV13>,
}

/// Frozen format-11 proposal shape (no `instruction`, and `closed_reason` only
/// accepts `tag_retired`; candidate frozen independently of `EntryCandidate`),
/// checked before the 11 -> 12 upgrade.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ProposalsV11 {
    format: u32,
    proposals: Vec<ProposalV11>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
enum ClosedReasonV11 {
    TagRetired,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct AdmittedV11 {
    trigger: String,
    matched: String,
    #[serde(deserialize_with = "crate::record::nullable")]
    criteria: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct CandidateV11 {
    tag: crate::TagName,
    role: crate::Speaker,
    quote: String,
    #[serde(deserialize_with = "crate::record::nullable")]
    evidence: Option<String>,
    cite: Cite,
    project: crate::Project,
    provider: crate::Label,
    host: crate::Label,
    #[serde(deserialize_with = "crate::record::nullable")]
    admitted: Option<AdmittedV11>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "frozen shape checked before upgrade")]
struct ProposalV11 {
    id: ProposalId,
    kind: ProposalKind,
    older: EntryId,
    #[serde(deserialize_with = "crate::record::nullable")]
    candidate: Option<CandidateV11>,
    #[serde(deserialize_with = "crate::record::nullable")]
    newer: Option<EntryId>,
    reason: String,
    quotes: Vec<Quote>,
    state: ProposalState,
    #[serde(with = "time::serde::rfc3339")]
    opened_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    settled_at: Option<OffsetDateTime>,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_by: Option<Cite>,
    author: Author,
    #[serde(deserialize_with = "crate::record::nullable")]
    updated_by: Option<Author>,
    #[serde(deserialize_with = "crate::record::nullable")]
    settled_author: Option<Author>,
    #[serde(deserialize_with = "crate::record::nullable")]
    closed_reason: Option<ClosedReasonV11>,
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
const MEMORY_KIND: &str = "memory";

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

pub(super) fn shape(path: &Path, message: impl Into<String>) -> Error {
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

/// Parse frontmatter: check `format` and `kind`, returning the file's format
/// and the remaining mapping as a YAML value.
pub(super) fn parse_frontmatter(
    path: &Path,
    front: &str,
    kind: &str,
) -> Result<(u32, yaml_serde::Value)> {
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
    let (from, value) = parse_frontmatter(path, front, ENTRY_KIND)?;
    let mut entry: Entry = if from == FORMAT {
        yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?
    } else if (10..=12).contains(&from) {
        let has_inactive_since = value
            .as_mapping()
            .is_some_and(|map| map.contains_key("inactive_since"));
        if from < 12 && has_inactive_since {
            return Err(shape(path, "unknown field `inactive_since`"));
        }
        if from == 12 && !has_inactive_since {
            return Err(shape(path, "missing field `inactive_since`"));
        }
        let old: EntryV12 = yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?;
        old.into_entry(from)
    } else if from == 13 {
        let old: EntryV13 = yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?;
        old.into_entry()
    } else if from == 14 || from == 15 {
        let old: EntryV14 = yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?;
        old.into_entry()
    } else if from == 16 || from == 17 {
        let old: EntryV16 = yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?;
        old.into_entry()
    } else {
        let json = serde_json::to_value(&value).map_err(|e| Error::corrupt(path, e))?;
        upgrade(path, Document::Entry, from, json)?
    };
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
            || (from >= 15
                && entry.approved_by.is_some()
                && said.starts_with(crate::proposal::INSTRUCTED_SAID_PREFIX))
    };
    let valid = match entry.author.kind {
        crate::AuthorKind::Daniel => {
            entry.cite.is_some()
                && entry.content_from != crate::ContentFrom::Agent
                && match &entry.stated_by {
                    // Daniel's own words: no separate quote.
                    None => entry.quote.is_none(),
                    // Scribe's statement of Daniel's turn (format 14): an
                    // approved pass claim from his message, his words in
                    // `quote` until the entry is forgotten.
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
                && entry.quote.is_none()
                && entry.window.is_none();
            (direct || approved_assistant) && entry.stated_by.is_none()
        }
        // An idea the session pass wrote itself (format 16): scribe's
        // statement of one of Daniel's turns as content, his words in
        // `quote` until it is forgotten, nothing typed, and an outcome.
        crate::AuthorKind::Scribe => {
            entry.author == Author::session_pass()
                && entry.stated_by.as_ref() == Some(&Author::session_pass())
                && entry.cite.is_some()
                && matches!(
                    entry.content_from,
                    crate::ContentFrom::Message {
                        role: crate::Speaker::User,
                        ..
                    }
                )
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
    let (from, value) = parse_frontmatter(path, front, SESSION_KIND)?;
    let mut record: SessionRecord = if from >= 6 {
        yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?
    } else {
        let json = serde_json::to_value(&value).map_err(|e| Error::corrupt(path, e))?;
        upgrade(path, Document::Session, from, json)?
    };
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
    codex_cursors: BTreeMap<SessionId, String>,
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
    // Each older shape is checked strictly. Formats 4 and 5 left control.json
    // and proposals.json at their current shapes (format 5 -> 6 changed
    // neither), so those formats check against the current structs.
    match document {
        Document::Control if from == 1 => {
            serde_json::from_slice::<ControlV1>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Control if from == 2 => {
            serde_json::from_slice::<ControlV2>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Control if from == 3 => {
            serde_json::from_slice::<ControlV3>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Control if from <= 6 => {
            serde_json::from_slice::<ControlV6>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Control => {
            serde_json::from_slice::<ControlV7>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Proposals if from == 4 => {
            serde_json::from_slice::<ProposalsV4>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        // Format 12 changed only entries, so a format-11 or format-12
        // proposals.json has the format-11 shape.
        Document::Proposals if from == 11 || from == 12 => {
            serde_json::from_slice::<ProposalsV11>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Proposals if from == 13 => {
            serde_json::from_slice::<ProposalsV13>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        // Format 15 changed no key, only admitted `closed_reason: covered`.
        Document::Proposals if from == 14 => {
            let doc = serde_json::from_slice::<ProposalsDocument>(bytes)
                .map_err(|e| Error::corrupt(path, e))?;
            if doc
                .proposals
                .iter()
                .any(|p| p.closed_reason == Some(crate::ClosedReason::Covered))
            {
                return Err(shape(path, "unknown variant `covered`"));
            }
        }
        // Formats 16 to 19 changed no proposal (16 and 18 changed only
        // entries, 17 and 19 added record kinds), so a format-15 to format-18
        // proposals.json has the current shape.
        Document::Proposals if (15..=18).contains(&from) => {
            serde_json::from_slice::<ProposalsDocument>(bytes)
                .map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Proposals if from == 10 => {
            serde_json::from_slice::<ProposalsV10>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Proposals if from == 9 => {
            serde_json::from_slice::<ProposalsV9>(bytes).map_err(|e| Error::corrupt(path, e))?;
        }
        Document::Proposals if from >= 5 => {
            serde_json::from_slice::<ProposalsV8>(bytes).map_err(|e| Error::corrupt(path, e))?;
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
        codex_cursors: doc.codex_cursors,
    })
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
pub(super) fn encode_control(path: &Path, state: &ControlState) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(&ControlOut {
        format: FORMAT,
        sessions: &state.sessions,
        codex_cursors: &state.codex_cursors,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Parse `proposals.json`.
pub(super) fn decode_proposals(path: &Path, bytes: &[u8]) -> Result<Vec<Proposal>> {
    let doc: ProposalsDocument = decode_json(path, bytes, Document::Proposals)?;
    for proposal in &doc.proposals {
        // A statement is scribe's statement of Daniel's turn (format 14):
        // only on a candidate from his words, with its subject beside it.
        // A forgotten candidate has neither.
        if let Some(candidate) = &proposal.candidate
            && (candidate.statement.is_some() != candidate.subject.is_some()
                || (candidate.statement.is_some() && candidate.role != crate::Speaker::User))
        {
            return Err(shape(
                path,
                "candidate statement and subject must be set together, on a user candidate",
            ));
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
                    && candidate.has_valid_provenance()
                    && (!candidate.quote.is_empty()
                        || proposal.state == ProposalState::Approved)
                    && candidate.quote.chars().count() <= 400 => {}
            (ProposalKind::Candidate, Some(_) | None)
            | (ProposalKind::Supersedes | ProposalKind::Stale, Some(_)) => {
                return Err(shape(path, "invalid candidate proposal payload"));
            }
            (ProposalKind::Supersedes | ProposalKind::Stale, None) => {}
        }
    }
    Ok(doc.proposals)
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PickupsDocument {
    format: u32,
    pickups: Vec<crate::action::Pickup>,
}

/// Parse `pickups.json` (format 8 and later; there is no older form).
pub(super) fn decode_pickups(path: &Path, bytes: &[u8]) -> Result<Vec<crate::action::Pickup>> {
    check_format(
        path,
        serde_json::from_slice(bytes).map_err(|e| Error::corrupt(path, e)),
    )?;
    let doc: PickupsDocument =
        serde_json::from_slice(bytes).map_err(|e| Error::corrupt(path, e))?;
    if doc
        .pickups
        .iter()
        .any(|p| p.author.kind != crate::AuthorKind::Agent)
    {
        return Err(shape(path, "pickups must have agent authors"));
    }
    Ok(doc.pickups)
}

/// `pickups.json`'s bytes at the current format, oldest first.
pub(super) fn encode_pickups(path: &Path, pickups: &[crate::action::Pickup]) -> Result<Vec<u8>> {
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

/// Parse a memory item file (format [`MEMORY_SINCE`] and later): frontmatter
/// only, every key required, and an id that matches the file's stem.
pub(super) fn decode_memory(
    path: &Path,
    bytes: &[u8],
    expected: &str,
) -> Result<crate::memory::MemoryItem> {
    let (front, body) = split_frontmatter(path, bytes)?;
    let (from, value) = parse_frontmatter(path, front, MEMORY_KIND)?;
    if from < MEMORY_SINCE {
        return Err(shape(
            path,
            format!("a memory item has format {MEMORY_SINCE} or later, not {from}"),
        ));
    }
    if !body.trim().is_empty() {
        return Err(shape(path, "a memory item has no body"));
    }
    let item: crate::memory::MemoryItem =
        yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?;
    if item.id.as_str() != expected {
        return Err(shape(
            path,
            format!("id {} does not match its file name {expected}", item.id),
        ));
    }
    if item.statement.chars().count() > crate::memory::STATEMENT_CHARS
        || (item.sensitive && !item.section.holds_sensitive())
        || item.occurrences.is_empty()
    {
        return Err(shape(
            path,
            "a memory item needs an occurrence, a statement of at most 200 characters, and a sensitive event only in now or milestones",
        ));
    }
    Ok(item)
}

/// A memory item's bytes at the current format.
pub(super) fn encode_memory(path: &Path, item: &crate::memory::MemoryItem) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct MemoryFile<'a> {
        format: u32,
        kind: &'static str,
        #[serde(flatten)]
        item: &'a crate::memory::MemoryItem,
    }
    let front = yaml_serde::to_string(&MemoryFile {
        format: FORMAT,
        kind: MEMORY_KIND,
        item,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    Ok(format!("---\n{front}---\n").into_bytes())
}
