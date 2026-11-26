//! Shared detail-view builder and field formatting helpers for single-record
//! output (STD-01 R15).

use cw_cli::{Mode, Role, Sink, TextSpan};
use serde::Serialize;
use std::fmt::{self, Write as _};
use std::io::Write;

/// The width of the detail view's label column.
const LABEL_WIDTH: usize = 12;

/// A detail view under construction: text in order, a span at a time, so a
/// value can carry the role that colors it. The kit escapes each span.
#[derive(Default)]
pub(crate) struct Detail {
    spans: Vec<TextSpan>,
}

impl Detail {
    fn into_spans(self) -> Vec<TextSpan> {
        self.spans
    }
}

impl fmt::Write for Detail {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.spans.push(TextSpan::new(s));
        Ok(())
    }
}

/// Emit one record as JSON in JSON mode or via a built [`Detail`] view in
/// human modes. The detail view never truncates: it is where a truncated table
/// cell is read in full (STD-01 R15).
pub(crate) fn emit_detail<O, E, T, F>(
    sink: &mut Sink<O, E>,
    payload: &T,
    build: F,
) -> cw_cli::Result<()>
where
    O: Write,
    E: Write,
    T: Serialize,
    F: FnOnce(&mut Detail, Mode) -> fmt::Result,
{
    let mode = sink.terminal().mode();
    let mut detail = Detail::default();
    // Json mode writes the payload and never reads the view.
    if mode != Mode::Json {
        // Writing to a `Detail` cannot fail.
        let _ = build(&mut detail, mode);
    }
    sink.emit_text_spans(payload, |_| detail.into_spans())
}

/// `label  value`, with continuation lines indented under the value. A label
/// as long as the column or longer is followed by one space, never run into
/// its value.
pub(crate) fn field(out: &mut Detail, label: &str, value: &str) -> fmt::Result {
    field_role(out, label, value, Role::Neutral)
}

/// [`field`] with the value in `role`'s color where color is on.
pub(crate) fn field_role(out: &mut Detail, label: &str, value: &str, role: Role) -> fmt::Result {
    let width = LABEL_WIDTH.max(label.chars().count() + 1);
    let indent = format!("\n{}", " ".repeat(width));
    write!(out, "{label:<width$}")?;
    out.spans
        .push(TextSpan::new(value.replace('\n', &indent)).role(role));
    writeln!(out)
}

pub(crate) fn or_none(items: &[String]) -> String {
    if items.is_empty() {
        String::from("-")
    } else {
        items.join(", ")
    }
}

pub(crate) fn joined_lines(items: &[String]) -> String {
    if items.is_empty() {
        String::from("-")
    } else {
        items.join("\n")
    }
}
