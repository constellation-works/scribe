mod detail;

pub(crate) use detail::show;

use super::Sink;
use super::color::role_for_status;
use super::payload::{
    EntryPayload, HitPayload, HookJson, HookReply, HookSpecificJson, PassPayload, PassReport,
    PluginReply, ProposalPayload, Records, ScanReport, SessionPayload, TagPayload,
    TranscriptPayload, TreeRow, WarningPayload,
};
use cw_cli::{Cell as KitCell, Column, Mode, Role, Table};
use scribe_core::ShortIds;
use serde::ser::{Serialize, SerializeMap, Serializer};
use std::io::Write;
use time::format_description::well_known::{Iso8601, Rfc3339};
use time::{OffsetDateTime, UtcOffset};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The kit's result of a write; the binary's error wraps its failure.
type Result = cw_cli::Result<()>;

fn left(header: &'static str) -> Column {
    Column::new(header)
}

fn right(header: &'static str) -> Column {
    Column::new(header).right()
}

fn cut(header: &'static str) -> Column {
    Column::new(header).truncatable()
}

fn entry_columns() -> Vec<Column> {
    vec![
        left("ID"),
        left("STATUS"),
        left("PROJECT"),
        left("TOPICS"),
        left("CREATED"),
        cut("CONTENT"),
        left("AUTHOR"),
        right("AGE"),
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

fn proposal_columns() -> Vec<Column> {
    vec![
        left("ID"),
        left("KIND"),
        left("STATE"),
        left("OLDER"),
        left("NEWER"),
        left("OPENED"),
        cut("REASON"),
        cut("ADMITTED"),
        left("CLOSED_REASON"),
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

fn scan_columns() -> Vec<Column> {
    vec![
        left("ACTION"),
        left("ID"),
        left("WHAT"),
        left("OLDER"),
        left("NEWER"),
    ]
}

fn tag_columns() -> Vec<Column> {
    vec![
        left("NAME"),
        right("ENTRIES"),
        right("STALE DAYS"),
        left("ALIASES"),
        cut("DESCRIPTION"),
        left("SOURCE"),
        left("ACTION"),
        right("RETIRED"),
        right("REJECTED"),
        left("ROUTE"),
    ]
}

fn transcript_columns() -> Vec<Column> {
    vec![
        left("SESSION"),
        left("PROJECT"),
        right("USER_TURNS"),
        right("BYTES"),
        left("STARTED"),
        left("ENDED"),
        cut("FIRST_PROMPT"),
    ]
}

fn mark_columns() -> Vec<Column> {
    vec![
        left("SESSION"),
        cut("MESSAGE"),
        right("MARKER"),
        left("OUTCOME"),
        left("CODE"),
        right("ATTEMPTS"),
        left("AT"),
    ]
}

fn hit_columns() -> Vec<Column> {
    vec![
        left("CITE"),
        left("KIND"),
        right("SCORE"),
        left("UPDATED"),
        left("TITLE"),
        cut("SNIPPET"),
    ]
}

/// A list's `--json` form: [`Envelope`] around its records.
struct ListJson<'a> {
    records: &'a Records,
    total: usize,
    warnings: &'a [WarningPayload],
    message_matches: Option<usize>,
}

impl Serialize for ListJson<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let (total, warnings) = (self.total, self.warnings);
        match self.records {
            Records::Groups(items) => {
                envelope("items", items, total, warnings).serialize(serializer)
            }
            Records::Entries(items) => {
                envelope("entries", items, total, warnings).serialize(serializer)
            }
            Records::Sessions { items, .. } => {
                envelope("sessions", items, total, warnings).serialize(serializer)
            }
            Records::SessionTree { rows, .. } => {
                envelope("sessions", rows, total, warnings).serialize(serializer)
            }
            Records::Proposals(items) => {
                envelope("proposals", items, total, warnings).serialize(serializer)
            }
            Records::Tags(items) => envelope("tags", items, total, warnings).serialize(serializer),
            Records::Hits(items) => Envelope {
                message_matches: self.message_matches,
                ..envelope("hits", items, total, warnings)
            }
            .serialize(serializer),
            Records::Transcripts(items) => {
                envelope("transcripts", items, total, warnings).serialize(serializer)
            }
            Records::Marks(items) => {
                envelope("marks", items, total, warnings).serialize(serializer)
            }
        }
    }
}

/// The `--json` form of a list: always `{"<records>": [...], "total",
/// "truncated", "warnings"}`, whatever the flags, so its shape never depends
/// on them (STD-01 R34, R10). Only a search that hints at stored messages
/// adds `message_matches` after them, so every other list is unchanged.
struct Envelope<'a, T> {
    key: &'static str,
    items: &'a [T],
    total: usize,
    warnings: &'a [WarningPayload],
    message_matches: Option<usize>,
}

