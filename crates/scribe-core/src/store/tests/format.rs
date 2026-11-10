use super::{SESSION_BODY, entry_text, session_text};
use crate::error::Error;
use crate::store::format::{
    FORMAT, UPGRADES, decode_control, decode_entry, decode_proposals, decode_session,
    split_frontmatter, split_sections_v3,
};
use std::path::Path;

const ENTRY: &str = "entries/decisions/decisions-1.md";
const SESSION: &str = "sessions/2026/11/s-1.md";

fn entry(text: &str) -> Result<crate::entry::Entry, Error> {
    decode_entry(
        Path::new(ENTRY),
        text.as_bytes(),
        "decisions-1",
        "decisions",
    )
}

fn session(text: &str) -> Result<crate::session::SessionRecord, Error> {
    decode_session(Path::new(SESSION), text.as_bytes(), "s-1", ("2026", "11"))
}

/// The full message of a corrupt-record error, causes included.
fn corrupt_message(err: &Error) -> String {
    assert!(matches!(err, Error::Corrupt { .. }), "got {err:?}");
    let mut text = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[test]
fn the_format_is_one_more_than_the_number_of_upgrade_steps() {
    // STD-03 R23: a format bump without its upgrade step, or the reverse,
    // fails here rather than stranding older stores.
    assert_eq!(usize::try_from(FORMAT).unwrap(), UPGRADES.len() + 1);
}

#[test]
fn an_entry_file_reads_whole_with_its_body_as_the_content() {
    let e = entry(&entry_text("")).unwrap();
    assert_eq!(e.id.to_string(), "decisions-1");
    assert_eq!(e.content, "run the gate before every push");
    assert_eq!(e.topics.as_slice()[0].as_str(), "ci");
    assert!(e.cite.message.is_none());
}

#[test]
fn an_unknown_frontmatter_key_is_refused_naming_the_file_and_key() {
    let err = entry(&entry_text("colour: blue\n")).unwrap_err();
    let message = corrupt_message(&err);
    assert!(message.contains(ENTRY), "{message}");
    assert!(message.contains("colour"), "{message}");
}

#[test]
fn a_missing_key_is_refused_even_when_its_value_may_be_null() {
    for key in ["superseded_by", "review_after", "project", "cite"] {
        let text = entry_text("")
            .lines()
            .filter(|l| !l.starts_with(&format!("{key}:")))
            .collect::<Vec<_>>()
            .join("\n");
        let message = corrupt_message(&entry(&text).unwrap_err());
        assert!(message.contains(ENTRY), "{message}");
        assert!(message.contains(key), "{key}: {message}");
    }
}

#[test]
fn a_repeated_key_is_refused() {
    let err = entry(&entry_text("status: retired\n")).unwrap_err();
    assert!(corrupt_message(&err).contains("status"), "{err:?}");
}

#[test]
fn a_newer_format_is_refused_by_version_not_by_shape() {
    // STD-03 R10: the unknown key must not turn this into a parse error; the
    // format number decides first.
    let text = entry_text("shape: new\n").replace("format: 1", &format!("format: {}", FORMAT + 1));
    let err = entry(&text).unwrap_err();
    assert!(
        matches!(err, Error::NewerFormat { found, supported, .. } if found == FORMAT + 1 && supported == FORMAT),
        "got {err:?}"
    );
    let control = format!(r#"{{"format": {}, "shape": "new"}}"#, FORMAT + 1);
    let err = decode_control(Path::new("control.json"), control.as_bytes()).unwrap_err();
    assert!(matches!(err, Error::NewerFormat { .. }), "got {err:?}");
}

#[test]
fn format_zero_a_wrong_kind_or_no_frontmatter_reads_as_corrupt() {
    for text in [
        entry_text("").replace("format: 1", "format: 0"),
        entry_text("").replace("kind: entry", "kind: session"),
        entry_text("").replace("kind: entry\n", ""),
        String::from("run the gate\n"),
        entry_text("").replacen("---\nrun", "run", 1),
    ] {
        let err = entry(&text).unwrap_err();
        corrupt_message(&err);
    }
}

#[test]
fn an_entry_whose_id_does_not_match_its_file_is_refused() {
    let text = entry_text("");
    let err = decode_entry(
        Path::new(ENTRY),
        text.as_bytes(),
        "decisions-2",
        "decisions",
    )
    .unwrap_err();
    assert!(corrupt_message(&err).contains("do not match"), "{err:?}");
    let err =
        decode_entry(Path::new(ENTRY), text.as_bytes(), "decisions-1", "rituals").unwrap_err();
    corrupt_message(&err);
    let moved = text.replace("tag: decisions", "tag: rituals");
    let err =
        decode_entry(Path::new(ENTRY), moved.as_bytes(), "decisions-1", "rituals").unwrap_err();
    corrupt_message(&err);
}

#[test]
fn the_frontmatter_closes_at_the_first_dashes_line() {
    let (front, body) =
        split_frontmatter(Path::new(ENTRY), b"---\na: 1\n---\nx\n---\ny\n").unwrap();
    assert_eq!(front, "a: 1\n");
    assert_eq!(body, "x\n---\ny");
    let (front, body) = split_frontmatter(Path::new(ENTRY), b"---\na: 1\n---").unwrap();
    assert_eq!((front, body), ("a: 1\n", ""));
}

#[test]
fn a_session_record_reads_its_four_sections() {
    let r = session(&session_text("", SESSION_BODY)).unwrap();
    assert_eq!(r.summary, "sum");
    assert_eq!(r.goals, "goal");
    assert_eq!(r.next, "- one\n- two");
    let err = decode_session(
        Path::new(SESSION),
        session_text("", SESSION_BODY).as_bytes(),
        "s-1",
        ("2026", "10"),
    )
    .unwrap_err();
    assert!(
        corrupt_message(&err).contains("does not match"),
        "a record filed under the wrong month"
    );
}

#[test]
fn a_session_keeps_its_terms_and_refuses_a_malformed_or_missing_list() {
    let r = session(&session_text("", SESSION_BODY)).unwrap();
    assert_eq!(r.terms.len(), 1);
    assert_eq!(r.terms[0].term, "level 4");
    assert_eq!(r.terms[0].said, "graduate to level 4");
    assert_eq!(r.terms[0].message, "m-1");
    let extra = session_text("", SESSION_BODY).replace("message: m-1}", "message: m-1, why: x}");
    assert!(corrupt_message(&session(&extra).unwrap_err()).contains("why"));
    let missing = session_text("", SESSION_BODY).replace(", message: m-1}", "}");
    assert!(corrupt_message(&session(&missing).unwrap_err()).contains("message"));
    let absent: String = session_text("", SESSION_BODY)
        .lines()
        .filter(|l| !l.starts_with("terms:"))
        .map(|l| format!("{l}\n"))
        .collect();
    assert!(corrupt_message(&session(&absent).unwrap_err()).contains("terms"));
}

#[test]
fn sections_must_all_appear_in_order() {
    let path = Path::new(SESSION);
    for body in [
        "## Goals\ng\n## Summary\ns\n## Outcome\no\n## Next\nn\n",
        "## Summary\ns\n## Goals\ng\n## Outcome\no\n",
        "text first\n## Summary\ns\n## Goals\ng\n## Outcome\no\n## Next\nn\n",
        "",
    ] {
        assert!(split_sections_v3(path, body).is_err(), "{body:?}");
    }
    let [s, _, o, _] = split_sections_v3(
        path,
        "## Summary\n## Other\ns\n## Goals\n## Outcome\no\n## Next\n",
    )
    .unwrap();
    assert_eq!(s, "## Other\ns", "another heading is section text");
    assert_eq!(o, "o");
}

#[test]
fn control_and_proposals_are_strict_json() {
    let control = br#"{"format":1,"sessions":{"s-1":{"on":true,"since":"2026-11-01T09:00:00Z","due":false,"final":false}}}"#;
    let state = decode_control(Path::new("control.json"), control).unwrap();
    assert!(state.get(&"s-1".parse().unwrap()).unwrap().on);
    let extra = br#"{"format":1,"sessions":{},"paused":true}"#;
    corrupt_message(&decode_control(Path::new("control.json"), extra).unwrap_err());
    let missing = br#"{"format":1,"sessions":{"s-1":{"on":true,"due":false,"final":false}}}"#;
    corrupt_message(&decode_control(Path::new("control.json"), missing).unwrap_err());
    let empty = br#"{"format":1,"proposals":[]}"#;
    assert!(
        decode_proposals(Path::new("proposals.json"), empty)
            .unwrap()
            .is_empty()
    );
    let repeated = br#"{"format":1,"proposals":[],"proposals":[]}"#;
    corrupt_message(&decode_proposals(Path::new("proposals.json"), repeated).unwrap_err());
}

#[test]
fn an_encoded_entry_reads_back_as_the_same_entry() {
    let mut original = entry(&entry_text("")).unwrap();
    original.said = String::from("#scribe decisions: it's \"quoted\" #ci: and --- dashes");
    original.content = String::from("it's \"quoted\" #ci: and --- dashes");
    original.cite.message = Some(String::from("m-1"));
    original.review_after = Some(time::macros::date!(2027 - 01 - 02));
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &original).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        text.starts_with(&format!("---\nformat: {FORMAT}\nkind: entry\n")),
        "{text}"
    );
    assert_eq!(entry(&text).unwrap(), original);
}

