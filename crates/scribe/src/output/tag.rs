//! Payloads and rendering for `scribe tag`.

use super::common::console::Sink;
use super::common::detail::{Detail, emit_detail, field};
use super::common::record::{
    AuthorPayload, OriginPayload, WarningPayload, envelope, origin_text, strings,
};
use super::common::table::{Cell, Result, cut, joined, left, one_line, right, table, timestamp};
use cw_cli::{Column, Mode, Role};
use scribe_core::TagSummary;
use serde::Serialize;
use std::fmt::{self, Write as _};
use std::io::Write;

/// An action tag's switch and counts, where the user decides to flip it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TagActionPayload {
    /// `free` (agents write unasked) or `ask-first`.
    pub(crate) mode: &'static str,
    /// Whether an agent's entry must quote the user's correction.
    pub(crate) require_quote: bool,
    /// Agent-written entries under it that were retired or forgotten.
    pub(crate) retired: usize,
    /// Agent-written entries under it whose result was rejected.
    pub(crate) rejected: usize,
}

/// One tag as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TagPayload {
    pub(crate) author: Option<AuthorPayload>,
    pub(crate) source: Option<&'static str>,
    pub(crate) name: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) description: Option<String>,
    /// `null` when entries never go stale.
    pub(crate) stale_after_days: Option<u32>,
    /// Whether `config.toml` (or the defaults) declares it; `false` for a
    /// directory of entries no configured tag names.
    pub(crate) configured: bool,
    /// Entries in it, any status.
    pub(crate) entries: usize,
    /// For an action tag, how agents use it and how often the user undid
    /// their entries; `null` for a memory tag.
    pub(crate) action: Option<TagActionPayload>,
    /// The capture route (`propose` or `write`); `null` for a tag
    /// with no capture rules, which the pass never captures.
    pub(crate) route: Option<&'static str>,
}

impl From<&TagSummary> for TagPayload {
    fn from(tag: &TagSummary) -> Self {
        Self {
            author: tag.author.as_ref().map(AuthorPayload::from),
            source: tag.source,
            name: tag.name.to_string(),
            aliases: tag
                .def
                .as_ref()
                .map(|d| strings(&d.aliases))
                .unwrap_or_default(),
            description: tag
                .def
                .as_ref()
                .and_then(|d| d.description.as_deref().map(shown)),
            stale_after_days: tag
                .def
                .as_ref()
                .and_then(|d| d.stale_after_days)
                .map(|d| d.get()),
            configured: tag.source == Some("config"),
            entries: tag.entries,
            action: tag
                .def
                .as_ref()
                .and_then(|d| d.action)
                .map(|a| TagActionPayload {
                    mode: a.mode(),
                    require_quote: a.require_quote,
                    retired: tag.retired,
                    rejected: tag.rejected,
                }),
            route: tag
                .def
                .as_ref()
                .and_then(|d| d.capture.route())
                .map(scribe_core::CaptureRoute::as_str),
        }
    }
}

/// An action tag's settings, as `tag show` prints them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TagShowActionPayload {
    /// `free` or `ask-first`.
    pub(crate) mode: &'static str,
    pub(crate) require_quote: bool,
}

/// Where each capture field came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CaptureOriginPayload {
    pub(crate) from: OriginPayload,
    pub(crate) route: OriginPayload,
    pub(crate) triggers: OriginPayload,
    pub(crate) criteria: OriginPayload,
    pub(crate) not: OriginPayload,
    pub(crate) max_open: OriginPayload,
}

/// A tag's effective capture rules; a field with origin `none` is `null`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CapturePayload {
    pub(crate) from: Option<&'static str>,
    pub(crate) route: Option<&'static str>,
    pub(crate) triggers: Option<Vec<String>>,
    pub(crate) criteria: Option<String>,
    pub(crate) not: Option<String>,
    /// The most open claims the tag may hold: the config's value, else
    /// the default 10 for a tag with a route; `null` for a tag that
    /// catches nothing.
    pub(crate) max_open: Option<u32>,
    pub(crate) origin: CaptureOriginPayload,
}

/// `tag show`: a tag's settings and its effective capture rules, each
/// field with its origin (docs/design/scribe/2_design.md §11.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TagShowPayload {
    pub(crate) tag: String,
    /// `config` (config.toml or the shipped defaults) or `store`.
    pub(crate) source: &'static str,
    pub(crate) aliases: Vec<String>,
    pub(crate) description: Option<String>,
    pub(crate) stale_after_days: Option<u32>,
    /// An action tag's settings; `null` for a memory tag.
    pub(crate) action: Option<TagShowActionPayload>,
    pub(crate) capture: CapturePayload,
}

