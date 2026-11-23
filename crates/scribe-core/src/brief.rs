//! The session-start page (`scribe brief`, and what the `SessionStart` hook
//! injects): a memory block, then `## Recent sessions`, then `## Open
//! claims`. Every line carries its date, and the caps are fixed because the
//! page goes into every interactive session.
//!
//! - The memory block is opaque text the caller supplies: today the memory
//!   page (`scribe memory show --page`). It is its own top block, so the
//!   design's Profile, Now and Milestones replace it without touching the
//!   sections below.
//! - Recent sessions: the last five active records, the session's own
//!   project first, titles only, each with the id `scribe session show`
//!   takes.
//! - Open claims: every open proposal, grouped by tag, each with the id that
//!   settles it and the words it rests on.
//!
//! Model-free and pure (STD-02 R5): the caller reads the records and
//! proposals, and [`build`] only selects, orders and groups them;
//! [`Brief::text`] renders the one text both the command and the hook print.

use crate::entry::{Entry, EntryId, EntryStatus, TagName};
use crate::handoff;
use crate::proposal::{Proposal, ProposalId, ProposalKind, ProposalState};
use crate::record::Project;
use crate::session::{SessionId, SessionRecord};
use std::collections::{BTreeMap, BTreeSet};
use time::{OffsetDateTime, UtcOffset};

/// How many sessions a brief shows when the caller does not say.
pub const DEFAULT_SESSIONS: usize = 5;

/// The longest quote a claim carries, in characters; a longer one is cut
/// and ends in `…`.
pub const QUOTE_CHARS: usize = 200;

/// The most sessions a brief shows.
pub const MAX_SESSIONS: usize = 20;

/// The brief.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brief {
    /// The memory block at the top, as the caller rendered it; `None` when
    /// there is none.
    pub memory: Option<String>,
    /// The project whose sessions come first; `None` orders every project's
    /// sessions by date alone.
    pub project: Option<Project>,
    /// The project's most recently opened group.
    pub last_group: Option<LastGroup>,
    /// The last sessions: the project's first, each part newest first.
    pub sessions: Vec<BriefSession>,
    /// The open claims, grouped by tag in tag order; claims by id within a tag.
    pub claims: Vec<ClaimGroup>,
}

/// The group metadata and summary carried by a project brief.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LastGroup {
    /// Stable group id.
    pub id: String,
    /// Group title.
    pub title: String,
    /// Closing time, if closed.
    pub closed: Option<String>,
    /// Summary produced at close, if still retained.
    pub summary: Option<String>,
}

/// One session in the brief.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefSession {
    /// The session.
    pub id: SessionId,
    /// Its project.
    pub project: Project,
    /// Its one-line title; empty when the session has a handoff and no
    /// session record.
    pub title: String,
    /// Its last recorded activity: the newest of its last segment end (or
    /// record update) and its newest active handoff.
    pub at: OffsetDateTime,
    /// Whether scribe holds a session record for this session; `false` when
    /// it was not switched on and appears only through its handoff.
    pub has_record: bool,
    /// The newest active `handoff` entry for this session, when it has one.
    pub handoff: Option<EntryId>,
    /// How many active `handoff` entries this session has.
    pub handoffs: usize,
    /// Refresh mode of the newest handoff.
    pub handoff_mode: Option<crate::handoff::Mode>,
}

/// The open claims under one tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimGroup {
    /// The tag.
    pub tag: TagName,
    /// Its open claims, by id.
    pub claims: Vec<Claim>,
}

/// One open claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    /// The proposal that holds it; settling it settles the claim.
    pub id: ProposalId,
    /// What approving it would do.
    pub kind: ProposalKind,
    /// The words it rests on, as its line quotes them: a candidate's
    /// verbatim words from its turn, or a consistency proposal's first
    /// quoted span of an entry (its reason when it quotes none), on one
    /// line and cut at [`QUOTE_CHARS`].
    pub quote: String,
    /// What it claims: a candidate's context (an older claim's statement,
    /// else its quote), or a consistency proposal's reason.
    pub statement: String,
    /// A candidate's subject, when it has one.
    pub subject: Option<String>,
    /// The entry it would supersede or retire; `None` for a candidate.
    pub older: Option<EntryId>,
    /// The entry that would supersede `older`.
    pub newer: Option<EntryId>,
    /// When it was opened.
    pub opened_at: OffsetDateTime,
}

