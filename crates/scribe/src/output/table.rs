//! Borderless tables (STD-01 R14, R15).
//!
//! One header row, exactly one line per record, two-space gutters, numbers
//! right-aligned. When a width is given (a terminal), the truncatable column
//! shrinks and an overlong cell ends in a single `…`; with no width (a pipe,
//! a file) nothing is ever cut. Width is a parameter, never read here, so
//! tests pin it.
//!
//! Widths are display columns, not characters: a wide CJK character or
//! emoji takes two, a combining mark none, so columns line up whatever the
//! text. A control character in a cell is shown as U+FFFD before it is
//! measured or painted, so stored text never moves the cursor or recolors
//! a row.

use super::color::{Role, paint};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The narrowest a truncatable column shrinks to.
const MIN_TRUNCATED_WIDTH: usize = 8;

/// Column alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Align {
    /// Text.
    Left,
    /// Numbers.
    Right,
}

/// A column's header and layout rules.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Column {
    /// Header text; carries the unit for numeric columns.
    pub(crate) header: &'static str,
    /// Alignment of every cell in the column.
    pub(crate) align: Align,
    /// Whether this column may be cut to fit a terminal. Only a column whose
    /// full value is available from `entry show`, `session show` or `--json` may be (R15).
    pub(crate) truncate: bool,
}

/// One cell: its text and what its color means. Empty text in a record's row
/// is an absent value and renders as `-` (R14); the one exception is a
/// continuation row, whose first cell is empty (see [`render`]).
#[derive(Clone, Debug)]
pub(crate) struct Cell {
    /// The text; an absent value is `-`, never empty (R14).
    pub(crate) text: String,
    /// Semantic color role.
    pub(crate) role: Role,
}

impl Cell {
    /// A cell with no emphasis.
    pub(crate) fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: Role::Neutral,
        }
    }
}

/// Render `rows` under `columns`, each line ending in `\n`.
///
/// An empty cell is an absent value and prints as `-` (R14), in every table.
/// A row whose first cell is empty is a continuation of the row above it (a
/// proposal's quote line): its blanks are layout, not values, and stay blank.
pub(crate) fn render(
    columns: &[Column],
    rows: &[Vec<Cell>],
    width: Option<usize>,
    color: bool,
) -> String {
    let rows: Vec<Vec<Cell>> = rows
        .iter()
        .map(|row| {
            let continuation = row.first().is_some_and(|cell| cell.text.is_empty());
            row.iter()
                .map(|cell| Cell {
                    text: if cell.text.is_empty() && !continuation {
                        "-".to_owned()
                    } else {
                        printable(&cell.text)
                    },
                    role: cell.role,
                })
                .collect()
        })
        .collect();
    let widths = fit(columns, &natural_widths(columns, &rows), width);
    let mut out = String::new();
    let header: Vec<Cell> = columns.iter().map(|c| Cell::plain(c.header)).collect();
    push_line(&mut out, columns, &widths, &header, false);
    for row in &rows {
        push_line(&mut out, columns, &widths, row, color);
    }
    out
}

/// `text` with each control character shown as U+FFFD.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

/// The display columns `s` takes on a terminal.
fn char_len(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn natural_widths(columns: &[Column], rows: &[Vec<Cell>]) -> Vec<usize> {
    columns
        .iter()
        .enumerate()
        .map(|(i, col)| {
            rows.iter()
                .filter_map(|r| r.get(i))
                .map(|cell| char_len(&cell.text))
                .chain([char_len(col.header)])
                .max()
                .unwrap_or(0)
        })
        .collect()
}

/// Shrink truncatable columns until the line fits `width`, if it can.
fn fit(columns: &[Column], natural: &[usize], width: Option<usize>) -> Vec<usize> {
    let mut widths = natural.to_vec();
    let Some(limit) = width else {
        return widths;
    };
    let gutters = 2 * columns.len().saturating_sub(1);
    let mut excess = (widths.iter().sum::<usize>() + gutters).saturating_sub(limit);
    for (i, col) in columns.iter().enumerate() {
        if excess == 0 {
            break;
        }
        if col.truncate {
            let floor = MIN_TRUNCATED_WIDTH.max(char_len(col.header));
            let give = excess.min(widths[i].saturating_sub(floor));
            widths[i] -= give;
            excess -= give;
        }
    }
    widths
}

fn push_line(out: &mut String, columns: &[Column], widths: &[usize], cells: &[Cell], color: bool) {
    let last = columns.len().saturating_sub(1);
    for (i, (col, &w)) in columns.iter().zip(widths).enumerate() {
        let (text, role) = cells
            .get(i)
            .map_or(("-", Role::Neutral), |c| (c.text.as_str(), c.role));
        let text = truncate(text, w);
        let pad = " ".repeat(w.saturating_sub(char_len(&text)));
        let painted = paint(&text, role, color);
        match col.align {
            Align::Right => {
                out.push_str(&pad);
                out.push_str(&painted);
            }
            Align::Left if i == last => out.push_str(&painted),
            Align::Left => {
                out.push_str(&painted);
                out.push_str(&pad);
            }
        }
        if i != last {
            out.push_str("  ");
        }
    }
    // A row whose trailing cells are blank (a proposal's quote row) ends at
    // its last text, not in padding.
    let end = out.trim_end_matches(' ').len();
    out.truncate(end);
    out.push('\n');
}

/// Cut `text` to `width` display columns, ending in one `…` when anything
/// was cut. A wide character that would straddle the edge is dropped whole,
/// so the cut text may be a column short; padding makes up the rest.
pub(crate) fn truncate(text: &str, width: usize) -> String {
    if char_len(text) <= width {
        return text.to_owned();
    }
    let keep = width.saturating_sub(1);
    let mut used = 0;
    let mut cut = String::new();
    for c in text.chars() {
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
