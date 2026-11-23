use crate::output::payload::{
    AuthorPayload, CitePayload, ContentFromPayload, EntryPayload, HandoffPayload, Record, Records,
    SessionHandoffsPayload, TagPayload, WarningPayload,
};
use crate::output::render::{list, show};
use crate::output::sink::{OutputMode, Sink};
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
        author: AuthorPayload {
            kind: "user",
            name: String::from("user"),
        },
        updated_by: None,
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
        retirement_reason: None,
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
        quote: None,
        evidence: None,
        as_of: None,
        window_turns: None,
        window: None,
        admitted: None,
        approved_by: None,
        instruction: None,
        stated_by: None,
        outcome: None,
        handoff_mode: None,
        removal_instruction: None,
        action: None,
    }
}

fn sink(mode: OutputMode) -> Sink {
    Sink {
        mode,
        color: false,
        width: None,
    }
}

fn render_list(
    mode: OutputMode,
    records: &Records,
    total: usize,
    warnings: &[WarningPayload],
) -> String {
    let mut out = Vec::new();
    list(&mut out, &sink(mode), records, total, warnings).unwrap();
    String::from_utf8(out).unwrap()
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
    let text = render_list(OutputMode::Plain, &records, 2, &[]);
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
        assert_eq!(render_list(OutputMode::Plain, &records, 0, &[]), "");
        assert_eq!(render_list(OutputMode::Table, &records, 0, &[]), "");
        let json: serde_json::Value =
            serde_json::from_str(&render_list(OutputMode::Json, &records, 0, &[])).unwrap();
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
        serde_json::from_str(&render_list(OutputMode::Json, &records, 4, &[warning])).unwrap();
    assert_eq!(json["total"], 4);
    assert_eq!(json["truncated"], true);
    assert_eq!(json["entries"].as_array().map(Vec::len), Some(1));
    assert_eq!(json["warnings"][0]["code"], "store_corrupt");
}

#[test]
fn json_carries_null_for_absent_values_rather_than_omitting_them() {
    // STD-01 R11.
    let mut out = Vec::new();
    let record = Record::Entry(Box::new(entry("preferences-1", "x")));
    show(&mut out, &sink(OutputMode::Json), &record).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
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
    let mut out = Vec::new();
    let record = Record::Entry(Box::new(entry(
        "preferences-1",
        &format!("{long}\nsecond line"),
    )));
    let narrow = Sink {
        width: Some(40),
        ..sink(OutputMode::Table)
    };
    show(&mut out, &narrow, &record).unwrap();
    let text = String::from_utf8(out).unwrap();
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
    let text = render_list(OutputMode::Table, &Records::Tags(vec![tag]), 1, &[]);
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

    let mut json_buf = Vec::new();
    show(
        &mut json_buf,
        &sink(OutputMode::Json),
        &Record::SessionHandoffs(Box::new(payload.clone())),
    )
    .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&json_buf).unwrap();
    assert_eq!(json["has_record"], false);
    assert_eq!(json["on"], true);
    assert_eq!(json["due"], true);

    let mut plain_buf = Vec::new();
    show(
        &mut plain_buf,
        &sink(OutputMode::Plain),
        &Record::SessionHandoffs(Box::new(payload.clone())),
    )
    .unwrap();
    let plain = String::from_utf8(plain_buf).unwrap();
    assert!(
        plain.starts_with("s-on\thandoff-only\tyes\tscribe\t-\t2026-11-22T10:00:00Z\t"),
        "{plain}"
    );

    let mut table_buf = Vec::new();
    show(
        &mut table_buf,
        &sink(OutputMode::Table),
        &Record::SessionHandoffs(Box::new(payload.clone())),
    )
    .unwrap();
    let table = String::from_utf8(table_buf).unwrap();
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
    let mut off_buf = Vec::new();
    show(
        &mut off_buf,
        &sink(OutputMode::Table),
        &Record::SessionHandoffs(Box::new(payload)),
    )
    .unwrap();
    let off_table = String::from_utf8(off_buf).unwrap();
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
    let records = Records::Groups(vec![
        group("g-1", Some("Review complete.")),
        group("g-2", None),
    ]);
    let text = render_list(OutputMode::Table, &records, 2, &[]);
    assert_eq!(
        text,
        "ID   TITLE           KIND    PROJECT  OPENED                CLOSED  SEATS\n\
         g-1  Release review  review  scribe   2026-11-22T09:00:00Z  -           2\n\
         g-2  Release review  review  scribe   2026-11-22T09:00:00Z  -           2\n"
    );
}

#[test]
fn only_the_group_title_truncates_on_a_narrow_terminal() {
    let mut long = group("g-1", None);
    long.title = "A title that is far too long to fit on a narrow terminal".into();
    let narrow = Sink {
        width: Some(80),
        ..sink(OutputMode::Table)
    };
    let mut out = Vec::new();
    list(&mut out, &narrow, &Records::Groups(vec![long]), 1, &[]).unwrap();
    let text = String::from_utf8(out).unwrap();
    for line in text.lines() {
        assert!(line.chars().count() <= 80, "line too wide: {line:?}");
    }
    assert_eq!(text.matches('…').count(), 1, "{text}");
    assert!(text.lines().nth(1).unwrap().contains("g-1"), "{text}");
}

#[test]
fn the_group_plain_line_and_show_keep_every_field_and_dash_an_absent_summary() {
    let records = Records::Groups(vec![group("g-2", None)]);
    let plain = render_list(OutputMode::Plain, &records, 1, &[]);
    let fields: Vec<&str> = plain.trim_end().split('\t').collect();
    assert_eq!(fields.len(), 10, "{plain:?}");
    assert!(
        fields[8].contains("first-session -> second-session"),
        "{plain:?}"
    );
    assert_eq!(fields[9], "-");

    let mut out = Vec::new();
    show(
        &mut out,
        &sink(OutputMode::Table),
        &Record::Group(Box::new(group("g-1", Some("Review complete.")))),
    )
    .unwrap();
    let shown = String::from_utf8(out).unwrap();
    for label in ["opener", "related", "seats", "summary"] {
        assert!(shown.lines().any(|l| l.starts_with(label)), "{shown}");
    }
    assert!(shown.contains("first-session -> second-session"), "{shown}");
}
