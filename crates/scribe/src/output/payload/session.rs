//! Payloads for session records, their logs and attached handoffs.

use super::{AuthorPayload, Timestamp, strings};
use scribe_core::activity::{ActiveChange, CatalogedBy};
use scribe_core::{Child, Entry, Segment, SessionControl, SessionId, SessionRecord, Term};
use serde::Serialize;
use std::collections::BTreeMap;

/// The last change of a session's `active` or `keep_active`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ActiveChangePayload {
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) by: AuthorPayload,
    /// `age`, `removed` or `manual`.
    pub(crate) route: &'static str,
    pub(crate) reason: String,
    pub(crate) cite: Option<String>,
}

impl From<&ActiveChange> for ActiveChangePayload {
    fn from(change: &ActiveChange) -> Self {
        Self {
            at: change.at,
            by: AuthorPayload::from(&change.by),
            route: change.route.as_str(),
            reason: change.reason.clone(),
            cite: change.cite.clone(),
        }
    }
}

/// Who cataloged a session by name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CatalogedByPayload {
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) by: AuthorPayload,
}

impl From<&CatalogedBy> for CatalogedByPayload {
    fn from(c: &CatalogedBy) -> Self {
        Self {
            at: c.at,
            by: AuthorPayload::from(&c.by),
        }
    }
}

/// One segment of a session's log, with its text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SegmentPayload {
    pub(crate) n: u32,
    pub(crate) heading: String,
    pub(crate) first_message: Option<String>,
    pub(crate) last_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) from: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) to: Timestamp,
    /// `open` or `sealed`.
    pub(crate) state: &'static str,
    pub(crate) windows: u32,
    /// `pass`, `rebuild`, `catalog` or `upgrade`.
    pub(crate) origin: &'static str,
    pub(crate) topics: Vec<String>,
    pub(crate) related: Vec<String>,
    /// Model-written, never the user's words.
    pub(crate) text: String,
}

impl From<&Segment> for SegmentPayload {
    fn from(s: &Segment) -> Self {
        Self {
            n: s.n.get(),
            heading: s.heading.clone(),
            first_message: s.first_message.clone(),
            last_message: s.last_message.clone(),
            from: s.from,
            to: s.to,
            state: s.state.as_str(),
            windows: s.windows.get(),
            origin: s.origin.as_str(),
            topics: strings(s.topics.as_slice()),
            related: strings(&s.related),
            text: s.text.clone(),
        }
    }
}

/// A phrase the user coined or defined, with their defining words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TermPayload {
    pub(crate) term: String,
    /// Verbatim from the user's turn.
    pub(crate) said: String,
    /// The uuid of that turn's message.
    pub(crate) message: String,
}

impl From<&Term> for TermPayload {
    fn from(t: &Term) -> Self {
        Self {
            term: t.term.clone(),
            said: t.said.clone(),
            message: t.message.clone(),
        }
    }
}

/// A link a session record stores: how it relates to another session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct LinkPayload {
    /// `spawned_from` or `continues`.
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    /// The session linked to.
    pub(crate) session: String,
    /// That session's record title; `null` when it has no record.
    pub(crate) title: Option<String>,
}

/// A session that links to this one, derived at read time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ChildPayload {
    /// How the child relates to this session: `spawned_from` or `continues`.
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    /// The child session.
    pub(crate) session: String,
    /// Its record title.
    pub(crate) title: String,
}

impl From<&Child> for ChildPayload {
    fn from(child: &Child) -> Self {
        Self {
            kind: child.kind.as_str(),
            session: child.session.to_string(),
            title: child.title.clone(),
        }
    }
}

/// What a session payload shows besides its own record: the sessions that
/// link to it and the titles of the records it links to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Kin<'a> {
    pub(crate) children: &'a [Child],
    pub(crate) titles: &'a BTreeMap<SessionId, String>,
}