#[test]
fn an_encoded_session_record_reads_back_as_the_same_record() {
    let mut original = session(&session_text("", SESSION_BODY)).unwrap();
    original.title = String::from("it's \"quoted\": and --- dashes");
    original.summary = String::from("line one\n\nline two\n---\nafter dashes");
    original.goals = String::new();
    original.next = String::from("- one\n- two");
    original.scribed_through = Some(String::from("m-9"));
    original.terms.push(crate::session::Term {
        term: String::from("agent PR"),
        said: String::from("by agent PR I mean\nany PR a crew opens: \"all\""),
        message: String::from("m-2"),
    });
    let bytes = crate::store::format::encode_session(Path::new(SESSION), &original).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.starts_with(&format!("---\nformat: {FORMAT}\nkind: session\n")),
        "{text}"
    );
    assert_eq!(session(&text).unwrap(), original);
}

#[test]
fn encoded_control_reads_back_as_the_same_state() {
    let control = br#"{"format":1,"sessions":{"s-1":{"on":true,"since":"2026-11-01T09:00:00Z","due":true,"final":false},"s-2":{"on":false,"since":null,"due":false,"final":false}}}"#;
    let path = Path::new("control.json");
    let state = decode_control(path, control).unwrap();
    let bytes = crate::store::format::encode_control(path, &state).unwrap();
    assert_eq!(decode_control(path, &bytes).unwrap(), state);
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["format"], FORMAT);
    assert_eq!(value["sessions"]["s-1"]["final"], false);
}