impl<T: Serialize> Serialize for Envelope<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let len = if self.message_matches.is_some() { 5 } else { 4 };
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry(self.key, self.items)?;
        map.serialize_entry("total", &self.total)?;
        map.serialize_entry("truncated", &(self.items.len() < self.total))?;
        map.serialize_entry("warnings", self.warnings)?;
        if let Some(count) = self.message_matches {
            map.serialize_entry("message_matches", &count)?;
        }
        map.end()
    }
}

/// A list: table, tab-separated lines, or the JSON envelope. `total` is how
/// many matched; `message_matches` is the count a search's hint gives.
pub(crate) fn list<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    records: &Records,
    total: usize,
    warnings: &[WarningPayload],
    message_matches: Option<usize>,
) -> Result {
    let json = ListJson {
        records,
        total,
        warnings,
        message_matches,
    };
    let mode = sink.terminal().mode();
    sink.emit(&json, |_| list_table(mode, shorts, records))
}

/// A list as the table its human modes show; no records is no rows.
fn list_table(mode: Mode, shorts: &ShortIds, records: &Records) -> Table {
    let prefixes = shorts.prefixes_only();
    let list_ids = if mode == Mode::Table {
        &prefixes
    } else {
        shorts
    };
    let (columns, rows): (Vec<Column>, Vec<Vec<Cell>>) = match records {
        // The table holds short values only; the tab-separated form keeps
        // every field, and `group show` and `--json` carry them all.
        Records::Groups(items) if mode == Mode::Table => (
            group_list_columns(),
            items.iter().map(group_list_row).collect(),
        ),
        Records::Groups(items) => (
            GROUP_COLUMNS.iter().map(|label| left(label)).collect(),
            items.iter().map(|g| group_row(g, Some(shorts))).collect(),
        ),
        // A STATE column, last so the other columns keep their places, only
        // when the list holds an open item (2_design.md §18.5).
        Records::Entries(items) if items.iter().any(|e| e.state.is_some()) => (
            entry_columns().into_iter().chain([left("STATE")]).collect(),
            items
                .iter()
                .map(|entry| {
                    let mut row = entry_row(entry, mode);
                    row.push(Cell::plain(entry.state.unwrap_or("-")));
                    row
                })
                .collect(),
        ),
        Records::Entries(items) => (
            entry_columns(),
            items.iter().map(|entry| entry_row(entry, mode)).collect(),
        ),
        Records::Sessions {
            items,
            active_column,
        } => (
            with_active(session_columns(), *active_column, 2),
            items
                .iter()
                .map(|s| session_row(s, *active_column, table_shorts(mode, list_ids), mode))
                .collect(),
        ),
        Records::SessionTree {
            rows,
            active_column,
        } => (
            with_active(session_tree_columns(), *active_column, 3),
            rows.iter()
                .map(|row| tree_row(row, table_shorts(mode, list_ids), *active_column, mode))
                .collect(),
        ),
        Records::Proposals(items) => (
            proposal_columns(),
            items
                .iter()
                .flat_map(|p| proposal_rows(p, mode, shorts))
                .collect(),
        ),
        Records::Tags(items) => (tag_columns(), items.iter().map(tag_row).collect()),
        Records::Hits(items) => (
            hit_columns(),
            items.iter().map(|h| hit_row(h, mode, shorts)).collect(),
        ),
        Records::Transcripts(items) => (
            transcript_columns(),
            items
                .iter()
                .map(|t| transcript_row(t, mode, table_shorts(mode, list_ids)))
                .collect(),
        ),
        Records::Marks(items) => (
            mark_columns(),
            items
                .iter()
                .map(|m| mark_row(m, mode, table_shorts(mode, shorts)))
                .collect(),
        ),
    };
    table(mode, columns, rows)
}