impl<'a> Kin<'a> {
    /// `id`'s kin from a listing's derived children and titles.
    pub(crate) fn of(
        id: &SessionId,
        children: &'a BTreeMap<SessionId, Vec<Child>>,
        titles: &'a BTreeMap<SessionId, String>,
    ) -> Self {
        Self {
            children: children.get(id).map_or(&[][..], Vec::as_slice),
            titles,
        }
    }
}

/// One agent-written handoff entry shown beside a session's header
/// (docs/design/scribe/2_design.md §16).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct HandoffPayload {
    pub(crate) id: String,
    pub(crate) author: AuthorPayload,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: Timestamp,
    pub(crate) project: String,
    pub(crate) quote: Option<String>,
    pub(crate) content: String,
    pub(crate) handoff_mode: Option<scribe_core::handoff::Mode>,
}

impl From<&Entry> for HandoffPayload {
    fn from(entry: &Entry) -> Self {
        Self {
            id: entry.id.to_string(),
            author: AuthorPayload::from(&entry.author),
            created: entry.created,
            project: entry.project.to_string(),
            quote: entry.quote.clone(),
            content: entry.content.clone(),
            handoff_mode: entry.handoff_mode,
        }
    }
}

/// `session show <id>` for a session with no session record, only
/// agent-written handoffs (2_design.md §16). Carries its switch state when
/// switched on before its first pass, and never fabricates record timestamps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SessionHandoffsPayload {
    pub(crate) id: String,
    pub(crate) has_record: bool,
    pub(crate) project: String,
    pub(crate) on: bool,
    pub(crate) due: bool,
    pub(crate) summary: String,
    pub(crate) goals: String,
    pub(crate) next: String,
    pub(crate) handoffs: Vec<HandoffPayload>,
}

impl SessionHandoffsPayload {
    pub(crate) fn new(
        id: &SessionId,
        control: Option<&SessionControl>,
        handoffs: &[Entry],
    ) -> Self {
        let project = handoffs
            .first()
            .map_or_else(|| String::from("global"), |e| e.project.to_string());
        Self {
            id: id.to_string(),
            has_record: false,
            project,
            on: control.is_some_and(|c| c.on),
            due: control.is_some_and(|c| c.due),
            summary: String::new(),
            goals: String::new(),
            next: String::new(),
            handoffs: handoffs.iter().map(HandoffPayload::from).collect(),
        }
    }
}

/// One session record, with its switch, as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SessionPayload {
    pub(crate) id: String,
    /// The session's `s-<n>` (2_design.md §15.7), accepted wherever an id
    /// is; `id` stays the provider's session id, which cites use.
    pub(crate) short_id: Option<String>,
    /// `open` or `final`.
    pub(crate) status: &'static str,
    /// When `#scribe forget` erased the session's words (2_design.md
    /// §15.7), or `null`.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) forgotten: Option<Timestamp>,
    pub(crate) project: String,
    pub(crate) topics: Vec<String>,
    pub(crate) related: Vec<String>,
    /// The links this session stores (`spawned_from`, `continues`).
    pub(crate) links: Vec<LinkPayload>,
    /// The sessions that link to this one, derived at read time.
    pub(crate) children: Vec<ChildPayload>,
    /// The user's own words, unlike the four model-written sections.
    pub(crate) terms: Vec<TermPayload>,
    pub(crate) provider: String,
    pub(crate) host: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: Timestamp,
    pub(crate) title: String,
    pub(crate) cwd: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) started: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) on_since: Timestamp,
    pub(crate) scribed_through: Option<String>,
    pub(crate) passes: u32,
    pub(crate) next_open: u32,
    /// Model-written, never the user's words.
    pub(crate) summary: String,
    pub(crate) goals: String,
    /// Always `""` from store format 4: what it held is in `log`. Kept so
    /// no reader of the field breaks.
    pub(crate) outcome: String,
    pub(crate) next: String,
    /// Whether cataloging is on now (`control.json`); `false` when the
    /// session has no switch.
    pub(crate) on: bool,
    /// Whether a pass is due and not yet run.
    pub(crate) due: bool,
    /// Whether the record still describes something current.
    pub(crate) active: bool,
    /// Exempt from automatic deactivation.
    pub(crate) keep_active: bool,
    pub(crate) active_change: Option<ActiveChangePayload>,
    pub(crate) cataloged_by: Option<CatalogedByPayload>,
    /// The agent that ran `session rebuild` to write this record, or `null`.
    pub(crate) rebuilt_by: Option<AuthorPayload>,
    /// The log's segments with their text, oldest first (one with
    /// `--segment`); absent from a list, whose rows carry no log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) log: Option<Vec<SegmentPayload>>,
    /// Active agent-written handoffs for this session, newest first; absent
    /// from a list, whose rows carry no handoffs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) handoffs: Option<Vec<HandoffPayload>>,
    /// Whether the table view prints each segment's text (`--log`,
    /// `--segment`) or only the table of contents.
    #[serde(skip)]
    pub(crate) log_text: bool,
}

