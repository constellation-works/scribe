use crate::output::payload::{
    AuthorPayload, CitePayload, ContentFromPayload, EntryPayload, Record, Records, TagPayload,
    WarningPayload,
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
            kind: "daniel",
            name: String::from("daniel"),
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
            "preferences-1\tactive\tscribe\tci,release\t2026-01-02T03:04:05Z\tone two three\tdaniel\t-",
            "preferences-2\tactive\tscribe\tci,release\t2026-01-02T03:04:05Z\t-\tdaniel\t-",
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
