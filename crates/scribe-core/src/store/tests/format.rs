use super::{SESSION_BODY, entry_text, session_text};
use crate::error::Error;
use crate::store::format::{
    FORMAT, UPGRADES, decode_control, decode_entry, decode_pickups, decode_proposals,
    decode_session, encode_pickups, split_frontmatter, split_sections_v3,
};
use std::path::Path;

const ENTRY: &str = "entries/preferences/preferences-1.md";
const SESSION: &str = "sessions/2026/11/s-1.md";

fn entry(text: &str) -> Result<crate::entry::Entry, Error> {
    decode_entry(
        Path::new(ENTRY),
        text.as_bytes(),
        "preferences-1",
        "preferences",
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
    assert_eq!(e.id.to_string(), "preferences-1");
    assert_eq!(e.content, "run the gate before every push");
    assert_eq!(e.topics.as_slice()[0].as_str(), "ci");
    assert!(e.cite.as_ref().unwrap().message.is_none());
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
        "preferences-2",
        "preferences",
    )
    .unwrap_err();
    assert!(corrupt_message(&err).contains("do not match"), "{err:?}");
    let err = decode_entry(
        Path::new(ENTRY),
        text.as_bytes(),
        "preferences-1",
        "rituals",
    )
    .unwrap_err();
    corrupt_message(&err);
    let moved = text.replace("tag: preferences", "tag: rituals");
    let err = decode_entry(
        Path::new(ENTRY),
        moved.as_bytes(),
        "preferences-1",
        "rituals",
    )
    .unwrap_err();
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
    original.said = String::from("#scribe preferences: it's \"quoted\" #ci: and --- dashes");
    original.content = String::from("it's \"quoted\" #ci: and --- dashes");
    original.cite.as_mut().unwrap().message = Some(String::from("m-1"));
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

#[test]
fn a_format_five_control_and_proposals_document_reads_at_its_own_shape() {
    // Format 5 -> 6 changed neither document, so a format-5 file is checked
    // against the current shape, not the frozen format-3 one it used to fall to.
    let control = br#"{"format":5,"sessions":{"s-1":{"on":false,"since":null,"due":false,"final":false,"consecutive_failures":0,"retry_after":null,"pending_links":[],"cataloged_by":null}}}"#;
    let state = decode_control(Path::new("control.json"), control).unwrap();
    assert_eq!(state.sessions.len(), 1);
    let proposals = br#"{"format":5,"proposals":[]}"#;
    assert!(
        decode_proposals(Path::new("proposals.json"), proposals)
            .unwrap()
            .is_empty()
    );
    // Strictness holds at format 5: an unknown key is still refused.
    let stray = br#"{"format":5,"sessions":{},"stray":1}"#;
    assert!(decode_control(Path::new("control.json"), stray).is_err());
    let stray = br#"{"format":5,"proposals":[],"stray":1}"#;
    assert!(decode_proposals(Path::new("proposals.json"), stray).is_err());
}

#[test]
fn format_six_proposals_keep_agent_metadata_and_format_seven_controls_require_identity() {
    let old = br#"{"format":3,"proposals":[{"id":"p-1","kind":"stale","older":"preferences-1","newer":null,"reason":"review","quotes":[],"state":"open","opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null}]}"#;
    let path = Path::new("proposals.json");
    let mut proposals = decode_proposals(path, old).unwrap();
    proposals[0].updated_by = Some(crate::Author::agent("sol".parse().unwrap()));
    let encoded = crate::store::format::encode_proposals(path, &proposals).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    value["format"] = serde_json::json!(6);
    value["proposals"][0]
        .as_object_mut()
        .unwrap()
        .remove("candidate");
    value["proposals"][0]
        .as_object_mut()
        .unwrap()
        .remove("closed_reason");
    value["proposals"][0]
        .as_object_mut()
        .unwrap()
        .remove("instruction");
    let back = decode_proposals(path, &serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(back, proposals);
    let missing = br#"{"format":7,"sessions":{}}"#;
    assert!(decode_control(Path::new("control.json"), missing).is_err());
    let missing_provider = br#"{"format":7,"codex_cursors":{},"sessions":{"s-1":{"on":false,"since":null,"due":false,"final":false,"consecutive_failures":0,"retry_after":null,"pending_links":[],"cataloged_by":null}}}"#;
    assert!(decode_control(Path::new("control.json"), missing_provider).is_err());
}

#[test]
fn a_format_nine_candidate_reads_as_daniels_words_without_evidence() {
    // Format 9 -> 10: a candidate gains `role: user` and `evidence: null`;
    // a candidate already carrying format-10 keys at format 9 is refused
    // by the frozen shape.
    let cite = format!(
        r#"{{"session":"s-1","message":"m-1","at":"2026-11-01T10:00:00Z","prompt_sha256":"{}","line":1,"cwd":"/work"}}"#,
        "0".repeat(64)
    );
    let old = format!(
        r#"{{"format":9,"proposals":[{{"id":"p-1","kind":"candidate","older":"preferences-1","candidate":{{"tag":"preferences","quote":"I prefer short updates.","cite":{cite},"project":"scribe","provider":"claude-code","host":"host"}},"newer":null,"reason":"Unmarked preferences statement","quotes":[],"state":"open","opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null,"author":{{"kind":"scribe","name":"session-pass"}},"updated_by":null,"settled_author":null}}]}}"#
    );
    let proposals = decode_proposals(Path::new("proposals.json"), old.as_bytes()).unwrap();
    let candidate = proposals[0].candidate.as_ref().unwrap();
    assert_eq!(candidate.role, crate::Speaker::User);
    assert_eq!(candidate.evidence, None);
    let stray = old.replace(
        r#""tag":"preferences""#,
        r#""tag":"preferences","role":"user""#,
    );
    assert!(decode_proposals(Path::new("proposals.json"), stray.as_bytes()).is_err());
}

#[test]
fn a_format_ten_store_upgrades_with_null_admission_and_closed_reason_only() {
    // Format 10 -> 11: a candidate gains `admitted: null` and each proposal
    // `closed_reason: null`; nothing else changes, and a format-10 file
    // already carrying a format-11 key is refused by the frozen shape.
    let cite = format!(
        r#"{{"session":"s-1","message":"m-1","at":"2026-11-01T10:00:00Z","prompt_sha256":"{}","line":1,"cwd":"/work"}}"#,
        "0".repeat(64)
    );
    let candidate = format!(
        r#"{{"id":"p-1","kind":"candidate","older":"preferences-1","candidate":{{"tag":"preferences","role":"user","quote":"I prefer short updates.","evidence":null,"cite":{cite},"project":"scribe","provider":"claude-code","host":"host"}},"newer":null,"reason":"Unmarked preferences statement","quotes":[],"state":"open","opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null,"author":{{"kind":"scribe","name":"session-pass"}},"updated_by":null,"settled_author":null}}"#
    );
    let stale = r#"{"id":"p-2","kind":"stale","older":"preferences-2","candidate":null,"newer":null,"reason":"review","quotes":[],"state":"rejected","opened_at":"2026-11-01T10:00:00Z","settled_at":"2026-11-02T10:00:00Z","settled_by":null,"author":{"kind":"scribe","name":"consistency"},"updated_by":null,"settled_author":{"kind":"daniel","name":"daniel"}}"#;
    let old = format!(r#"{{"format":10,"proposals":[{candidate},{stale}]}}"#);
    let path = Path::new("proposals.json");
    let proposals = decode_proposals(path, old.as_bytes()).unwrap();
    assert_eq!(proposals.len(), 2);
    assert_eq!(proposals[0].candidate.as_ref().unwrap().admitted, None);
    assert!(proposals.iter().all(|p| p.closed_reason.is_none()));

    // Re-encoded at the current format, the old keys and values are untouched:
    // the document differs only by the added nulls and the format number.
    let encoded = crate::store::format::encode_proposals(path, &proposals).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["format"], FORMAT);
    for p in value["proposals"].as_array_mut().unwrap() {
        let p = p.as_object_mut().unwrap();
        assert_eq!(p.remove("closed_reason"), Some(serde_json::Value::Null));
        assert_eq!(p.remove("instruction"), Some(serde_json::Value::Null));
        if let Some(c) = p.get_mut("candidate").and_then(|c| c.as_object_mut()) {
            assert_eq!(c.remove("admitted"), Some(serde_json::Value::Null));
        }
    }
    value["format"] = serde_json::json!(10);
    let original: serde_json::Value = serde_json::from_str(&old).unwrap();
    assert_eq!(value, original);

    let stray = old.replace(
        r#""settled_author":null}"#,
        r#""settled_author":null,"closed_reason":null}"#,
    );
    assert!(decode_proposals(path, stray.as_bytes()).is_err());
    let stray = old.replace(r#""host":"host"}"#, r#""host":"host","admitted":null}"#);
    assert!(decode_proposals(path, stray.as_bytes()).is_err());
}

#[test]
fn format_seven_through_nine_controls_decode_and_older_ones_refuse_cursors() {
    // A format-7 `control.json` carries `codex_cursors` and a per-session
    // `provider` (format 6 -> 7, `codex_controls`); formats 8 and 9 leave
    // control.json unchanged, so all three share the shape and must decode.
    let path = Path::new("control.json");
    for format in [7, 8, 9] {
        let input = format!(
            r#"{{"format":{format},"codex_cursors":{{}},"sessions":{{"s-1":{{"provider":"claude-code","on":true,"since":null,"due":false,"final":false,"consecutive_failures":0,"retry_after":null,"pending_links":[],"cataloged_by":null}},"s-2":{{"provider":"codex","on":false,"since":null,"due":false,"final":false,"consecutive_failures":0,"retry_after":null,"pending_links":[],"cataloged_by":null}}}}}}"#
        );
        let state = decode_control(path, input.as_bytes())
            .unwrap_or_else(|e| panic!("format {format} control should decode: {e:?}"));
        assert_eq!(state.sessions.len(), 2, "format {format}");
        assert_eq!(
            state.sessions[&"s-1".parse().unwrap()].provider,
            crate::Provider::ClaudeCode,
            "format {format}"
        );
        assert_eq!(
            state.sessions[&"s-2".parse().unwrap()].provider,
            crate::Provider::Codex,
            "format {format}"
        );
        assert!(state.codex_cursors.is_empty(), "format {format}");
    }
    // A format-7 cursor entry survives the upgrade to the current format.
    let input = br#"{"format":7,"codex_cursors":{"s-1":"cursor-1"},"sessions":{"s-1":{"provider":"claude-code","on":true,"since":null,"due":false,"final":false,"consecutive_failures":0,"retry_after":null,"pending_links":[],"cataloged_by":null}}}"#;
    let state = decode_control(path, input).unwrap();
    assert_eq!(
        state.codex_cursors[&"s-1".parse().unwrap()],
        "cursor-1".to_string()
    );
    // Formats 4-6 predate `codex_cursors`: carrying it is corrupt.
    for format in [4, 5, 6] {
        let input = format!(
            r#"{{"format":{format},"codex_cursors":{{}},"sessions":{{"s-1":{{"on":false,"since":null,"due":false,"final":false,"consecutive_failures":0,"retry_after":null,"pending_links":[],"cataloged_by":null}}}}}}"#
        );
        assert!(
            decode_control(path, input.as_bytes()).is_err(),
            "format {format} with codex_cursors must be refused"
        );
    }
}

#[test]
fn format_six_controls_reject_duplicate_and_unknown_nested_author_fields() {
    for author in [
        r#"{"kind":"agent","name":"sol","name":"opus"}"#,
        r#"{"kind":"agent","name":"sol","unexpected":true}"#,
    ] {
        let input = format!(
            r#"{{"format":6,"sessions":{{"s-1":{{"on":true,"since":null,"due":true,"final":false,"consecutive_failures":0,"retry_after":null,"pending_links":[],"cataloged_by":{{"at":"2026-11-01T10:00:00Z","by":{author}}}}}}}}}"#
        );
        assert!(
            decode_control(Path::new("control.json"), input.as_bytes()).is_err(),
            "{input}"
        );
    }
}

/// A format-4 session record for `s-1` whose `log` metadata is `log` (YAML
/// list items) and whose body is `body`.
fn session_v4(log: &str, body: &str) -> String {
    session_text(
        &format!(
            "links: []\nactive: true\nkeep_active: false\nactive_change: null\ncataloged_by: null\nrebuilt_by: null\nlog:\n{log}"
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
        "rebuilt_by",
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
    original.rebuilt_by = Some(crate::record::Author::agent("opus".parse().unwrap()));
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
fn a_format_five_session_record_reads_with_no_rebuilder() {
    // Format 5 -> 6 adds `rebuilt_by`; a record no rebuild wrote reads as null.
    let text = two_segments()
        .replacen(
            &format!("format: {}", crate::store::format::FORMAT),
            "format: 5",
            1,
        )
        .replacen("rebuilt_by: null\n", "", 1);
    let r = session(&text).unwrap();
    assert_eq!(r.rebuilt_by, None);
    assert_eq!(r.log.len(), 2);
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
        {"id":"p-1","kind":"stale","older":"preferences-1","newer":null,"reason":"r",
         "quotes":[{"entry":"preferences-1","text":"t"}],"state":"open",
         "opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null},
        {"id":"p-2","kind":"stale","older":"preferences-1","newer":null,"reason":"r",
         "quotes":[{"entry":"preferences-1","text":"t"}],"state":"rejected",
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

#[test]
fn an_older_entry_reads_with_no_quote_and_no_window_and_a_window_round_trips() {
    let old = entry(&entry_text("")).unwrap();
    assert_eq!((old.quote.as_deref(), old.window.as_ref()), (None, None));

    let mut original = old;
    original.window = Some(crate::Window {
        turns: vec![crate::WindowTurn {
            role: crate::Speaker::Assistant,
            message: Some(String::from("a-1")),
            at: time::macros::datetime!(2026-11-01 09:59:00 UTC),
            text: String::from("Bytes, then: --- \"quoted\""),
            cut: true,
        }],
        truncated: true,
        omitted: 3,
    });
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &original).unwrap();
    assert_eq!(entry(&String::from_utf8(bytes).unwrap()).unwrap(), original);

    // Daniel's own entries never carry a quote: only an agent quotes Daniel.
    let mut quoted = original;
    quoted.quote = Some(String::from("Daniel's words"));
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &quoted).unwrap();
    corrupt_message(&entry(&String::from_utf8(bytes).unwrap()).unwrap_err());
}

#[test]
fn pickups_round_trip_oldest_first_and_only_agents_pick_up() {
    let path = Path::new("pickups.json");
    let author = |kind: &str, name: &str| -> crate::Author {
        serde_json::from_value(serde_json::json!({"kind": kind, "name": name})).unwrap()
    };
    let pickup = |entry: &str, minutes: i64, rejected: bool| crate::action::Pickup {
        entry: entry.parse().unwrap(),
        author: author("agent", "intake"),
        at: time::macros::datetime!(2026-11-01 10:00:00 UTC) + time::Duration::minutes(minutes),
        result: if rejected {
            None
        } else {
            Some("task:T-1".parse().unwrap())
        },
        rejected,
    };
    let pickups = [pickup("task-2", 5, true), pickup("task-1", 1, false)];
    let bytes = encode_pickups(path, &pickups).unwrap();
    let read = decode_pickups(path, &bytes).unwrap();
    assert_eq!(read, [pickups[1].clone(), pickups[0].clone()]);

    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["format"], FORMAT);
    doc["pickups"][0]["author"] = serde_json::json!({"kind": "daniel", "name": "daniel"});
    corrupt_message(&decode_pickups(path, doc.to_string().as_bytes()).unwrap_err());
    doc["pickups"][0]["colour"] = serde_json::json!("blue");
    corrupt_message(&decode_pickups(path, doc.to_string().as_bytes()).unwrap_err());
}

#[test]
fn a_format_eleven_store_upgrades_with_null_instruction_and_approved_by_only() {
    // Frozen format-11 entry (has format-10 `evidence` and `as_of`, no
    // `approved_by` or `instruction`) and format-11 proposals (has
    // `admitted` and `closed_reason`, no `instruction`).
    let sha = "0".repeat(64);
    let entry_v11 = format!(
        "---\n\
format: 11\n\
kind: entry\n\
id: preferences-1\n\
tag: preferences\n\
status: active\n\
project: scribe\n\
topics: [ci]\n\
related: [\"repo:scribe\"]\n\
provider: claude-code\n\
host: host\n\
author: {{kind: daniel, name: daniel}}\n\
updated_by: null\n\
created: 2026-11-01T10:00:00Z\n\
updated: 2026-11-01T10:00:00Z\n\
said: \"#scribe preferences: run the gate\"\n\
content_from: {{kind: instruction}}\n\
cite: {{session: s-1, message: m-1, at: 2026-11-01T10:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}\n\
retirement_reason: null\n\
superseded_by: null\n\
review_after: null\n\
quote: null\n\
window: null\n\
evidence: null\n\
as_of: null\n\
---\n\
run the gate before every push\n"
    );
    let upgraded_entry = entry(&entry_v11).unwrap();
    assert_eq!(upgraded_entry.approved_by, None);
    assert_eq!(upgraded_entry.instruction, None);
    assert_eq!(upgraded_entry.content, "run the gate before every push");

    // Re-encoded at format 12, the entry differs only by `format: 12` and
    // the two new null fields.
    let reencoded = String::from_utf8(
        crate::store::format::encode_entry(Path::new(ENTRY), &upgraded_entry).unwrap(),
    )
    .unwrap();
    assert!(reencoded.starts_with("---\nformat: 12\n"), "{reencoded}");
    let (reencoded_front, reencoded_body) =
        split_frontmatter(Path::new(ENTRY), reencoded.as_bytes()).unwrap();
    let (v11_front, v11_body) = split_frontmatter(Path::new(ENTRY), entry_v11.as_bytes()).unwrap();
    assert_eq!(reencoded_body, v11_body);
    let mut reencoded_map: yaml_serde::Mapping = yaml_serde::from_str(reencoded_front).unwrap();
    let mut v11_map: yaml_serde::Mapping = yaml_serde::from_str(v11_front).unwrap();
    assert_eq!(
        reencoded_map.remove("format"),
        Some(yaml_serde::Value::from(12))
    );
    assert_eq!(
        reencoded_map.remove("approved_by"),
        Some(yaml_serde::Value::Null)
    );
    assert_eq!(
        reencoded_map.remove("instruction"),
        Some(yaml_serde::Value::Null)
    );
    assert_eq!(v11_map.remove("format"), Some(yaml_serde::Value::from(11)));
    assert_eq!(reencoded_map, v11_map);

    // Format-11 entries already carrying format-12 keys are refused.
    for extra in ["approved_by: null\n", "instruction: null\n"] {
        let stray = entry_v11.replacen("as_of: null\n", &format!("as_of: null\n{extra}"), 1);
        assert!(entry(&stray).is_err(), "{extra}");
    }

    // Frozen format-11 proposals.json: one open candidate with `admitted`,
    // one candidate closed with `tag_retired`.
    let cite = format!(
        r#"{{"session":"s-1","message":"m-1","at":"2026-11-01T10:00:00Z","prompt_sha256":"{sha}","line":1,"cwd":"/work"}}"#
    );
    let open = format!(
        r#"{{"id":"p-1","kind":"candidate","older":"preferences-1","candidate":{{"tag":"preferences","role":"user","quote":"I prefer short updates.","evidence":null,"cite":{cite},"project":"scribe","provider":"claude-code","host":"host","admitted":{{"trigger":"I prefer","matched":"I prefer","criteria":null}}}},"newer":null,"reason":"Unmarked preferences statement","quotes":[],"state":"open","opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null,"author":{{"kind":"scribe","name":"session-pass"}},"updated_by":null,"settled_author":null,"closed_reason":null}}"#
    );
    let closed = format!(
        r#"{{"id":"p-2","kind":"candidate","older":"decisions-1","candidate":{{"tag":"decisions","role":"user","quote":"We decided on X.","evidence":null,"cite":{cite},"project":"scribe","provider":"claude-code","host":"host","admitted":null}},"newer":null,"reason":"Unmarked decisions statement","quotes":[],"state":"rejected","opened_at":"2026-11-01T10:00:00Z","settled_at":"2026-11-02T10:00:00Z","settled_by":null,"author":{{"kind":"scribe","name":"session-pass"}},"updated_by":null,"settled_author":{{"kind":"scribe","name":"tag-retired"}},"closed_reason":"tag_retired"}}"#
    );
    let old = format!(r#"{{"format":11,"proposals":[{open},{closed}]}}"#);
    let path = Path::new("proposals.json");
    let proposals = decode_proposals(path, old.as_bytes()).unwrap();
    assert_eq!(proposals.len(), 2);
    assert!(proposals.iter().all(|p| p.instruction.is_none()));
    assert_eq!(proposals[0].closed_reason, None);
    assert_eq!(
        proposals[1].closed_reason,
        Some(crate::ClosedReason::TagRetired)
    );

    let encoded = crate::store::format::encode_proposals(path, &proposals).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["format"], 12);
    for p in value["proposals"].as_array_mut().unwrap() {
        let p = p.as_object_mut().unwrap();
        assert_eq!(p.remove("instruction"), Some(serde_json::Value::Null));
    }
    value["format"] = serde_json::json!(11);
    let original: serde_json::Value = serde_json::from_str(&old).unwrap();
    assert_eq!(value, original);

    // Format-11 proposals carrying format-12 fields or `instructed` are refused.
    let stray = old.replace(
        r#""closed_reason":null}"#,
        r#""closed_reason":null,"instruction":null}"#,
    );
    assert!(decode_proposals(path, stray.as_bytes()).is_err());
    let stray_reason = old.replace(
        r#""closed_reason":"tag_retired""#,
        r#""closed_reason":"instructed""#,
    );
    assert!(decode_proposals(path, stray_reason.as_bytes()).is_err());
}

#[test]
fn closed_reason_accepts_instructed_and_older_values_read_unchanged() {
    let path = Path::new("proposals.json");
    let doc = serde_json::json!({
        "format": FORMAT,
        "proposals": [
            {
                "id": "p-1",
                "kind": "stale",
                "older": "preferences-1",
                "candidate": null,
                "newer": null,
                "reason": "review",
                "quotes": [],
                "state": "rejected",
                "opened_at": "2026-11-01T10:00:00Z",
                "settled_at": "2026-11-02T10:00:00Z",
                "settled_by": {
                    "session": "s-1",
                    "message": "m-9",
                    "at": "2026-11-02T10:00:00Z",
                    "prompt_sha256": "0".repeat(64),
                    "line": 1,
                    "cwd": "/work"
                },
                "author": {"kind": "scribe", "name": "consistency"},
                "updated_by": null,
                "settled_author": {"kind": "agent", "name": "aide"},
                "closed_reason": "instructed",
                "instruction": {
                    "session": "s-1",
                    "message": "m-9",
                    "at": "2026-11-02T10:00:00Z",
                    "quote": "reject p-1"
                }
            },
            {
                "id": "p-2",
                "kind": "stale",
                "older": "preferences-2",
                "candidate": null,
                "newer": null,
                "reason": "review",
                "quotes": [],
                "state": "rejected",
                "opened_at": "2026-11-01T10:00:00Z",
                "settled_at": "2026-11-02T10:00:00Z",
                "settled_by": null,
                "author": {"kind": "scribe", "name": "consistency"},
                "updated_by": null,
                "settled_author": {"kind": "scribe", "name": "tag-retired"},
                "closed_reason": "tag_retired",
                "instruction": null
            }
        ]
    });
    let proposals = decode_proposals(path, doc.to_string().as_bytes()).unwrap();
    assert_eq!(
        proposals[0].closed_reason,
        Some(crate::ClosedReason::Instructed)
    );
    assert_eq!(
        proposals[0]
            .instruction
            .as_ref()
            .and_then(|i| i.quote.as_deref()),
        Some("reject p-1")
    );
    assert_eq!(
        proposals[1].closed_reason,
        Some(crate::ClosedReason::TagRetired)
    );
    assert_eq!(proposals[1].instruction, None);

    let encoded = crate::store::format::encode_proposals(path, &proposals).unwrap();
    assert_eq!(decode_proposals(path, &encoded).unwrap(), proposals);

    // `closed_reason: instructed` on a non-rejected proposal is refused.
    let mut bad = doc;
    bad["proposals"][0]["state"] = serde_json::json!("open");
    assert!(
        corrupt_message(&decode_proposals(path, bad.to_string().as_bytes()).unwrap_err())
            .contains("closed_reason")
    );
}

#[test]
fn instructed_entry_fields_round_trip_and_validate_approved_by() {
    let mut original = entry(&entry_text("")).unwrap();
    original.approved_by = Some(crate::Author::agent("aide".parse().unwrap()));
    original.instruction = Some(crate::Instruction {
        session: "s-2".parse().unwrap(),
        message: String::from("u-7"),
        at: time::macros::datetime!(2026-11-02 12:00:00 UTC),
        quote: Some(String::from("approve p-1")),
    });
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &original).unwrap();
    let back = entry(&String::from_utf8(bytes).unwrap()).unwrap();
    assert_eq!(back, original);

    // Forgotten entry drops the instruction's quote to `null`.
    let mut forgotten = original.clone();
    forgotten.status = crate::EntryStatus::Forgotten;
    forgotten.said.clear();
    forgotten.content.clear();
    forgotten.instruction.as_mut().unwrap().quote = None;
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &forgotten).unwrap();
    let back = entry(&String::from_utf8(bytes).unwrap()).unwrap();
    assert_eq!(back, forgotten);
    assert_eq!(back.instruction.unwrap().quote, None);

    // `approved_by` must be an agent author, never Daniel or scribe.
    for non_agent in [crate::Author::daniel(), crate::Author::consistency()] {
        let mut bad = original.clone();
        bad.approved_by = Some(non_agent);
        let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &bad).unwrap();
        corrupt_message(&entry(&String::from_utf8(bytes).unwrap()).unwrap_err());
    }
}
