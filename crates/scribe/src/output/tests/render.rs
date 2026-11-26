use crate::output::common::record::{AuthorPayload, CitePayload, WarningPayload};
use crate::output::common::table::table_date;
use crate::output::entry::{ContentFromPayload, EntryPayload};
use crate::output::session::{HandoffPayload, SessionHandoffsPayload};
use crate::output::tag::TagPayload;
use crate::output::{Record, Records, list, show};
use cw_app::Env;
use cw_cli::{Format, Mode, Probe, Sink, Terminal};
use scribe_core::ShortIds;
use time::macros::datetime;

fn entry(id: &str, content: &str) -> EntryPayload {
    let at = datetime!(2026-01-02 03:04:05 UTC);
    EntryPayload {
        id: id.to_owned(),
        tag: String::from("preferences"),
        status: "active",
        project: String::from("scribe"),
        topics: vec![String::from("ci"), String::from("release")],
        related: Vec::new(),
        provider: String::from("claude-code"),
        host: String::from("host"),
        created: at,
        updated: at,
        said: String::from("#scribe preferences: x"),
        content: content.to_owned(),
        content_from: ContentFromPayload {
            kind: "instruction",
            role: None,
            message: None,
            at: None,
        },
        cite: Some(CitePayload {
            session: String::from("s-1"),
            message: None,
            at,
            prompt_sha256: "0".repeat(64),
            line: 1,
            cwd: String::from("/work"),
        }),
        superseded_by: None,
        review_after: None,
        author: AuthorPayload {
            kind: "user",
            name: String::from("user"),
        },
        updated_by: None,
        retirement_reason: None,
        quote: None,
        evidence: None,
        as_of: None,
        window_turns: None,
        window: None,
        action: None,
        admitted: None,
        approved_by: None,
        instruction: None,
        stated_by: None,
        outcome: None,
        handoff_mode: None,
        decision: None,
        item: None,
        state: None,
        removal_instruction: None,
    }
}

/// A sink on in-memory buffers, deciding as a pipe (or a terminal `width`
/// columns wide, in a table) would.
fn sink_at(mode: Mode, width: Option<usize>) -> Sink<Vec<u8>, Vec<u8>> {
    let format = match mode {
        Mode::Table => Format::Table,
        Mode::Plain => Format::Auto,
        Mode::Json => Format::Json,
    };
    let probe = width.map_or_else(Probe::piped, |w| Probe::terminal(Some(w)));
    let terminal = Terminal::decide(
        &Env::from_vars([("NO_COLOR", "1")]),
        "scribe",
        probe,
        Some(format),
    );
    Sink::new(terminal, Vec::new(), Vec::new())
}

fn sink(mode: Mode) -> Sink<Vec<u8>, Vec<u8>> {
    sink_at(mode, None)
}

/// What a sink wrote to stdout.
fn written(mut sink: Sink<Vec<u8>, Vec<u8>>) -> String {
    sink.finish().unwrap();
    String::from_utf8(sink.into_parts().0).unwrap()
}

fn render_list(mode: Mode, records: &Records, total: usize, warnings: &[WarningPayload]) -> String {
    let mut sink = sink(mode);
    list(
        &mut sink,
        &ShortIds::default(),
        records,
        total,
        warnings,
        None,
    )
    .unwrap();
    written(sink)
}

fn render_show_at(mode: Mode, width: Option<usize>, record: &Record) -> String {
    let mut sink = sink_at(mode, width);
    show(&mut sink, &ShortIds::default(), record).unwrap();
    written(sink)
}

fn render_show(mode: Mode, record: &Record) -> String {
    render_show_at(mode, None, record)
}

fn entries(items: Vec<EntryPayload>) -> Records {
    Records::Entries(items)
}

#[test]
fn the_plain_form_is_one_tab_separated_line_per_record_with_text_flattened() {
    // STD-01 R9: a line break in the content must not split the record.
    let records = entries(vec![
        entry("preferences-1", "one\ntwo\tthree"),
        entry("preferences-2", ""),
    ]);
    let text = render_list(Mode::Plain, &records, 2, &[]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "preferences-1\tactive\tscribe\tci,release\t2026-01-02T03:04:05Z\tone two three\tuser\t-",
            "preferences-2\tactive\tscribe\tci,release\t2026-01-02T03:04:05Z\t-\tuser\t-",
        ]
    );
}

#[test]
fn the_shared_table_timestamp_formatter_uses_the_stored_utc_date() {
    // The full instant stays in piped, JSON and show output (render.rs).
    assert_eq!(table_date(datetime!(2026-10-20 11:02:00 UTC)), "2026-10-20");
    // 23:30 on the 22nd at UTC-5 is 04:30 on the 23rd in UTC.
    assert_eq!(table_date(datetime!(2026-11-22 23:30:00 -5)), "2026-11-23");
}

