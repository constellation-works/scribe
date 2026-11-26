//! Payloads and rendering for `scribe session` (records, logs, handoffs and
//! pass reports).

use super::common::console::{Sink, notice};
use super::common::detail::{Detail, emit_detail, field, field_role, joined_lines};
use super::common::record::{AuthorPayload, Timestamp, WarningPayload, author, envelope, strings};
use super::common::table::{
    Cell, Result, cut, due_role, joined, left, list_shorts, list_timestamp, one_line, or_dash,
    plain_line, right, shown_id, table, timestamp, yes_no,
};
use cw_cli::{Column, Mode, Role};
use scribe_core::activity::{ActiveChange, CatalogedBy};
use scribe_core::{
    Child, Entry, Segment, SessionControl, SessionId, SessionRecord, ShortIds, Term,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::io::Write;

pub(crate) use scribe_core::apps::session::{PassPayload, PassReport};

/// Whether `session list` includes an `ACTIVE` column (`--include-inactive`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveColumn {
    Hidden,
    Shown,
}

impl ActiveColumn {
    pub(crate) fn from_flag(shown: bool) -> Self {
        if shown { Self::Shown } else { Self::Hidden }
    }

    fn is_shown(self) -> bool {
        match self {
            Self::Hidden => false,
            Self::Shown => true,
        }
    }
}

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

/// One row of `session list --tree`. Its JSON is the session's alone: the
/// nesting is in each session's `links` and `children`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct TreeRow {
    /// How deep it nests: 0 for a root.
    #[serde(skip)]
    pub(crate) depth: usize,
    /// The session it nests under, if it has one in the list.
    #[serde(skip)]
    pub(crate) parent: Option<String>,
    pub(crate) session: SessionPayload,
}

/// The run's error of a [`PassReport`].
pub(crate) trait PassFailure {
    /// The run's error when any pass failed, naming each failed session,
    /// code and short reason; `None` when none did.
    fn failure(&self) -> Option<crate::error::CliError>;
}

impl PassFailure for PassReport {
    fn failure(&self) -> Option<crate::error::CliError> {
        let failed: Vec<String> = self
            .passes
            .iter()
            .filter_map(|p| {
                let code = p.error_code?;
                let reason = p.error.as_deref().unwrap_or("reason unavailable");
                Some(format!("{} {code}: {reason}", p.session))
            })
            .collect();
        (!failed.is_empty()).then(|| {
            if self.catalog {
                return crate::error::CliError::PassFailed(format!(
                    "{} session catalog(s) failed: {}; the switch stays due; run 'orbit job run scribe-session-catalog' for the session again to resume",
                    failed.len(),
                    failed.join(", ")
                ));
            }
            if self.passes.iter().any(|p| p.author.is_some()) {
                return crate::error::CliError::PassFailed(format!(
                    "{} session rebuild(s) failed: {}; checkpoint kept; run 'orbit job run scribe-session-rebuild' for the session again to resume",
                    failed.len(),
                    failed.join(", ")
                ));
            }
            crate::error::CliError::PassFailed(format!(
                "{} session pass(es) failed: {}; each stays due with retry backoff; use 'orbit job run scribe-session-pass' with explicit sessions to retry now",
                failed.len(),
                failed.join(", ")
            ))
        })
    }
}

/// Render a `session list` result.
pub(crate) fn list<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    items: &[SessionPayload],
    active_column: ActiveColumn,
    total: usize,
    warnings: &[WarningPayload],
) -> Result {
    let json = envelope("sessions", items, total, warnings);
    let mode = sink.terminal().mode();
    let active = active_column.is_shown();
    sink.emit(&json, |_| {
        let list_ids = list_shorts(mode, shorts);
        let columns = with_active(session_columns(), active, 2);
        let rows = items
            .iter()
            .map(|s| session_row(s, active, list_ids.as_deref(), mode))
            .collect();
        table(mode, columns, rows)
    })
}

