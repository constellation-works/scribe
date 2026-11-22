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

use crate::entry::{EntryId, TagName};
use crate::proposal::{Proposal, ProposalId, ProposalKind, ProposalState};
use crate::record::Project;
use crate::session::{SessionId, SessionRecord};
use std::collections::BTreeMap;
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
    /// The last sessions: the project's first, each part newest first.
    pub sessions: Vec<BriefSession>,
    /// The open claims, grouped by tag in tag order; claims by id within a tag.
    pub claims: Vec<ClaimGroup>,
}

/// One session in the brief.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefSession {
    /// The session.
    pub id: SessionId,
    /// Its project.
    pub project: Project,
    /// Its one-line title.
    pub title: String,
    /// Its last recorded activity: the end of its last segment, else the
    /// record's last update.
    pub at: OffsetDateTime,
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
    /// What it claims: a candidate's statement (its quote when it has
    /// none), or a consistency proposal's reason.
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
        out.push_str("## Recent sessions\n");
        if self.sessions.is_empty() {
            out.push_str("(none)\n");
        }
        for s in &self.sessions {
            out.push_str(&format!(
                "- {} · {} · {} · {}\n",
                s.id,
                stamp(s.at),
                s.project,
                one_line(&s.title)
            ));
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
            "Settle a claim by typing #scribe approve <id> or #scribe reject <id>; scribe proposal list --state open shows each in full.\n",
        );
        out
    }
}

/// Build the brief from every session record and proposal.
///
/// Sessions: the active records other than `exclude` (the session being
/// started), those in `project` first, each part newest activity first, at
/// most `sessions`. Claims: every open proposal, in any project,
/// under its candidate's tag or, for a consistency proposal, the tag of the
/// entry it would change.
#[must_use]
pub fn build(
    records: &[SessionRecord],
    proposals: &[Proposal],
    project: Option<&Project>,
    exclude: Option<&SessionId>,
    sessions: usize,
) -> Brief {
    let mut recent: Vec<BriefSession> = records
        .iter()
        .filter(|r| r.active)
        .filter(|r| exclude.is_none_or(|s| &r.id != s))
        .map(|r| BriefSession {
            id: r.id.clone(),
            project: r.project.clone(),
            title: r.title.clone(),
            at: r.log.last().map_or(r.updated, |s| s.to),
        })
        .collect();
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
                statement: c.statement.clone().unwrap_or_else(|| c.quote.clone()),
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