#[test]
fn a_table_timestamp_whose_utc_date_leaves_the_supported_range_is_a_dash() {
    // ORB-27257: a record keeps the offset it was written with, so its local
    // date can sit at the supported bound while its UTC date does not. The
    // last UTC date in range renders; the first one past the bound is `-`.
    // 9999-12-30 23:00 at UTC-1 is 9999-12-31 00:00 UTC.
    assert_eq!(table_date(datetime!(9999-12-30 23:00:00 -1)), "9999-12-31");
    // 9999-12-31 23:00 at UTC-1 is 10000-01-01 00:00 UTC: out of range.
    assert_eq!(table_date(datetime!(9999-12-31 23:00:00 -1)), "-");
    // The lower bound: -9999-01-01 00:30 at UTC+1 is in year -10000 UTC.
    assert_eq!(table_date(datetime!(-9999-01-01 00:30:00 +1)), "-");
}

#[test]
fn an_empty_list_is_empty_stdout_in_human_modes_and_an_empty_envelope_in_json() {
    // STD-01 R16 (and R7: JSON mode always emits one document).
    for (records, key) in [
        (entries(Vec::new()), "entries"),
        (
            Records::Sessions {
                items: Vec::new(),
                active_column: false,
            },
            "sessions",
        ),
        (Records::Proposals(Vec::new()), "proposals"),
        (Records::Tags(Vec::new()), "tags"),
        (Records::Transcripts(Vec::new()), "transcripts"),
    ] {
        assert_eq!(render_list(Mode::Plain, &records, 0, &[]), "");
        assert_eq!(render_list(Mode::Table, &records, 0, &[]), "");
        let json: serde_json::Value =
            serde_json::from_str(&render_list(Mode::Json, &records, 0, &[])).unwrap();
        assert_eq!(
            json,
            serde_json::json!({key: [], "total": 0, "truncated": false, "warnings": []})
        );
    }
}

#[test]
fn a_cut_list_and_its_skipped_files_show_in_the_json_envelope() {
    // STD-01 R34.
    let warning = WarningPayload {
        path: String::from("/s/entries/preferences/preferences-9.md"),
        code: "store_corrupt",
        error: String::from("bad"),
    };
    let records = entries(vec![entry("preferences-1", "x")]);
    let json: serde_json::Value =
        serde_json::from_str(&render_list(Mode::Json, &records, 4, &[warning])).unwrap();
    assert_eq!(json["total"], 4);
    assert_eq!(json["truncated"], true);
    assert_eq!(json["entries"].as_array().map(Vec::len), Some(1));
    assert_eq!(json["warnings"][0]["code"], "store_corrupt");
}

#[test]
fn json_carries_null_for_absent_values_rather_than_omitting_them() {
    // STD-01 R11.
    let record = Record::Entry(Box::new(entry("preferences-1", "x")));
    let value: serde_json::Value = serde_json::from_str(&render_show(Mode::Json, &record)).unwrap();
    for key in ["superseded_by", "review_after"] {
        assert!(
            value.get(key).is_some_and(serde_json::Value::is_null),
            "{key}"
        );
    }
    assert!(value["content_from"]["role"].is_null());
    assert!(value["cite"]["message"].is_null());
    assert_eq!(value["related"], serde_json::json!([]));
    assert_eq!(value["created"], "2026-01-02T03:04:05Z");
}

#[test]
fn the_detail_view_keeps_the_content_whole() {
    // STD-01 R15: show is where a cut table cell is read in full.
    let long = "word ".repeat(60);
    let record = Record::Entry(Box::new(entry(
        "preferences-1",
        &format!("{long}\nsecond line"),
    )));
    let text = render_show_at(Mode::Table, Some(40), &record);
    assert!(text.contains(long.trim_end()), "{text}");
    assert!(text.contains("second line"), "{text}");
}

#[test]
fn a_single_record_list_is_still_a_table_and_counts_align_right() {
    // STD-01 R16: the shape does not depend on the record count.
    let tag = TagPayload {
        author: None,
        source: Some("config"),
        name: String::from("preferences"),
        aliases: vec![String::from("dec")],
        description: None,
        stale_after_days: None,
        configured: true,
        entries: 7,
        action: None,
        route: None,
    };
    let text = render_list(Mode::Table, &Records::Tags(vec![tag]), 1, &[]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("NAME"));
    let entries_col = lines[0].find("ENTRIES").unwrap() + "ENTRIES".len();
    assert_eq!(&lines[1][entries_col - 1..entries_col], "7", "{text}");
}