impl SessionPayload {
    pub(crate) fn new(
        record: &SessionRecord,
        control: Option<&SessionControl>,
        kin: Kin<'_>,
    ) -> Self {
        Self {
            id: record.id.to_string(),
            short_id: None,
            status: record.status.as_str(),
            forgotten: None,
            project: record.project.to_string(),
            topics: strings(record.topics.as_slice()),
            related: strings(&record.related),
            links: record
                .links
                .iter()
                .map(|l| LinkPayload {
                    kind: l.kind.as_str(),
                    session: l.session.to_string(),
                    title: kin.titles.get(&l.session).cloned(),
                })
                .collect(),
            children: kin.children.iter().map(ChildPayload::from).collect(),
            terms: record.terms.iter().map(TermPayload::from).collect(),
            provider: record.provider.to_string(),
            host: record.host.to_string(),
            created: record.created,
            updated: record.updated,
            title: record.title.clone(),
            cwd: record.cwd.clone(),
            started: record.started,
            on_since: record.on_since,
            scribed_through: record.scribed_through.clone(),
            passes: record.passes,
            next_open: record.next_open,
            summary: record.summary.clone(),
            goals: record.goals.clone(),
            outcome: String::new(),
            next: record.next.clone(),
            on: control.is_some_and(|c| c.on),
            due: control.is_some_and(|c| c.due),
            active: record.active,
            keep_active: record.keep_active,
            active_change: record.active_change.as_ref().map(ActiveChangePayload::from),
            cataloged_by: record.cataloged_by.as_ref().map(CatalogedByPayload::from),
            rebuilt_by: record.rebuilt_by.as_ref().map(AuthorPayload::from),
            log: None,
            handoffs: None,
            log_text: false,
        }
    }

    /// With its active handoff entries, newest first, for `session show`.
    pub(crate) fn with_handoffs<'a>(
        mut self,
        handoffs: impl IntoIterator<Item = &'a Entry>,
    ) -> Self {
        self.handoffs = Some(handoffs.into_iter().map(HandoffPayload::from).collect());
        self
    }

    /// With when it was forgotten.
    pub(crate) fn with_forgotten(mut self, at: Option<Timestamp>) -> Self {
        self.forgotten = at;
        self
    }

    /// With its `s-<n>`.
    pub(crate) fn with_number(mut self, number: Option<std::num::NonZeroU64>) -> Self {
        self.short_id = number.map(|n| format!("s-{n}"));
        self
    }

    /// With `segments` as its `log`: a `show` carries the whole log, or the
    /// one segment asked for.
    pub(crate) fn with_log<'a>(mut self, segments: impl IntoIterator<Item = &'a Segment>) -> Self {
        self.log = Some(segments.into_iter().map(SegmentPayload::from).collect());
        self
    }
}