/// A format-4 session record for `s-1` whose `log` metadata is `log` (YAML
/// list items) and whose body is `body`.
fn session_v4(log: &str, body: &str) -> String {
    session_text(
        &format!(
            "links: []\nactive: true\nkeep_active: false\nactive_change: null\ncataloged_by: null\nlog:\n{log}"
        ),
        body,
    )
    .replacen("format: 1", &format!("format: {FORMAT}"), 1)
}

fn segment_yaml(n: u32, heading: &str, state: &str) -> String {
    format!(
        "  - {{n: {n}, heading: {heading}, first_message: m-{n}a, last_message: m-{n}b, \
         from: 2026-11-01T09:00:00Z, to: 2026-11-01T10:00:00Z, state: {state}, windows: 1, \
         origin: pass, topics: [], related: []}}\n"
    )
}

const V4_HEADER: &str = "## Summary\nsum\n\n## Goals\ngoal\n\n## Next\n- one\n\n";

fn two_segments() -> String {
    session_v4(
        &[
            segment_yaml(1, "first part", "sealed"),
            segment_yaml(2, "second part", "open"),
        ]
        .concat(),
        &format!("{V4_HEADER}## Log\n### 1. first part\none\n\n### 2. second part\ntwo\n"),
    )
}

#[test]
fn a_legacy_session_record_reads_as_a_header_over_one_sealed_upgrade_segment() {
    // Formats 1 and 2 predate links; formats 1 to 3 predate the log.
    for format in ["format: 1", "format: 2", "format: 3"] {
        let mut text = session_text("", SESSION_BODY).replacen("format: 1", format, 1);
        if format == "format: 3" {
            text = text.replacen("next_open: 0\n", "next_open: 0\nlinks: []\n", 1);
        }
        let r = session(&text).unwrap();
        assert!(r.links.is_empty(), "{format}");
        assert!(r.active && !r.keep_active, "{format}");
        assert_eq!(r.active_change, None, "{format}");
        assert_eq!(r.cataloged_by, None, "{format}");
        assert_eq!((r.summary.as_str(), r.goals.as_str()), ("sum", "goal"));
        assert_eq!(r.next, "- one\n- two");
        assert_eq!(r.log.len(), 1, "{format}");
        let one = &r.log[0];
        assert_eq!(one.n.get(), 1);
        assert_eq!(one.state, crate::session::SegmentState::Sealed);
        assert_eq!(one.origin, crate::session::SegmentOrigin::Upgrade);
        assert_eq!(one.heading, "a session");
        assert_eq!(one.text, "sum\n\nout", "{format}: old summary and outcome");
        assert_eq!(one.first_message, None);
        assert_eq!(one.last_message, r.scribed_through);
        assert_eq!((one.from, one.to), (r.started, r.updated));
        assert_eq!(one.windows.get(), 1);
    }
    // At the current format each new key is required.
    for key in [
        "links",
        "active",
        "keep_active",
        "active_change",
        "cataloged_by",
        "log",
    ] {
        let missing: String = two_segments()
            .split_inclusive('\n')
            .filter(|l| !l.starts_with(&format!("{key}:")))
            .collect();
        let missing = if key == "log" {
            missing
                .lines()
                .filter(|l| !l.starts_with("  - "))
                .map(|l| format!("{l}\n"))
                .collect()
        } else {
            missing
        };
        assert!(
            corrupt_message(&session(&missing).unwrap_err()).contains(key),
            "{key}"
        );
    }
}

