use crate::output::color::Role;
use crate::output::table::{Align, Cell, Column, render, truncate};

const COLUMNS: [Column; 3] = [
    Column {
        header: "ID",
        align: Align::Right,
        truncate: false,
    },
    Column {
        header: "TITLE",
        align: Align::Left,
        truncate: true,
    },
    Column {
        header: "TAGS",
        align: Align::Left,
        truncate: false,
    },
];

fn row(id: &str, title: &str, tags: &str) -> Vec<Cell> {
    vec![Cell::plain(id), Cell::plain(title), Cell::plain(tags)]
}

#[test]
fn one_header_and_one_line_per_record_with_two_space_gutters() {
    let rows = [row("1", "short", "-"), row("10", "a longer title", "home")];
    let text = render(&COLUMNS, &rows, None, false);
    assert_eq!(
        text,
        "ID  TITLE           TAGS\n \
         1  short           -\n\
         10  a longer title  home\n"
    );
    assert_eq!(text.lines().count(), rows.len() + 1);
}

#[test]
fn a_missing_cell_renders_as_a_dash() {
    let text = render(&COLUMNS, &[vec![Cell::plain("1")]], None, false);
    assert_eq!(text.lines().nth(1), Some(" 1  -      -"));
}

#[test]
fn overflow_is_truncated_with_a_single_ellipsis_at_a_pinned_width() {
    // STD-01 R14, R15: the width is a parameter, never read from the environment here.
    let rows = [row("1", "a title far too long for the terminal", "home")];
    let text = render(&COLUMNS, &rows, Some(24), false);
    for line in text.lines() {
        assert!(line.chars().count() <= 24, "line too wide: {line:?}");
    }
    let body = text.lines().nth(1).unwrap();
    assert_eq!(body.matches('…').count(), 1, "got {body:?}");
}

#[test]
fn without_a_width_nothing_is_cut() {
    let long = "x".repeat(300);
    let text = render(&COLUMNS, &[row("1", &long, "-")], None, false);
    assert!(text.contains(&long));
    assert!(!text.contains('…'));
}

#[test]
fn color_is_applied_to_cells_only_when_allowed() {
    let rows = [vec![
        Cell::plain("1"),
        Cell {
            text: "urgent".into(),
            role: Role::Warn,
        },
        Cell::plain("-"),
    ]];
    assert!(!render(&COLUMNS, &rows, None, false).contains('\x1b'));
    let colored = render(&COLUMNS, &rows, None, true);
    assert!(colored.contains("\x1b[33murgent\x1b[0m"));
    assert!(
        !colored.lines().next().unwrap().contains('\x1b'),
        "headers are never colored"
    );
}

#[test]
fn truncate_keeps_short_text_and_marks_cut_text() {
    assert_eq!(truncate("abc", 3), "abc");
    assert_eq!(truncate("abcdef", 4), "abc…");
}

#[test]
fn wide_characters_take_two_columns_so_the_next_column_lines_up() {
    let rows = [row("1", "日本語", "a"), row("2", "abcdef", "b")];
    let text = render(&COLUMNS, &rows, None, false);
    let tags_at: Vec<usize> = text
        .lines()
        .map(|line| {
            let cut = line.rfind(' ').unwrap();
            unicode_width::UnicodeWidthStr::width(&line[..cut])
        })
        .collect();
    assert_eq!(tags_at, [tags_at[0]; 3], "{text}");
}

#[test]
fn truncation_counts_display_columns() {
    assert_eq!(truncate("日本語です", 5), "日本…");
    assert_eq!(truncate("日本語です", 6), "日本…");
    assert_eq!(truncate("日本語です", 10), "日本語です");
}

#[test]
fn control_characters_in_a_cell_print_as_replacement_characters() {
    let rows = [row("1", "red \x1b[31mtext\r", "a")];
    let text = render(&COLUMNS, &rows, None, true);
    assert!(text.contains("red \u{FFFD}[31mtext\u{FFFD}"), "{text:?}");
    assert!(!text.contains("\x1b[31m"), "{text:?}");
}