/// Render a `session list --tree` result.
pub(crate) fn list_tree<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    rows: &[TreeRow],
    active_column: ActiveColumn,
    total: usize,
    warnings: &[WarningPayload],
) -> Result {
    let json = envelope("sessions", rows, total, warnings);
    let mode = sink.terminal().mode();
    let active = active_column.is_shown();
    sink.emit(&json, |_| {
        let list_ids = list_shorts(mode, shorts);
        let columns = with_active(session_tree_columns(), active, 3);
        let table_rows = rows
            .iter()
            .map(|row| tree_row(row, list_ids.as_deref(), active, mode))
            .collect();
        table(mode, columns, table_rows)
    })
}

/// Render a single session (`session show` and session write verbs).
pub(crate) fn show<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    session: &SessionPayload,
) -> Result {
    emit_detail(sink, session, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => session_plain(out, shorts, session),
        Mode::Table => session_detail(out, session),
    })
}

/// Render `session show` for a handoff-only session.
pub(crate) fn show_handoffs<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    session: &SessionHandoffsPayload,
) -> Result {
    emit_detail(sink, session, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => session_handoffs_plain(out, shorts, session),
        Mode::Table => session_handoffs_detail(out, session),
    })
}

/// A session pass's report: a table or tab-separated lines, one per
/// session passed, or `{"passes": [...], "remaining": n}`. Nothing passed
/// prints nothing but the JSON object.
pub(crate) fn emit_pass<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    report: &PassReport,
) -> Result {
    for pass in &report.passes {
        for warning in &pass.warnings {
            sink.warn(&format!("session {}: {warning}", pass.session));
        }
    }
    for text in &report.notices {
        notice(sink, text);
    }
    let mode = sink.terminal().mode();
    sink.emit(report, |report| {
        let list_ids = list_shorts(mode, shorts);
        let rows = report
            .passes
            .iter()
            .map(|p| pass_row(p, list_ids.as_deref()))
            .collect();
        table(mode, pass_columns(), rows)
    })
}

/// Render a session pass prepare or apply result (always JSON).
pub(crate) fn emit_pass_step<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    value: &serde_json::Value,
) -> Result {
    sink.emit_text(value, |_| String::new())
}

fn session_columns() -> Vec<Column> {
    vec![
        left("ID"),
        left("STATUS"),
        left("DUE"),
        left("PROJECT"),
        left("TOPICS"),
        left("UPDATED"),
        cut("TITLE"),
    ]
}

fn session_tree_columns() -> Vec<Column> {
    vec![
        left("ID"),
        left("PARENT"),
        left("STATUS"),
        left("DUE"),
        left("PROJECT"),
        left("TOPICS"),
        left("UPDATED"),
        cut("TITLE"),
    ]
}

fn pass_columns() -> Vec<Column> {
    vec![
        left("SESSION"),
        left("RESULT"),
        left("CODE"),
        right("PASSES"),
        right("TERMS DROPPED"),
    ]
}

/// `columns` with an `ACTIVE` column inserted at `at` when `active`.
fn with_active(mut columns: Vec<Column>, active: bool, at: usize) -> Vec<Column> {
    if active {
        columns.insert(at, left("ACTIVE"));
    }
    columns
}

/// A `session list --tree` row: the session's row with its parent after
/// the id; in a table the title is indented two spaces per level.
fn tree_row(row: &TreeRow, shorts: Option<&ShortIds>, active: bool, mode: Mode) -> Vec<Cell> {
    let mut cells = session_row(&row.session, active, shorts, mode);
    cells.insert(
        1,
        Cell::plain(
            row.parent
                .as_deref()
                .map_or_else(|| String::from("-"), |parent| shown_id(parent, shorts)),
        ),
    );
    if mode == Mode::Table
        && let Some(title) = cells.last_mut()
    {
        title.text = format!("{}{}", "  ".repeat(row.depth), title.text);
    }
    cells
}

/// A session's list row; `active` adds its `active` column after the
/// status (`session list --include-inactive`). `shorts` is set for a
/// non-JSON output: it shows the display id. Plain and JSON output
/// keep the full instant; the `updated` cell is the UTC date
/// in table mode (see [`list_timestamp`]).
fn session_row(
    s: &SessionPayload,
    active: bool,
    shorts: Option<&ShortIds>,
    mode: Mode,
) -> Vec<Cell> {
    let updated = list_timestamp(s.updated, mode);
    let mut cells = vec![
        Cell::plain(shown_id(&s.id, shorts)),
        Cell::plain(s.status),
        Cell {
            text: yes_no(s.due).to_owned(),
            role: due_role(s.due),
        },
        Cell::plain(s.project.clone()),
        Cell::plain(joined(&s.topics)),
        Cell::plain(updated),
        Cell::plain(one_line(&s.title)),
    ];
    if active {
        cells.insert(
            2,
            Cell {
                text: yes_no(s.active).to_owned(),
                role: if s.active { Role::Neutral } else { Role::Muted },
            },
        );
    }
    cells
}