/// One cell of a row as scribe builds it: its text, which the plain line and
/// the detail view read back, and what its color means.
#[derive(Clone, Debug)]
pub(super) struct Cell {
    /// The text; an absent value is `-`, never empty (STD-01 R14), except in a
    /// continuation row.
    pub(super) text: String,
    /// Semantic color role.
    pub(super) role: Role,
}

impl Cell {
    /// A cell with no emphasis.
    pub(super) fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: Role::Neutral,
        }
    }
}

/// The kit table for `rows`. In a table an empty cell is an absent value and
/// shows as `-` (STD-01 R14); a row whose first cell is empty continues the
/// row above it (a proposal's quote line), so its blanks stay blank.
fn table(mode: Mode, columns: Vec<Column>, rows: Vec<Vec<Cell>>) -> Table {
    let mut table = Table::new(columns);
    for row in rows {
        let continuation = row.first().is_some_and(|cell| cell.text.is_empty());
        table.push(row.into_iter().map(|cell| {
            let kit = if cell.text.is_empty() && mode == Mode::Table && !continuation {
                KitCell::absent()
            } else {
                KitCell::new(cell.text)
            };
            kit.role(cell.role)
        }));
    }
    table
}

/// Display names for non-JSON output; JSON retains provider ids.
fn table_shorts(mode: Mode, shorts: &ShortIds) -> Option<&ShortIds> {
    (mode != Mode::Json).then_some(shorts)
}

/// A session id in its display form when `shorts` is set.
fn shown_id(id: &str, shorts: Option<&ShortIds>) -> String {
    shorts.map_or_else(|| id.to_owned(), |s| s.display(id).to_owned())
}

/// Render a session citation without changing its segment suffix.
fn shown_cite(cite: &str, shorts: &ShortIds) -> String {
    let Some(session) = cite.strip_prefix("session:") else {
        return cite.to_owned();
    };
    let (id, suffix) = session
        .split_once('#')
        .map_or((session, ""), |(id, _)| (id, &session[id.len()..]));
    format!("session:{}{suffix}", shorts.display(id))
}

/// `columns` with an `ACTIVE` column inserted at `at` when `active`.
fn with_active(mut columns: Vec<Column>, active: bool, at: usize) -> Vec<Column> {
    if active {
        columns.insert(at, left("ACTIVE"));
    }
    columns
}

fn envelope<'a, T>(
    key: &'static str,
    items: &'a [T],
    total: usize,
    warnings: &'a [WarningPayload],
) -> Envelope<'a, T> {
    Envelope {
        key,
        items,
        total,
        warnings,
        message_matches: None,
    }
}

/// A hook's reply: one line of Claude Code hook JSON, or nothing when the
/// hook has nothing to say. The mode is ignored: Claude Code reads this, so
/// the sink it goes through is one that writes compact JSON.
pub(crate) fn hook<O: Write, E: Write>(sink: &mut Sink<O, E>, reply: &HookReply) -> Result {
    if let Some(reason) = &reply.block {
        let json = HookJson {
            decision: Some("block"),
            reason: Some(reason),
            system_message: None,
            hook_specific_output: None,
        };
        return sink.emit_text(&json, |_| String::new());
    }
    if reply.message.is_none() && reply.context.is_none() {
        return Ok(());
    }
    let json = HookJson {
        decision: None,
        reason: None,
        system_message: reply.message.as_deref(),
        hook_specific_output: reply.context.as_deref().map(|context| HookSpecificJson {
            hook_event_name: reply.event,
            additional_context: context,
        }),
    };
    sink.emit_text(&json, |_| String::new())
}

/// A plugin call's envelope: one JSON line, whatever the mode, since the
/// Orbit host reads it.
pub(crate) fn plugin<O: Write, E: Write>(sink: &mut Sink<O, E>, reply: &PluginReply) -> Result {
    #[derive(serde::Serialize)]
    struct Ok<'a, T: serde::Serialize> {
        ok: bool,
        output: &'a T,
    }
    #[derive(serde::Serialize)]
    struct Failed<'a> {
        ok: bool,
        error: &'a super::payload::PluginError,
    }
    match reply {
        PluginReply::Scan(report) => sink.emit_text(
            &Ok {
                ok: true,
                output: report.as_ref(),
            },
            |_| String::new(),
        ),
        PluginReply::Data(value) => sink.emit_text(
            &Ok {
                ok: true,
                output: value,
            },
            |_| String::new(),
        ),
        PluginReply::Error(error) => {
            sink.emit_text(&Failed { ok: false, error }, |_| String::new())
        }
    }
}