#[test]
fn handoff_only_session_renders_switch_state_in_every_mode() {
    let at = datetime!(2026-11-22 10:00:00 UTC);
    let mut payload = SessionHandoffsPayload {
        id: String::from("s-on"),
        has_record: false,
        project: String::from("scribe"),
        on: true,
        due: true,
        summary: String::new(),
        goals: String::new(),
        next: String::new(),
        handoffs: vec![HandoffPayload {
            id: String::from("handoff-1"),
            author: AuthorPayload {
                kind: "agent",
                name: String::from("opus"),
            },
            created: at,
            project: String::from("scribe"),
            quote: Some(String::from("write a handoff")),
            content: String::from("## State\nReady."),
            handoff_mode: None,
        }],
    };

    let handoffs = Record::SessionHandoffs(Box::new(payload.clone()));
    let json: serde_json::Value =
        serde_json::from_str(&render_show(Mode::Json, &handoffs)).unwrap();
    assert_eq!(json["has_record"], false);
    assert_eq!(json["on"], true);
    assert_eq!(json["due"], true);

    let plain = render_show(Mode::Plain, &handoffs);
    assert!(
        plain.starts_with("s-on\thandoff-only\tyes\tscribe\t-\t2026-11-22T10:00:00Z\t"),
        "{plain}"
    );

    let table = render_show(Mode::Table, &handoffs);
    assert!(table.contains("no catalog record yet"), "{table}");
    assert!(
        table
            .lines()
            .any(|l| l.starts_with("on ") && l.contains("yes")),
        "{table}"
    );
    assert!(
        table
            .lines()
            .any(|l| l.starts_with("due ") && l.contains("yes")),
        "{table}"
    );

    payload.on = false;
    payload.due = false;
    let off_table = render_show(Mode::Table, &Record::SessionHandoffs(Box::new(payload)));
    assert!(
        off_table.contains("session was not switched on"),
        "{off_table}"
    );
}

fn group(id: &str, summary: Option<&str>) -> scribe_core::group::Group {
    let session = |id: &str| scribe_core::SessionId::try_from(id.to_owned()).unwrap();
    let seat = |name: &str, chain: &[&str]| scribe_core::group::Seat {
        name: name.to_owned(),
        role: String::from("reviewer"),
        transcripts: chain.iter().map(|id| session(id)).collect(),
        newest_handoff: None,
    };
    scribe_core::group::Group {
        id: id.to_owned(),
        slug: None,
        title: String::from("Release review"),
        kind: String::from("review"),
        project: String::from("scribe"),
        opened: String::from("2026-11-22T09:00:00Z"),
        closed: None,
        summary: summary.map(str::to_owned),
        opener: String::from("5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f"),
        related: Vec::new(),
        seats: vec![
            seat("editor", &["first-session", "second-session"]),
            seat("checker", &["third-session"]),
        ],
    }
}

// The group list table holds short values: a seat count, no seat chains,
// opener or summary (STD-01 R15); an absent value is `-` (R14).
#[test]
fn the_group_list_table_holds_short_values_and_counts_seats() {
    let mut closed = group("g-1", Some("Review complete."));
    closed.opened = String::from("2026-11-22T23:30:00-05:00");
    closed.closed = Some(String::from("2026-11-24T00:30:00+02:00"));
    let records = Records::Groups(vec![closed, group("g-2", None)]);
    let text = render_list(Mode::Table, &records, 2, &[]);
    assert_eq!(
        text,
        "ID   TITLE           KIND    PROJECT  OPENED      CLOSED      SEATS\n\
         g-1  Release review  review  scribe   2026-11-23  2026-11-23      2\n\
         g-2  Release review  review  scribe   2026-11-22  -               2\n"
    );
}

#[test]
fn only_the_group_title_truncates_on_a_narrow_terminal() {
    let mut long = group("g-1", None);
    long.title = "A title that is far too long to fit on a narrow terminal".into();
    let mut narrow = sink_at(Mode::Table, Some(80));
    list(
        &mut narrow,
        &ShortIds::default(),
        &Records::Groups(vec![long]),
        1,
        &[],
        None,
    )
    .unwrap();
    let text = written(narrow);
    for line in text.lines() {
        assert!(line.chars().count() <= 80, "line too wide: {line:?}");
    }
    assert_eq!(text.matches('…').count(), 1, "{text}");
    assert!(text.lines().nth(1).unwrap().contains("g-1"), "{text}");
}

#[test]
fn the_group_plain_line_and_show_keep_every_field_and_dash_an_absent_summary() {
    let records = Records::Groups(vec![group("g-2", None)]);
    let plain = render_list(Mode::Plain, &records, 1, &[]);
    let fields: Vec<&str> = plain.trim_end().split('\t').collect();
    assert_eq!(fields.len(), 10, "{plain:?}");
    assert!(
        fields[8].contains("first-session -> second-session"),
        "{plain:?}"
    );
    assert_eq!(fields[4], "2026-11-22T09:00:00Z");
    assert_eq!(fields[9], "-");

    let mut shown_group = group("g-1", Some("Review complete."));
    shown_group.closed = Some(String::from("2026-11-22T10:00:00Z"));
    let shown = render_show(Mode::Table, &Record::Group(Box::new(shown_group)));
    for label in ["opener", "related", "seats", "summary"] {
        assert!(shown.lines().any(|l| l.starts_with(label)), "{shown}");
    }
    assert!(
        shown
            .lines()
            .any(|l| l.starts_with("opened") && l.contains("2026-11-22T09:00:00Z")),
        "{shown}"
    );
    assert!(
        shown
            .lines()
            .any(|l| l.starts_with("closed") && l.contains("2026-11-22T10:00:00Z")),
        "{shown}"
    );
    assert!(shown.contains("first-session -> second-session"), "{shown}");
}