fn pass_row(p: &PassPayload, shorts: Option<&ShortIds>) -> Vec<Cell> {
    vec![
        Cell::plain(shown_id(&p.session, shorts)),
        Cell {
            text: p.result.to_owned(),
            role: if p.result == "failed" {
                Role::Warn
            } else {
                Role::Neutral
            },
        },
        Cell::plain(p.error_code.unwrap_or("-")),
        Cell::plain(
            p.passes
                .map_or_else(|| String::from("-"), |n| n.to_string()),
        ),
        Cell::plain(p.dropped_terms.to_string()),
    ]
}

fn session_plain(out: &mut Detail, shorts: &ShortIds, session: &SessionPayload) -> fmt::Result {
    writeln!(
        out,
        "{}",
        plain_line(&session_row(session, false, Some(shorts), Mode::Plain))
    )?;
    if let Some(at) = session.forgotten {
        writeln!(out, "forgotten\t{}", timestamp(at))?;
    }
    for handoff in session.handoffs.iter().flatten() {
        writeln!(
            out,
            "handoff\t{}\t{}\t{}\t{}",
            handoff.id,
            author(&handoff.author),
            timestamp(handoff.created),
            format_args!(
                "{}{}",
                handoff
                    .handoff_mode
                    .map_or(String::new(), |m| format!("mode: {} ", m.as_str())),
                one_line(&handoff.content)
            )
        )?;
    }
    for segment in session.log.iter().flatten() {
        writeln!(
            out,
            "segment\t{}\t{}\t{}\t{}\t{}",
            segment.n,
            segment.state,
            timestamp(segment.from),
            timestamp(segment.to),
            one_line(&segment.heading)
        )?;
    }
    Ok(())
}

fn session_handoffs_plain(
    out: &mut Detail,
    shorts: &ShortIds,
    session: &SessionHandoffsPayload,
) -> fmt::Result {
    let latest = session
        .handoffs
        .first()
        .map_or_else(|| String::from("-"), |h| timestamp(h.created));
    writeln!(
        out,
        "{}\thandoff-only\t{}\t{}\t-\t{}\tno session record",
        shown_id(&session.id, Some(shorts)),
        yes_no(session.due),
        session.project,
        latest
    )?;
    for handoff in &session.handoffs {
        writeln!(
            out,
            "handoff\t{}\t{}\t{}\t{}",
            handoff.id,
            author(&handoff.author),
            timestamp(handoff.created),
            format_args!(
                "{}{}",
                handoff
                    .handoff_mode
                    .map_or(String::new(), |m| format!("mode: {} ", m.as_str())),
                one_line(&handoff.content)
            )
        )?;
    }
    Ok(())
}