impl Brief {
    /// How many open claims it lists.
    #[must_use]
    pub fn claim_count(&self) -> usize {
        self.claims.iter().map(|g| g.claims.len()).sum()
    }

    /// The text the command prints and the hook injects.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::from("# scribe brief\n\n");
        if let Some(memory) = self
            .memory
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            out.push_str(memory);
            out.push_str("\n\n");
        }
        if let Some(group) = &self.last_group {
            out.push_str(&format!(
                "## Last group\n{} · {}\n",
                group.id,
                one_line(&group.title)
            ));
            if let Some(summary) = &group.summary {
                out.push_str(summary);
                out.push('\n');
            } else if group.closed.is_none() {
                out.push_str("Still open.\n");
            }
            out.push('\n');
        }
        out.push_str("## Recent sessions\n");
        if self.sessions.is_empty() {
            out.push_str("(none)\n");
        }
        for s in &self.sessions {
            match (s.has_record, &s.handoff) {
                (true, None) => out.push_str(&format!(
                    "- {} · {} · {} · {}\n",
                    s.id,
                    stamp(s.at),
                    s.project,
                    one_line(&s.title)
                )),
                (true, Some(handoff)) => out.push_str(&format!(
                    "- {} · {} · {} · {} [handoff: {}{}]\n",
                    s.id,
                    stamp(s.at),
                    s.project,
                    one_line(&s.title),
                    handoff,
                    s.handoff_mode
                        .map_or(String::new(), |m| format!(", mode: {}", m.as_str()))
                )),
                (false, Some(handoff)) => out.push_str(&format!(
                    "- {} · {} · {} · no session record [handoff: {}{}]\n",
                    s.id,
                    stamp(s.at),
                    s.project,
                    handoff,
                    s.handoff_mode
                        .map_or(String::new(), |m| format!(", mode: {}", m.as_str()))
                )),
                (false, None) => out.push_str(&format!(
                    "- {} · {} · {} · no session record\n",
                    s.id,
                    stamp(s.at),
                    s.project
                )),
            }
        }
        out.push_str("\n## Open claims\n");
        if self.claims.is_empty() {
            out.push_str("(none)\n");
            return out;
        }
        for group in &self.claims {
            for claim in &group.claims {
                out.push_str(&format!(
                    "- {} · {} · {} · \"{}\"\n",
                    claim.id,
                    stamp(claim.opened_at),
                    group.tag,
                    claim.quote
                ));
            }
        }
        out.push_str(
            "Settle a claim by typing #scribe approve <id> or #scribe reject <id>; scribe proposal show <id> shows one in full.\n",
        );
        out
    }
}