impl TagShowPayload {
    /// The payload for `def`, read from `source`, whose config file is
    /// `config_file`.
    pub(crate) fn new(def: &scribe_core::TagDef, source: &'static str, config_file: &str) -> Self {
        let c = &def.capture;
        Self {
            tag: def.name.to_string(),
            source,
            aliases: strings(&def.aliases),
            description: def.description.as_deref().map(shown),
            stale_after_days: def.stale_after_days.map(std::num::NonZeroU32::get),
            action: def.action.map(|a| TagShowActionPayload {
                mode: a.mode(),
                require_quote: a.require_quote,
            }),
            capture: CapturePayload {
                from: c.from.as_ref().map(|r| r.value.as_str()),
                route: c.route.as_ref().map(|r| r.value.as_str()),
                triggers: c.triggers.as_ref().map(|r| r.value.clone()),
                criteria: c.criteria.as_ref().map(|r| shown(&r.value)),
                not: c.not.as_ref().map(|r| shown(&r.value)),
                max_open: (c.max_open.is_some() || c.route.is_some()).then(|| c.max_open()),
                origin: CaptureOriginPayload {
                    from: OriginPayload::of(c.from.as_ref(), config_file),
                    route: OriginPayload::of(c.route.as_ref(), config_file),
                    triggers: OriginPayload::of(c.triggers.as_ref(), config_file),
                    criteria: OriginPayload::of(c.criteria.as_ref(), config_file),
                    not: OriginPayload::of(c.not.as_ref(), config_file),
                    max_open: match (&c.max_open, &c.route) {
                        (Some(rule), _) => OriginPayload::of(Some(rule), config_file),
                        (None, Some(_)) => OriginPayload::SHIPPED,
                        (None, None) => OriginPayload::of::<()>(None, config_file),
                    },
                },
            },
        }
    }
}

/// Render a `tag list` result.
pub(crate) fn list<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    items: &[TagPayload],
    total: usize,
    warnings: &[WarningPayload],
) -> Result {
    let json = envelope("tags", items, total, warnings);
    let mode = sink.terminal().mode();
    sink.emit(&json, |_| {
        let rows = items.iter().map(tag_row).collect();
        table(mode, tag_columns(), rows)
    })
}

/// Render a single store tag (`tag add`, `tag update`, `tag remove`).
pub(crate) fn show<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    tag: &scribe_core::StoreTag,
) -> Result {
    emit_detail(sink, tag, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => writeln!(
            out,
            "{}\t{}\t{}\t{}",
            tag.name,
            one_line(&tag.description),
            tag.author,
            tag.aliases
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Mode::Table => {
            field(out, "name", tag.name.as_str())?;
            field(out, "description", &tag.description)?;
            field(
                out,
                "aliases",
                &tag.aliases
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            )?;
            field(out, "author", &tag.author.to_string())?;
            field(
                out,
                "updated_by",
                &tag.updated_by
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), ToString::to_string),
            )?;
            field(
                out,
                "stale_after_days",
                &tag.stale_after_days
                    .map_or_else(|| "-".to_owned(), |d| d.to_string()),
            )?;
            field(out, "created", &timestamp(tag.created))?;
            field(out, "updated", &timestamp(tag.updated))
        }
    })
}

/// Render `tag show` settings and effective capture rules.
pub(crate) fn show_rules<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    tag: &TagShowPayload,
) -> Result {
    emit_detail(sink, tag, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => tag_rules(out, tag, false),
        Mode::Table => tag_rules(out, tag, true),
    })
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

/// `tag show`: the tag's settings, then each capture field with its value
/// and origin. Plain is `<field>\t<value>\t<origin>` lines (origin `-` for
/// a setting); the table aligns the same lines, labels first.
fn tag_rules(out: &mut Detail, t: &TagShowPayload, table: bool) -> fmt::Result {
    let origin = origin_text;
    let c = &t.capture;
    let action = t.action.as_ref().map_or_else(
        || String::from("-"),
        |a| {
            if a.require_quote {
                format!("{},quote", a.mode)
            } else {
                a.mode.to_owned()
            }
        },
    );
    let rows: Vec<(&str, String, String)> = vec![
        ("tag", t.tag.clone(), String::from("-")),
        ("source", t.source.to_owned(), String::from("-")),
        ("aliases", joined(&t.aliases), String::from("-")),
        (
            "description",
            one_line(t.description.as_deref().unwrap_or("-")),
            String::from("-"),
        ),
        (
            "stale_after_days",
            t.stale_after_days
                .map_or_else(|| String::from("-"), |d| d.to_string()),
            String::from("-"),
        ),
        ("action", action, String::from("-")),
        (
            "from",
            c.from.unwrap_or("-").to_owned(),
            origin(&c.origin.from),
        ),
        (
            "route",
            c.route.unwrap_or("-").to_owned(),
            origin(&c.origin.route),
        ),
        (
            "triggers",
            c.triggers.as_ref().map_or_else(
                || String::from("-"),
                |t| {
                    if t.is_empty() {
                        String::from("(none: the pass captures nothing)")
                    } else {
                        t.iter()
                            .map(|x| format!("{x:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                },
            ),
            origin(&c.origin.triggers),
        ),
        (
            "criteria",
            one_line(c.criteria.as_deref().unwrap_or("-")),
            origin(&c.origin.criteria),
        ),
        (
            "not",
            one_line(c.not.as_deref().unwrap_or("-")),
            origin(&c.origin.not),
        ),
        (
            "max_open",
            c.max_open
                .map_or_else(|| String::from("-"), |n| n.to_string()),
            origin(&c.origin.max_open),
        ),
    ];
    for (label, value, from) in rows {
        if table {
            let label = if label == "stale_after_days" {
                "stale days"
            } else {
                label
            };
            if from == "-" {
                field(out, label, &value)?;
            } else {
                field(out, label, &format!("{value}  ({from})"))?;
            }
        } else {
            writeln!(out, "{label}\t{value}\t{from}")?;
        }
    }
    Ok(())
}

/// Tag text as output shows it: `{user}` reads "the user", never the
/// configured name, which reaches prompts only (2_design.md §15.8).
fn shown(text: &str) -> String {
    scribe_core::voice::UserName::default().fill(text)
}