fn session_detail(out: &mut Detail, s: &SessionPayload) -> fmt::Result {
    field(out, "id", &s.id)?;
    if let Some(short) = &s.short_id {
        field(out, "short id", short)?;
    }
    field(out, "title", or_dash(&s.title))?;
    field(out, "status", s.status)?;
    if let Some(at) = s.forgotten {
        field(out, "forgotten", &timestamp(at))?;
    }
    field(out, "active", yes_no(s.active))?;
    field(out, "keep active", yes_no(s.keep_active))?;
    let change = s.active_change.as_ref().map_or_else(
        || String::from("-"),
        |c| {
            format!(
                "{} {} by {}:{} ({}){}",
                timestamp(c.at),
                c.route,
                c.by.kind,
                c.by.name,
                one_line(&c.reason),
                c.cite
                    .as_ref()
                    .map_or_else(String::new, |cite| format!(", cite {cite}"))
            )
        },
    );
    field(out, "change", &change)?;
    let cataloged = s.cataloged_by.as_ref().map_or_else(
        || String::from("-"),
        |c| format!("{} by {}:{}", timestamp(c.at), c.by.kind, c.by.name),
    );
    field(out, "cataloged", &cataloged)?;
    field(out, "on", yes_no(s.on))?;
    field_role(out, "due", yes_no(s.due), due_role(s.due))?;
    field(out, "project", &s.project)?;
    field(out, "topics", &joined(&s.topics))?;
    field(out, "related", &joined(&s.related))?;
    let links: Vec<String> = s
        .links
        .iter()
        .map(|l| match &l.title {
            Some(title) => format!("{} {} ({})", l.kind, l.session, one_line(title)),
            None => format!("{} {}", l.kind, l.session),
        })
        .collect();
    field(out, "links", &joined_lines(&links))?;
    let children: Vec<String> = s
        .children
        .iter()
        .map(|c| format!("{} {} ({})", c.session, c.kind, one_line(&c.title)))
        .collect();
    field(out, "children", &joined_lines(&children))?;
    field(out, "provider", &s.provider)?;
    field(out, "host", &s.host)?;
    field(out, "cwd", &s.cwd)?;
    field(out, "started", &timestamp(s.started))?;
    field(out, "on since", &timestamp(s.on_since))?;
    field(out, "updated", &timestamp(s.updated))?;
    field(out, "passes", &s.passes.to_string())?;
    field(out, "next open", &s.next_open.to_string())?;
    field(out, "through", s.scribed_through.as_deref().unwrap_or("-"))?;
    // One line per term: the phrase, then Daniel's defining words.
    let terms: Vec<String> = s
        .terms
        .iter()
        .map(|t| format!("{}: \"{}\" (message {})", t.term, t.said, t.message))
        .collect();
    field(out, "terms", &joined_lines(&terms))?;
    if let Some(handoffs) = s.handoffs.as_deref().filter(|h| !h.is_empty()) {
        session_handoffs(out, handoffs)?;
    }
    for (name, text) in [
        ("Summary", &s.summary),
        ("Goals", &s.goals),
        ("Next", &s.next),
    ] {
        writeln!(out)?;
        writeln!(out, "## {name}")?;
        writeln!(out, "{}", or_dash(text))?;
    }
    session_log(out, s)
}

fn session_handoffs_detail(out: &mut Detail, s: &SessionHandoffsPayload) -> fmt::Result {
    field(out, "id", &s.id)?;
    field(
        out,
        "record",
        if s.on || s.due {
            "none (handoff only; no catalog record yet)"
        } else {
            "none (handoff only; session was not switched on)"
        },
    )?;
    field(out, "on", yes_no(s.on))?;
    field_role(out, "due", yes_no(s.due), due_role(s.due))?;
    field(out, "project", &s.project)?;
    session_handoffs(out, &s.handoffs)
}

fn session_handoffs(out: &mut Detail, handoffs: &[HandoffPayload]) -> fmt::Result {
    writeln!(out)?;
    writeln!(out, "## Handoffs")?;
    for h in handoffs {
        writeln!(out)?;
        writeln!(
            out,
            "### {} · {} · {}",
            h.id,
            author(&h.author),
            timestamp(h.created)
        )?;
        if let Some(mode) = h.handoff_mode {
            writeln!(out, "mode: {}", mode.as_str())?;
        }
        writeln!(out, "{}", h.content.trim_end())?;
    }
    Ok(())
}

fn session_log(out: &mut Detail, s: &SessionPayload) -> fmt::Result {
    let log = s.log.as_deref().unwrap_or_default();
    writeln!(out)?;
    writeln!(out, "## Log")?;
    if log.is_empty() {
        writeln!(out, "-")?;
        return Ok(());
    }
    // The table of contents: one line per segment.
    for segment in log {
        writeln!(
            out,
            "{:>3}  {:<6}  {}  {}  {}",
            segment.n,
            segment.state,
            timestamp(segment.from),
            timestamp(segment.to),
            one_line(&segment.heading)
        )?;
    }
    if s.log_text {
        for segment in log {
            writeln!(out)?;
            writeln!(out, "### {}. {}", segment.n, segment.heading)?;
            writeln!(out, "{}", or_dash(&segment.text))?;
        }
    }
    Ok(())
}