/// Build the brief from every session record, entry and proposal.
///
/// Sessions: the active records other than `exclude` (the session being
/// started), plus any unrecorded session (never switched on, no record at
/// all) that has an active `handoff` entry, those in `project` first, each
/// part newest activity first, at most `sessions`. Claims: every open
/// proposal, in any project, under its candidate's tag or, for a consistency
/// proposal, the tag of the entry it would change.
#[must_use]
pub fn build(
    records: &[SessionRecord],
    entries: &[Entry],
    proposals: &[Proposal],
    project: Option<&Project>,
    exclude: Option<&SessionId>,
    sessions: usize,
) -> Brief {
    let mut by_session: BTreeMap<SessionId, Vec<&Entry>> = BTreeMap::new();
    for entry in entries
        .iter()
        .filter(|e| e.status == EntryStatus::Active && e.tag.as_str() == handoff::HANDOFF_TAG)
    {
        for sid in handoff::session_ids(entry) {
            by_session.entry(sid).or_default().push(entry);
        }
    }
    for list in by_session.values_mut() {
        list.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
    }
    let recorded_ids: BTreeSet<&SessionId> = records.iter().map(|r| &r.id).collect();

    let mut recent: Vec<BriefSession> = records
        .iter()
        .filter(|r| r.active)
        .filter(|r| exclude.is_none_or(|s| &r.id != s))
        .map(|r| {
            let handoffs = by_session.get(&r.id);
            let newest = handoffs.and_then(|v| v.first().copied());
            let record_at = r.log.last().map_or(r.updated, |s| s.to);
            BriefSession {
                id: r.id.clone(),
                project: r.project.clone(),
                title: r.title.clone(),
                at: newest.map_or(record_at, |h| record_at.max(h.created)),
                has_record: true,
                handoff: newest.map(|h| h.id.clone()),
                handoffs: handoffs.map_or(0, Vec::len),
                handoff_mode: newest.and_then(|h| h.handoff_mode),
            }
        })
        .collect();
    for (sid, list) in &by_session {
        if recorded_ids.contains(sid) || exclude.is_some_and(|e| e == sid) {
            continue;
        }
        let Some(newest) = list.first() else {
            continue;
        };
        recent.push(BriefSession {
            id: sid.clone(),
            project: newest.project.clone(),
            title: String::new(),
            at: newest.created,
            has_record: false,
            handoff: Some(newest.id.clone()),
            handoffs: list.len(),
            handoff_mode: newest.handoff_mode,
        });
    }
    let elsewhere = |s: &BriefSession| project.is_some_and(|p| &s.project != p);
    recent.sort_by(|a, b| {
        elsewhere(a)
            .cmp(&elsewhere(b))
            .then_with(|| b.at.cmp(&a.at))
            .then_with(|| a.id.cmp(&b.id))
    });
    recent.truncate(sessions);

    let mut groups: BTreeMap<TagName, Vec<Claim>> = BTreeMap::new();
    for p in proposals.iter().filter(|p| p.state == ProposalState::Open) {
        let (tag, claim) = claim(p);
        groups.entry(tag).or_default().push(claim);
    }
    let claims = groups
        .into_iter()
        .map(|(tag, mut claims)| {
            claims.sort_by_key(|c| c.id);
            ClaimGroup { tag, claims }
        })
        .collect();
    Brief {
        memory: None,
        project: project.cloned(),
        last_group: None,
        sessions: recent,
        claims,
    }
}

fn claim(p: &Proposal) -> (TagName, Claim) {
    match &p.candidate {
        Some(c) => (
            c.tag.clone(),
            Claim {
                id: p.id,
                kind: p.kind,
                quote: cut(&c.quote),
                statement: c.stated().unwrap_or(&c.quote).to_owned(),
                subject: c.subject.clone().filter(|s| !s.trim().is_empty()),
                older: None,
                newer: None,
                opened_at: p.opened_at,
            },
        ),
        None => (
            p.older.tag().clone(),
            Claim {
                id: p.id,
                kind: p.kind,
                quote: cut(p
                    .quotes
                    .first()
                    .map_or(p.reason.as_str(), |q| q.text.as_str())),
                statement: p.reason.clone(),
                subject: None,
                older: Some(p.older.clone()),
                newer: p.newer.clone(),
                opened_at: p.opened_at,
            },
        ),
    }
}

/// `2026-11-21 18:04Z`.
fn stamp(at: OffsetDateTime) -> String {
    let at = at.to_offset(UtcOffset::UTC);
    format!(
        "{} {:02}:{:02}Z",
        crate::record::nullable_date::format(at.date()),
        at.hour(),
        at.minute()
    )
}

/// `text` on one line, cut at [`QUOTE_CHARS`] characters with `…` last.
fn cut(text: &str) -> String {
    let line = one_line(text);
    if line.chars().count() <= QUOTE_CHARS {
        return line;
    }
    let mut out: String = line.chars().take(QUOTE_CHARS - 1).collect();
    out.push('…');
    out
}

/// `text` on one line: runs of whitespace, newlines included, become one space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