#[test]
fn a_current_session_record_reads_its_header_and_log() {
    let r = session(&two_segments()).unwrap();
    assert_eq!((r.summary.as_str(), r.goals.as_str()), ("sum", "goal"));
    assert_eq!(r.next, "- one");
    let texts: Vec<_> = r
        .log
        .iter()
        .map(|s| (s.heading.as_str(), s.text.as_str()))
        .collect();
    assert_eq!(texts, [("first part", "one"), ("second part", "two")]);
    assert_eq!(r.open_segment().map(|s| s.n.get()), Some(2));
    assert_eq!(r.segment(1).map(|s| s.text.as_str()), Some("one"));
    assert!(r.segment(3).is_none());
}

#[test]
fn a_log_whose_body_and_metadata_disagree_is_corrupt_naming_the_file() {
    let good = two_segments();
    let open_twice = good.replacen("state: sealed", "state: open", 1);
    let cases = [
        good.replace("### 2. second part", "### 2. another part"),
        good.replace("\n\n### 2. second part\ntwo\n", "\n"),
        good.replace(
            "### 2. second part\ntwo\n",
            "### 2. second part\ntwo\n\n### 3. third\nthree\n",
        ),
        good.replace("## Log\n", "## Outcome\nout\n\n## Log\n"),
        good.replace("## Log\n### 1.", "## Log\nloose text\n### 1."),
        good.replace("n: 2,", "n: 3,").replace("### 2.", "### 3."),
        open_twice,
        good.replace("status: open", "status: final"),
    ];
    for text in cases {
        let message = corrupt_message(&session(&text).unwrap_err());
        assert!(message.contains(SESSION), "{message}");
    }
}