/// The `group list` table: short values only (STD-01 R15). The opener,
/// related references, seat chains and summary are in `group show`.
fn group_list_columns() -> Vec<Column> {
    vec![
        left("ID"),
        cut("TITLE"),
        left("KIND"),
        left("PROJECT"),
        left("OPENED"),
        left("CLOSED"),
        right("SEATS"),
    ]
}

/// The full field list: `group show` labels and the tab-separated line.
const GROUP_COLUMNS: [&str; 10] = [
    "ID", "TITLE", "KIND", "PROJECT", "OPENED", "CLOSED", "OPENER", "RELATED", "SEATS", "SUMMARY",
];

fn group_list_row(g: &scribe_core::group::Group) -> Vec<Cell> {
    [
        g.id.clone(),
        one_line(&g.title),
        one_line(&g.kind),
        g.project.clone(),
        table_date_string(&g.opened),
        g.closed
            .as_deref()
            .map(table_date_string)
            .unwrap_or_else(|| "-".into()),
        g.seats.len().to_string(),
    ]
    .into_iter()
    .map(Cell::plain)
    .collect()
}

/// A group's full field list; `shorts` prints each seat's sessions as short
/// ids, for a terminal.
fn group_row(g: &scribe_core::group::Group, shorts: Option<&ShortIds>) -> Vec<Cell> {
    let seats = g
        .seats
        .iter()
        .map(|s| {
            let chain = s
                .transcripts
                .iter()
                .map(|id| shown_id(id.as_str(), shorts))
                .collect::<Vec<_>>()
                .join(" -> ");
            format!(
                "{} ({}): {} [handoff: {}]",
                s.name,
                one_line(&s.role),
                chain,
                s.newest_handoff.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    [
        g.id.clone(),
        one_line(&g.title),
        one_line(&g.kind),
        g.project.clone(),
        g.opened.clone(),
        g.closed.clone().unwrap_or_else(|| "-".into()),
        shown_id(&g.opener, shorts),
        or_dash(
            &g.related
                .iter()
                .map(|r| {
                    let cite = r.to_string();
                    shorts.map_or(cite.clone(), |ids| shown_cite(&cite, ids))
                })
                .collect::<Vec<_>>()
                .join(", "),
        )
        .to_owned(),
        or_dash(&seats).to_owned(),
        g.summary.as_deref().map_or_else(|| "-".into(), one_line),
    ]
    .into_iter()
    .map(Cell::plain)
    .collect()
}

fn entry_row(e: &EntryPayload, mode: Mode) -> Vec<Cell> {
    vec![
        Cell::plain(e.id.clone()),
        Cell {
            text: e.status.to_owned(),
            role: role_for_status(e.status),
        },
        Cell::plain(e.project.clone()),
        Cell::plain(joined(&e.topics)),
        Cell::plain(list_timestamp(e.created, mode)),
        Cell::plain(one_line(&e.content)),
        Cell::plain(author(&e.author)),
        Cell::plain(
            e.action
                .as_ref()
                .map_or_else(|| String::from("-"), |a| age(a.age_seconds)),
        ),
    ]
}

/// `user`, or `<kind>:<name>` for an agent's or scribe's entry.
fn author(a: &super::payload::AuthorPayload) -> String {
    if a.kind == "user" {
        a.name.clone()
    } else {
        format!("{}:{}", a.kind, a.name)
    }
}

/// An age in its largest whole unit: `45s`, `12m`, `3h`, `9d`.
fn age(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3_600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// A session pass's report: a table or tab-separated lines, one per
/// session passed, or `{"passes": [...], "remaining": n}`. Nothing passed
/// prints nothing but the JSON object.
pub(crate) fn pass<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    report: &PassReport,
) -> Result {
    let mode = sink.terminal().mode();
    sink.emit(report, |report| {
        let prefixes = shorts.prefixes_only();
        let pass_ids = if mode == Mode::Table {
            &prefixes
        } else {
            shorts
        };
        let rows = report
            .passes
            .iter()
            .map(|p| pass_row(p, table_shorts(mode, pass_ids)))
            .collect();
        table(mode, pass_columns(), rows)
    })
}

/// A consistency scan's report: a table or tab-separated lines, one per
/// approval re-applied, proposal opened, retired-tag candidate closed
/// (`closed`, the proposal id, `closed_reason` or kind, older, newer),
/// covered candidate, session deactivated or deactivation
/// failed, or the JSON object. A scan that changed nothing prints nothing
/// but the JSON object.
pub(crate) fn scan<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    report: &ScanReport,
) -> Result {
    let mode = sink.terminal().mode();
    sink.emit(report, |report| {
        table(mode, scan_columns(), scan_rows(shorts, report))
    })
}

fn scan_rows(shorts: &ShortIds, report: &ScanReport) -> Vec<Vec<Cell>> {
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    for r in &report.reapplied {
        rows.push(vec![
            Cell::plain("reapplied"),
            Cell::plain(r.proposal.clone()),
            Cell::plain(r.status),
            Cell::plain(r.entry.clone()),
            Cell::plain("-"),
        ]);
    }
    for p in &report.opened {
        rows.push(vec![
            Cell {
                text: String::from("opened"),
                role: Role::Warn,
            },
            Cell::plain(p.id.clone()),
            Cell::plain(p.kind),
            Cell::plain(p.older.clone()),
            Cell::plain(p.newer.clone().unwrap_or_else(|| String::from("-"))),
        ]);
    }
    for p in &report.closed {
        rows.push(vec![
            Cell::plain("closed"),
            Cell::plain(p.id.clone()),
            Cell::plain(p.closed_reason.unwrap_or(p.kind)),
            Cell::plain(p.older.clone()),
            Cell::plain(p.newer.clone().unwrap_or_else(|| String::from("-"))),
        ]);
    }
    for item in &report.covered {
        rows.push(vec![
            Cell::plain("covered"),
            Cell::plain(item.proposal.clone()),
            Cell::plain(item.by.clone()),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    for d in &report.deactivated {
        rows.push(vec![
            Cell {
                text: String::from("deactivated"),
                role: Role::Warn,
            },
            Cell::plain(shown_id(&d.session, Some(shorts))),
            Cell::plain(d.route),
            Cell::plain(d.cite.clone().unwrap_or_else(|| String::from("-"))),
            Cell::plain("-"),
        ]);
    }
    for f in &report.failed_deactivations {
        rows.push(vec![
            Cell {
                text: String::from("failed"),
                role: Role::Warn,
            },
            Cell::plain(shown_id(&f.session, Some(shorts))),
            Cell::plain(f.route),
            Cell::plain(f.error_code),
            Cell::plain("-"),
        ]);
    }
    let memory = &report.memory;
    for m in &memory.merged {
        rows.push(vec![
            Cell::plain("merged"),
            Cell::plain(m.item.clone()),
            Cell::plain("memory"),
            Cell::plain(m.into.clone()),
            Cell::plain(m.item.clone()),
        ]);
    }
    for m in &memory.superseded {
        rows.push(vec![
            Cell::plain("superseded"),
            Cell::plain(m.item.clone()),
            Cell::plain("memory"),
            Cell::plain(m.item.clone()),
            Cell::plain(m.by.clone()),
        ]);
    }
    for m in &memory.promoted {
        rows.push(vec![
            Cell::plain("promoted"),
            Cell::plain(m.item.clone()),
            Cell::plain(m.rule),
            Cell::plain(m.item.clone()),
            Cell::plain(m.to.clone()),
        ]);
    }
    for id in &memory.completed {
        rows.push(vec![
            Cell::plain("completed"),
            Cell::plain(id.clone()),
            Cell::plain("memory"),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    for f in &memory.failed_batches {
        rows.push(vec![
            Cell {
                text: String::from("failed"),
                role: Role::Warn,
            },
            Cell::plain(format!("memory-batch-{}", f.batch)),
            Cell::plain(f.error_code),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    rows
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

/// `approved by <aide> on your instruction "<quote>" (<session>, <date>)`
/// for an entry an agent approved on the user's checked instruction
/// (2_design.md §12.3); `None` when they typed the approval.
fn approved_line(e: &EntryPayload, shorts: Option<&ShortIds>) -> Option<String> {
    let by = e.approved_by.as_ref()?;
    let inst = e.instruction.as_ref()?;
    Some(scribe_core::voice::instructed_line(
        "approved",
        &by.name,
        scribe_core::voice::INSTRUCTION_YOURS,
        inst.quote.as_deref(),
        &shown_id(&inst.session, shorts),
        inst.at,
    ))
}

/// How a candidate settled on the user's checked instruction reads:
/// `approved by <aide> on your instruction "<quote>" (<session>, <date>)`,
/// or `rejected by …`; `None` for one they settled by typing, or still open.
fn settled_line(p: &ProposalPayload, shorts: Option<&ShortIds>) -> Option<String> {
    let inst = p.instruction.as_ref()?;
    let by = p.settled_author.as_ref()?;
    Some(scribe_core::voice::instructed_line(
        p.state,
        &by.name,
        scribe_core::voice::INSTRUCTION_YOURS,
        inst.quote.as_deref(),
        &shown_id(&inst.session, shorts),
        inst.at,
    ))
}

/// A proposal's rows: one, and for a candidate carrying scribe's
/// context (or an older claim's statement), in the table, a second under it holding the user's quote in
/// the `REASON` column (2_design.md §11.1).
fn proposal_rows(p: &ProposalPayload, mode: Mode, shorts: &ShortIds) -> Vec<Vec<Cell>> {
    let mut rows = vec![proposal_row(p, mode, shorts)];
    if mode == Mode::Table
        && let Some(quote) = stated_candidate_quote(p)
    {
        let mut under: Vec<Cell> = proposal_columns()
            .iter()
            .map(|_| Cell::plain(String::new()))
            .collect();
        if let Some(cell) = under.get_mut(PROPOSAL_REASON_COLUMN) {
            *cell = Cell {
                text: quote,
                role: Role::Muted,
            };
        }
        rows.push(under);
    }
    if mode == Mode::Table
        && let Some(line) = settled_line(p, None)
    {
        let mut under: Vec<Cell> = proposal_columns()
            .iter()
            .map(|_| Cell::plain(String::new()))
            .collect();
        if let Some(cell) = under.get_mut(PROPOSAL_REASON_COLUMN) {
            *cell = Cell {
                text: line,
                role: Role::Muted,
            };
        }
        rows.push(under);
    }
    rows
}

/// Where `REASON` sits in [`PROPOSAL_COLUMNS`].
const PROPOSAL_REASON_COLUMN: usize = 6;

/// `The user's words: "<quote>"` (or the assistant's) for a candidate
/// carrying scribe's context or an older claim's statement.
fn stated_candidate_quote(p: &ProposalPayload) -> Option<String> {
    let candidate = p.candidate.as_ref()?;
    candidate.stated()?;
    Some(format!(
        "{} \"{}\"",
        scribe_core::voice::words_label(&scribe_core::voice::UserName::default(), candidate.role),
        one_line(&candidate.quote)
    ))
}

/// A proposal's line. A candidate carrying scribe's context shows the
/// context (an older claim its labelled statement) as its reason; the plain
/// line adds the speaker's quote after it, where the table puts it on the
/// row under.
fn proposal_row(p: &ProposalPayload, mode: Mode, shorts: &ShortIds) -> Vec<Cell> {
    let reason = match (
        p.candidate.as_ref().and_then(super::payload::claim_context),
        stated_candidate_quote(p),
    ) {
        (Some(context), Some(quote)) if mode != Mode::Table => {
            format!("{} | {quote}", one_line(&context))
        }
        (Some(context), _) => one_line(&context),
        (None, _) => one_line(&p.reason),
    };
    // The plain line carries the instruction after the reason, where the
    // table puts it on a row under (2_design.md §12.3).
    let reason = match settled_line(p, (mode == Mode::Plain).then_some(shorts)) {
        Some(line) if mode != Mode::Table => format!("{reason} | {line}"),
        _ => reason,
    };
    vec![
        Cell::plain(p.id.clone()),
        Cell::plain(p.kind),
        Cell {
            text: p.state.to_owned(),
            role: if p.state == "open" {
                Role::Warn
            } else {
                Role::Muted
            },
        },
        Cell::plain(p.older.clone()),
        Cell::plain(p.newer.clone().unwrap_or_else(|| String::from("-"))),
        Cell::plain(list_timestamp(p.opened_at, mode)),
        Cell::plain(reason),
        Cell::plain(
            p.candidate
                .as_ref()
                .and_then(|c| c.admitted.as_ref())
                .map_or_else(|| String::from("-"), |a| one_line(&admitted(a))),
        ),
        Cell::plain(p.closed_reason.unwrap_or("-")),
    ]
}

/// What admitted a pass claim, on one line: the trigger as configured, the
/// turn's words it matched and the criteria in force.
fn admitted(a: &scribe_core::Admitted) -> String {
    format!(
        "trigger {:?} matched {:?}; criteria: {}",
        a.trigger,
        a.matched,
        a.criteria.as_deref().unwrap_or("-")
    )
}

/// A hit's row, the same in the table and the plain line, except that the
/// table cuts the title to [`TITLE_CELL_CHARS`] so the snippet keeps room.
/// A hidden record's cite is muted in the table.
fn hit_row(h: &HitPayload, mode: Mode, shorts: &ShortIds) -> Vec<Cell> {
    let title = one_line(&h.title);
    let hidden = h.active == Some(false) || !matches!(h.status, "active" | "open" | "final");
    vec![
        Cell {
            text: if mode == Mode::Plain {
                shown_cite(&h.cite, shorts)
            } else {
                h.cite.clone()
            },
            role: if hidden { Role::Muted } else { Role::Neutral },
        },
        Cell::plain(h.kind),
        Cell::plain(format!("{:.3}", h.score.as_f64().unwrap_or_default())),
        Cell::plain(list_timestamp(h.updated, mode)),
        Cell::plain(if mode == Mode::Table {
            cap_title(&title)
        } else {
            title
        }),
        Cell::plain(one_line(&h.snippet)),
    ]
}

/// The widest title a hit's table row shows, in display columns.
const TITLE_CELL_COLUMNS: usize = 40;

/// A hit's title as its table row shows it: cut to [`TITLE_CELL_COLUMNS`]
/// display columns, ending in one `…` when anything was cut. The cap is
/// scribe's own so the snippet keeps room; fitting a terminal is the kit's.
fn cap_title(title: &str) -> String {
    if title.width() <= TITLE_CELL_COLUMNS {
        return title.to_owned();
    }
    let keep = TITLE_CELL_COLUMNS - 1;
    let mut used = 0;
    let mut cut = String::new();
    for c in title.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > keep {
            break;
        }
        used += w;
        cut.push(c);
    }
    cut.push('…');
    cut
}

fn transcript_row(t: &TranscriptPayload, mode: Mode, shorts: Option<&ShortIds>) -> Vec<Cell> {
    vec![
        Cell::plain(shown_id(&t.session, shorts)),
        Cell::plain(t.project.clone()),
        Cell::plain(t.user_turns.to_string()),
        Cell::plain(t.bytes.to_string()),
        Cell::plain(optional_list_timestamp(t.started, mode)),
        Cell::plain(optional_list_timestamp(t.ended, mode)),
        Cell::plain(one_line(&t.first_prompt)),
    ]
}

fn mark_row(
    m: &scribe_core::annotation::MarkRecord,
    mode: Mode,
    shorts: Option<&ShortIds>,
) -> Vec<Cell> {
    vec![
        Cell::plain(shown_id(m.session.as_str(), shorts)),
        Cell::plain(m.message.clone()),
        Cell::plain(m.marker.to_string()),
        Cell::plain(m.outcome.as_str().to_owned()),
        Cell::plain(m.code.clone().unwrap_or_else(|| String::from("-"))),
        Cell::plain(m.attempts.to_string()),
        Cell::plain(list_timestamp(m.at, mode)),
    ]
}

fn tag_row(t: &TagPayload) -> Vec<Cell> {
    vec![
        Cell {
            text: t.name.clone(),
            role: if t.configured {
                Role::Neutral
            } else {
                Role::Muted
            },
        },
        Cell::plain(t.entries.to_string()),
        Cell::plain(
            t.stale_after_days
                .map_or_else(|| String::from("-"), |d| d.to_string()),
        ),
        Cell::plain(joined(&t.aliases)),
        Cell::plain(one_line(t.description.as_deref().unwrap_or("-"))),
        Cell::plain(t.source.unwrap_or("-").to_owned()),
        Cell::plain(t.action.as_ref().map_or_else(
            || String::from("-"),
            |a| {
                if a.require_quote {
                    format!("{},quote", a.mode)
                } else {
                    a.mode.to_owned()
                }
            },
        )),
        Cell::plain(
            t.action
                .as_ref()
                .map_or_else(|| String::from("-"), |a| a.retired.to_string()),
        ),
        Cell::plain(
            t.action
                .as_ref()
                .map_or_else(|| String::from("-"), |a| a.rejected.to_string()),
        ),
        Cell::plain(t.route.unwrap_or("-").to_owned()),
    ]
}

/// One tab-separated line, no header, no color (STD-01 R9). Free text is
/// flattened to one line by [`one_line`]; `--json` and `show` carry it
/// verbatim.
fn plain_line(cells: &[Cell]) -> String {
    let texts: Vec<&str> = cells.iter().map(|c| c.text.as_str()).collect();
    texts.join("\t")
}

/// Free text on one line: every run of whitespace, tabs and line breaks
/// included, becomes one space, so a record never spans fields or lines.
/// Empty text is `-` (STD-01 R14).
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        String::from("-")
    } else {
        flat
    }
}

fn or_dash(text: &str) -> &str {
    if text.is_empty() { "-" } else { text }
}

fn joined(items: &[String]) -> String {
    if items.is_empty() {
        String::from("-")
    } else {
        items.join(",")
    }
}

fn yes_no(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}

fn due_role(due: bool) -> Role {
    if due { Role::Warn } else { Role::Neutral }
}

fn timestamp(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_else(|_| String::from("-"))
}

/// A human table's timestamp in table mode (on a terminal, or with
/// `--format table` on a pipe): its UTC date, `YYYY-MM-DD`. Sorting, plain
/// output and JSON use the full instant ([`timestamp`]).
/// A record's offset can put its UTC date outside the supported range, which
/// has no `YYYY-MM-DD`; the table shows `-` there rather than panicking.
pub(super) fn table_date(at: OffsetDateTime) -> String {
    at.checked_to_offset(UtcOffset::UTC)
        .and_then(|utc| utc.date().format(&Iso8601::DATE).ok())
        .unwrap_or_else(|| String::from("-"))
}

fn list_timestamp(at: OffsetDateTime, mode: Mode) -> String {
    if mode == Mode::Table {
        table_date(at)
    } else {
        timestamp(at)
    }
}

fn optional_list_timestamp(at: Option<OffsetDateTime>, mode: Mode) -> String {
    at.map(|at| list_timestamp(at, mode))
        .unwrap_or_else(|| String::from("-"))
}

fn table_date_string(value: &str) -> String {
    OffsetDateTime::parse(value, &Rfc3339)
        .map(table_date)
        .unwrap_or_else(|_| String::from("-"))
}

/// Render scaffold artifacts and actionable next steps without claiming capture.
pub(super) fn init<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    report: &scribe_core::apps::init::InitReport,
) -> Result {
    sink.emit_text(report, |report| {
        let mut text = String::new();
        for file in &report.files {
            text.push_str(&format!("{}\t{}\n", file.status, file.path.display()));
        }
        for step in &report.next {
            text.push_str(step);
            text.push('\n');
        }
        text
    })
}

/// Render the result of a group backfill.
pub(crate) fn group_backfill<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    report: &super::GroupBackfill,
) -> Result {
    sink.emit_text(report, |report| {
        let mut text = String::new();
        for member in &report.members {
            text.push_str(&format!(
                "{} {}: {} stored, {} skipped{}\n",
                member.seat,
                shown_id(&member.session, Some(shorts)),
                member.stored,
                member.skipped,
                if member.transcript_unavailable {
                    " (transcript unavailable)"
                } else {
                    ""
                }
            ));
        }
        text.push_str(&format!(
            "Total: {} stored, {} skipped\n",
            report.stored, report.skipped
        ));
        text
    })
}