#[test]
fn the_activity_and_author_fields_round_trip() {
    let mut original = session(&two_segments()).unwrap();
    let daniel = crate::record::Author::daniel();
    let at = time::macros::datetime!(2026-11-02 08:00 UTC);
    original.active = false;
    original.keep_active = true;
    original.active_change = Some(crate::activity::ActiveChange {
        at,
        by: daniel.clone(),
        route: crate::activity::ActiveRoute::Manual,
        reason: String::from("the feature was removed: \"all\" of it"),
        cite: Some(String::from("session:s-0#2")),
    });
    original.cataloged_by = Some(crate::activity::CatalogedBy { at, by: daniel });
    let bytes = crate::store::format::encode_session(Path::new(SESSION), &original).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.contains("\n## Log\n### 1. first part\none\n"),
        "{text}"
    );
    assert!(!text.contains("## Outcome"), "{text}");
    assert_eq!(session(&text).unwrap(), original);

    let mut e = entry(&entry_text("")).unwrap();
    e.updated_by = Some(crate::record::Author::consistency());
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &e).unwrap();
    let back = entry(&String::from_utf8(bytes).unwrap()).unwrap();
    assert_eq!(back, e);
    assert_eq!(back.author.to_string(), "daniel:daniel");
    assert_eq!(back.updated_by.unwrap().to_string(), "scribe:consistency");
}

#[test]
fn a_session_the_writer_cannot_keep_apart_is_refused() {
    let mut r = session(&two_segments()).unwrap();
    r.log[0].text = String::from("one\n### 2. forged");
    assert!(crate::store::format::encode_session(Path::new(SESSION), &r).is_err());
    let mut r = session(&two_segments()).unwrap();
    r.summary = String::from("sum\n## Log");
    assert!(crate::store::format::encode_session(Path::new(SESSION), &r).is_err());
}

#[test]
fn legacy_entries_proposals_and_control_gain_their_authors() {
    let e = entry(&entry_text("")).unwrap();
    assert_eq!(e.author, crate::record::Author::daniel());
    assert_eq!(e.updated_by, None);
    let current = entry_text("").replacen("format: 1", &format!("format: {FORMAT}"), 1);
    assert!(corrupt_message(&entry(&current).unwrap_err()).contains("author"));

    let proposals = br#"{"format":3,"proposals":[
        {"id":"p-1","kind":"stale","older":"decisions-1","newer":null,"reason":"r",
         "quotes":[{"entry":"decisions-1","text":"t"}],"state":"open",
         "opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null},
        {"id":"p-2","kind":"stale","older":"decisions-1","newer":null,"reason":"r",
         "quotes":[{"entry":"decisions-1","text":"t"}],"state":"rejected",
         "opened_at":"2026-11-01T10:00:00Z","settled_at":"2026-11-01T11:00:00Z","settled_by":null}]}"#;
    let list = decode_proposals(Path::new("proposals.json"), proposals).unwrap();
    for p in &list {
        assert_eq!(p.author, crate::record::Author::consistency());
    }
    assert_eq!(list[0].settled_author, None, "open: not settled");
    assert_eq!(
        list[1].settled_author,
        Some(crate::record::Author::daniel())
    );

    let control = br#"{"format":3,"sessions":{"s-1":{"on":true,"since":null,"due":false,
        "final":false,"consecutive_failures":0,"retry_after":null,"pending_links":[]}}}"#;
    let state = decode_control(Path::new("control.json"), control).unwrap();
    assert_eq!(state.sessions[&"s-1".parse().unwrap()].cataloged_by, None);
    let unknown = String::from_utf8(control.to_vec()).unwrap().replace(
        "\"pending_links\":[]",
        "\"pending_links\":[],\"cataloged_by\":null",
    );
    assert!(
        decode_control(Path::new("control.json"), unknown.as_bytes()).is_err(),
        "a format-3 switch has no cataloged_by"
    );
}

#[test]
fn a_format_two_control_reads_with_no_pending_links() {
    let text = r#"{"format": 2, "sessions": {"s-1": {"on": true, "since": null, "due": false,
        "final": false, "consecutive_failures": 1, "retry_after": null}}}"#;
    let state = decode_control(Path::new("control.json"), text.as_bytes()).unwrap();
    let switch = &state.sessions[&"s-1".parse().unwrap()];
    assert_eq!(switch.consecutive_failures, 1);
    assert!(switch.pending_links.is_empty());
    let unknown = text.replace("\"retry_after\": null", "\"retry_after\": null, \"x\": 1");
    assert!(decode_control(Path::new("control.json"), unknown.as_bytes()).is_err());
}
