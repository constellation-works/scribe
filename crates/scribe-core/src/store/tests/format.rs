use super::{SESSION_BODY, entry_text, session_text};
use crate::error::Error;
use crate::store::format::{
    FORMAT, UPGRADES, decode_control, decode_entry, decode_marks, decode_pickups, decode_proposals,
    decode_session, encode_marks, encode_pickups, split_frontmatter, split_sections_v3,
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

    // Re-encoded at the current format, the old keys and values are
    // untouched: the document differs only by the new nulls and the
    // format number (formats 12 to 14 leave proposals' old keys alone).
    let encoded = crate::store::format::encode_proposals(path, &proposals).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["format"], FORMAT);
    for p in value["proposals"].as_array_mut().unwrap() {
        let p = p.as_object_mut().unwrap();
        assert_eq!(p.remove("closed_reason"), Some(serde_json::Value::Null));
        assert_eq!(p.remove("instruction"), Some(serde_json::Value::Null));
        if let Some(c) = p.get_mut("candidate").and_then(|c| c.as_object_mut()) {
            assert_eq!(c.remove("admitted"), Some(serde_json::Value::Null));
            assert_eq!(c.remove("statement"), Some(serde_json::Value::Null));
            assert_eq!(c.remove("context"), Some(serde_json::Value::Null));
            assert_eq!(c.remove("subject"), Some(serde_json::Value::Null));
        }
    }
    value["format"] = serde_json::json!(10);
    let mut original: serde_json::Value = serde_json::from_str(&old).unwrap();
    // Format 23 renames the user's author value, which the read maps.
    original["proposals"][1]["settled_author"] =
        serde_json::json!({"kind": "user", "name": "user"});
    assert_eq!(value, original);

    let stray = old.replace(
        r#""settled_author":null}"#,
        r#""settled_author":null,"closed_reason":null}"#,
    );
    assert!(decode_proposals(path, stray.as_bytes()).is_err());
    let stray = old.replace(r#""host":"host"}"#, r#""host":"host","admitted":null}"#);
    assert!(decode_proposals(path, stray.as_bytes()).is_err());
}

/// What Daniel typed, and the body it recorded, in the format-11 fixture.
const SAID: &str = "#scribe preferences: run the gate";
const BODY: &str = "run the gate before every push";

/// An entry file at format 11: every key format 11 wrote and none of format
/// 12's `inactive_since`, so reading it runs only the 11 -> 12 step. Its
/// `updated` is 2026-11-03T08:30:00Z.
fn format_eleven_entry(status: &str, superseded_by: &str, said: &str, body: &str) -> String {
    format!(
        "---
format: 11
kind: entry
id: preferences-1
tag: preferences
status: {status}
project: scribe
topics: [ci]
related: [\"repo:scribe\"]
provider: claude-code
host: host
author: {{kind: daniel, name: daniel}}
updated_by: null
created: 2026-11-01T10:00:00Z
updated: 2026-11-03T08:30:00Z
said: \"{said}\"
content_from: {{kind: instruction}}
cite: {{session: s-1, message: null, at: 2026-11-01T10:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
retirement_reason: null
superseded_by: {superseded_by}
review_after: null
quote: null
window: null
evidence: null
as_of: null
---
{body}",
        sha = "0".repeat(64)
    )
}

#[test]
fn a_format_eleven_proposals_document_reads_at_its_own_shape() {
    // Format 11 -> 12 changed only entries, so a format-11 proposals.json
    // reads against the current shape: each candidate's `admitted` and each
    // proposal's `closed_reason` survive, and the document encodes back to
    // the same keys and values with only its format number moved.
    let cite = serde_json::json!({
        "session": "s-1",
        "message": "m-1",
        "at": "2026-11-01T10:00:00Z",
        "prompt_sha256": "0".repeat(64),
        "line": 1,
        "cwd": "/work",
    });
    let open = serde_json::json!({
        "id": "p-1",
        "kind": "candidate",
        "older": "preferences-1",
        "candidate": {
            "tag": "preferences",
            "role": "user",
            "quote": "I prefer short updates.",
            "evidence": null,
            "cite": cite,
            "project": "scribe",
            "provider": "claude-code",
            "host": "host",
            "admitted": {
                "trigger": "remember",
                "matched": "remember that I prefer short updates",
                "criteria": "a stated preference",
            },
        },
        "newer": null,
        "reason": "Unmarked preferences statement",
        "quotes": [],
        "state": "open",
        "opened_at": "2026-11-01T10:00:00Z",
        "settled_at": null,
        "settled_by": null,
        "author": {"kind": "scribe", "name": "session-pass"},
        "updated_by": null,
        "settled_author": null,
        "closed_reason": null,
    });
    // Closed when its tag was retired: its trigger matched with no criteria.
    let mut closed = open.clone();
    closed["id"] = serde_json::json!("p-2");
    closed["older"] = serde_json::json!("idea-1");
    closed["candidate"]["tag"] = serde_json::json!("idea");
    closed["candidate"]["admitted"] =
        serde_json::json!({"trigger": "remember", "matched": "remember", "criteria": null});
    closed["state"] = serde_json::json!("rejected");
    closed["settled_at"] = serde_json::json!("2026-11-02T10:00:00Z");
    closed["settled_author"] = serde_json::json!({"kind": "scribe", "name": "tag-retired"});
    closed["closed_reason"] = serde_json::json!("tag_retired");

    let old = serde_json::json!({"format": 11, "proposals": [open, closed]});
    let path = Path::new("proposals.json");
    let proposals = decode_proposals(path, &serde_json::to_vec(&old).unwrap()).unwrap();
    assert_eq!(proposals.len(), 2);
    assert_eq!(
        proposals[0].candidate.as_ref().unwrap().admitted,
        Some(crate::Admitted {
            trigger: "remember".into(),
            matched: "remember that I prefer short updates".into(),
            criteria: Some("a stated preference".into()),
        })
    );
    assert_eq!(proposals[0].closed_reason, None);
    let closed_admitted = proposals[1]
        .candidate
        .as_ref()
        .unwrap()
        .admitted
        .as_ref()
        .unwrap();
    assert_eq!(closed_admitted.criteria, None);
    assert_eq!(
        proposals[1].closed_reason,
        Some(crate::ClosedReason::TagRetired)
    );

    // Re-encoded, the document is unchanged but for its format number, the
    // new null `instruction` of format 13 and the candidates' null
    // `statement` and `subject` of format 14.
    let encoded = crate::store::format::encode_proposals(path, &proposals).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["format"], FORMAT);
    value["format"] = serde_json::json!(11);
    for p in value["proposals"].as_array_mut().unwrap() {
        let p = p.as_object_mut().unwrap();
        assert_eq!(p.remove("instruction"), Some(serde_json::Value::Null));
        if let Some(c) = p.get_mut("candidate").and_then(|c| c.as_object_mut()) {
            assert_eq!(c.remove("statement"), Some(serde_json::Value::Null));
            assert_eq!(c.remove("context"), Some(serde_json::Value::Null));
            assert_eq!(c.remove("subject"), Some(serde_json::Value::Null));
        }
    }
    assert_eq!(value, old);

    // Strict at format 11: a candidate without `admitted` (the format-10
    // shape) and a proposal carrying a format-12 key are both refused.
    let mut format_ten = old.clone();
    format_ten["proposals"][0]["candidate"]
        .as_object_mut()
        .unwrap()
        .remove("admitted");
    assert!(decode_proposals(path, &serde_json::to_vec(&format_ten).unwrap()).is_err());
    let mut stray = old.clone();
    stray["proposals"][1]["inactive_since"] = serde_json::json!(null);
    assert!(decode_proposals(path, &serde_json::to_vec(&stray).unwrap()).is_err());
}

#[test]
fn a_format_eleven_entry_gains_inactive_since_from_its_status() {
    // Format 11 -> 12: an active entry reads `inactive_since: null`; one
    // already retired, superseded or forgotten reads its `updated`, the
    // best time an older record holds.
    let path = Path::new(ENTRY);
    let active = entry(&format_eleven_entry("active", "null", SAID, BODY)).unwrap();
    assert_eq!(active.inactive_since, None);

    let retired_at = time::macros::datetime!(2026-11-03 08:30:00 UTC);
    for (status, superseded_by, said, body) in [
        ("retired", "null", SAID, BODY),
        ("superseded", "preferences-2", SAID, BODY),
        ("forgotten", "null", "", ""),
    ] {
        let upgraded = entry(&format_eleven_entry(status, superseded_by, said, body)).unwrap();
        assert_eq!(upgraded.status.as_str(), status);
        assert_eq!(upgraded.inactive_since, Some(retired_at), "{status}");

        // Written back at format 12, it reads back the same.
        let encoded = crate::store::format::encode_entry(path, &upgraded).unwrap();
        let text = String::from_utf8(encoded).unwrap();
        assert!(text.starts_with(&format!("---\nformat: {FORMAT}\n")));
        assert_eq!(entry(&text).unwrap(), upgraded, "{status}");
    }
    let retired = entry(&format_eleven_entry("retired", "null", SAID, BODY)).unwrap();

    // At format 12 the key is required and must agree with the status.
    let current = crate::store::format::encode_entry(path, &active).unwrap();
    let current = String::from_utf8(current).unwrap();
    assert!(entry(&current.replace("inactive_since: null\n", "")).is_err());
    assert!(
        entry(&current.replace(
            "inactive_since: null",
            "inactive_since: 2026-11-02T10:00:00Z"
        ))
        .is_err(),
        "an active entry has no inactive_since"
    );
    let still_retired =
        String::from_utf8(crate::store::format::encode_entry(path, &retired).unwrap()).unwrap();
    assert!(
        entry(&still_retired.replace(
            "inactive_since: 2026-11-03T08:30:00Z",
            "inactive_since: null"
        ))
        .is_err(),
        "a retired entry names when it left active"
    );
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
    let daniel = crate::record::Author::user();
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
    assert_eq!(back.author.to_string(), "user:user");
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
    assert_eq!(e.author, crate::record::Author::user());
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
    assert_eq!(list[1].settled_author, Some(crate::record::Author::user()));

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
    let sha = "0".repeat(64);
    let old_entry = format!(
        "---
format: 11
kind: entry
id: preferences-1
tag: preferences
status: active
project: scribe
topics: [ci]
related: [\"repo:scribe\"]
provider: claude-code
host: host
author: {{kind: daniel, name: daniel}}
updated_by: null
created: 2026-11-01T10:00:00Z
updated: 2026-11-01T10:00:00Z
said: \"#scribe preferences: run the gate\"
content_from: {{kind: instruction}}
cite: {{session: s-1, message: null, at: 2026-11-01T10:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
retirement_reason: null
superseded_by: null
review_after: null
quote: null
window: null
evidence: null
as_of: null
---
run the gate before every push
"
    );
    let decoded_entry = entry(&old_entry).unwrap();
    assert_eq!(decoded_entry.approved_by, None);
    assert_eq!(decoded_entry.instruction, None);
    assert_eq!(decoded_entry.content, "run the gate before every push");

    // Re-encoded at the current format, the entry differs only by
    // `format`, `inactive_since: null` (format 12), `approved_by: null`,
    // `instruction: null` (format 13) and `stated_by: null` (format 14).
    let encoded_entry = String::from_utf8(
        crate::store::format::encode_entry(Path::new(ENTRY), &decoded_entry).unwrap(),
    )
    .unwrap();
    assert!(encoded_entry.starts_with(&format!("---\nformat: {FORMAT}\n")));
    assert!(encoded_entry.contains("\napproved_by: null\n"));
    assert!(encoded_entry.contains("\ninstruction: null\n"));
    let stripped_entry = encoded_entry
        .replacen(&format!("format: {FORMAT}\n"), "format: 11\n", 1)
        .replace("inactive_since: null\n", "")
        .replace("approved_by: null\n", "")
        .replace("removal_instruction: null\n", "")
        .replace("instruction: null\n", "")
        .replace("stated_by: null\n", "")
        .replace("outcome: null\n", "");
    assert_eq!(entry(&stripped_entry).unwrap(), decoded_entry);

    // Format-11 entries must reject later keys.
    for stray in [
        old_entry.replace("as_of: null\n", "as_of: null\ninactive_since: null\n"),
        old_entry.replace("as_of: null\n", "as_of: null\napproved_by: null\n"),
        old_entry.replace("as_of: null\n", "as_of: null\ninstruction: null\n"),
    ] {
        assert!(entry(&stray).is_err(), "{stray}");
    }

    let cite = format!(
        r#"{{"session":"s-1","message":"m-1","at":"2026-11-01T10:00:00Z","prompt_sha256":"{sha}","line":1,"cwd":"/work"}}"#
    );
    let open_candidate = format!(
        r#"{{"id":"p-1","kind":"candidate","older":"preferences-1","candidate":{{"tag":"preferences","role":"user","quote":"I prefer short updates.","evidence":null,"cite":{cite},"project":"scribe","provider":"claude-code","host":"host","admitted":{{"trigger":"I prefer","matched":"I prefer","criteria":null}}}},"newer":null,"reason":"Unmarked preferences statement","quotes":[],"state":"open","opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null,"author":{{"kind":"scribe","name":"session-pass"}},"updated_by":null,"settled_author":null,"closed_reason":null}}"#
    );
    let retired_candidate = format!(
        r#"{{"id":"p-2","kind":"candidate","older":"decisions-1","candidate":{{"tag":"decisions","role":"user","quote":"I always choose merge commits.","evidence":null,"cite":{cite},"project":"scribe","provider":"claude-code","host":"host","admitted":null}},"newer":null,"reason":"Unmarked decisions statement","quotes":[],"state":"rejected","opened_at":"2026-11-01T10:00:00Z","settled_at":"2026-11-02T10:00:00Z","settled_by":null,"author":{{"kind":"scribe","name":"session-pass"}},"updated_by":null,"settled_author":{{"kind":"scribe","name":"tag-retired"}},"closed_reason":"tag_retired"}}"#
    );
    let old_proposals =
        format!(r#"{{"format":11,"proposals":[{open_candidate},{retired_candidate}]}}"#);
    let path = Path::new("proposals.json");
    let proposals = decode_proposals(path, old_proposals.as_bytes()).unwrap();
    assert_eq!(proposals.len(), 2);
    assert!(proposals.iter().all(|p| p.instruction.is_none()));
    assert_eq!(proposals[0].closed_reason, None);
    assert_eq!(
        proposals[1].closed_reason,
        Some(crate::ClosedReason::TagRetired)
    );

    let encoded_proposals = crate::store::format::encode_proposals(path, &proposals).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&encoded_proposals).unwrap();
    assert_eq!(value["format"], FORMAT);
    for p in value["proposals"].as_array_mut().unwrap() {
        let p = p.as_object_mut().unwrap();
        assert_eq!(p.remove("instruction"), Some(serde_json::Value::Null));
        if let Some(c) = p.get_mut("candidate").and_then(|c| c.as_object_mut()) {
            assert_eq!(c.remove("statement"), Some(serde_json::Value::Null));
            assert_eq!(c.remove("context"), Some(serde_json::Value::Null));
            assert_eq!(c.remove("subject"), Some(serde_json::Value::Null));
        }
    }
    value["format"] = serde_json::json!(11);
    let original: serde_json::Value = serde_json::from_str(&old_proposals).unwrap();
    assert_eq!(value, original);

    // Format-11 proposals must reject format-13 keys/values and frozen-candidate additions.
    let stray_inst = old_proposals.replace(
        r#""closed_reason":null}"#,
        r#""closed_reason":null,"instruction":null}"#,
    );
    assert!(decode_proposals(path, stray_inst.as_bytes()).is_err());
    let stray_reason = old_proposals.replace(
        r#""closed_reason":"tag_retired""#,
        r#""closed_reason":"instructed""#,
    );
    assert!(decode_proposals(path, stray_reason.as_bytes()).is_err());
    let stray_candidate = old_proposals.replace(
        r#""admitted":null}"#,
        r#""admitted":null,"statement":"extra"}"#,
    );
    assert!(decode_proposals(path, stray_candidate.as_bytes()).is_err());
}

#[test]
fn a_format_eleven_entry_and_session_with_local_yaml_tags_read_after_upgrade() {
    let sha = "0".repeat(64);
    let tagged_entry = format!(
        "---
format: 11
kind: entry
id: preferences-1
tag: preferences
status: active
project: !local scribe
topics: [ci]
related: [\"repo:scribe\"]
provider: claude-code
host: host
author: {{kind: daniel, name: daniel}}
updated_by: null
created: 2026-11-01T10:00:00Z
updated: 2026-11-01T10:00:00Z
said: !local \"#scribe preferences: run the gate\"
content_from: {{kind: instruction}}
cite: {{session: s-1, message: null, at: 2026-11-01T10:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
retirement_reason: null
superseded_by: null
review_after: null
quote: null
window: null
evidence: null
as_of: null
---
run the gate before every push
"
    );
    let e = entry(&tagged_entry).unwrap();
    assert_eq!(e.project.as_str(), "scribe");
    assert_eq!(e.said, "#scribe preferences: run the gate");
    assert_eq!(e.approved_by, None);
    assert_eq!(e.instruction, None);

    let tagged_session = two_segments()
        .replacen(&format!("format: {FORMAT}\n"), "format: 11\n", 1)
        .replacen("project: scribe\n", "project: !local scribe\n", 1)
        .replacen("title: a session\n", "title: !local a session\n", 1);
    let s = session(&tagged_session).unwrap();
    assert_eq!(s.project.as_str(), "scribe");
    assert_eq!(s.title, "a session");
    assert_eq!(s.log.len(), 2);
}

#[test]
fn instructed_entry_fields_round_trip_and_refuse_contradictions() {
    let mut e = entry(&entry_text("")).unwrap();
    let inst = crate::Instruction {
        session: "s-9".parse().unwrap(),
        message: String::from("m-9"),
        at: time::macros::datetime!(2026-11-02 09:00:00 UTC),
        quote: Some(String::from("approve p-1")),
    };
    e.approved_by = Some(crate::Author::agent("aide".parse().unwrap()));
    e.instruction = Some(inst.clone());

    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &e).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(entry(&text).unwrap(), e);

    // Forgotten instructed entry: quote cleared to null while metadata stays.
    let mut forgotten = e.clone();
    forgotten.status = crate::EntryStatus::Forgotten;
    forgotten.inactive_since = Some(time::macros::datetime!(2026-11-03 08:30:00 UTC));
    forgotten.said.clear();
    forgotten.content.clear();
    forgotten.instruction.as_mut().unwrap().quote = None;
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &forgotten).unwrap();
    assert_eq!(
        entry(&String::from_utf8(bytes).unwrap()).unwrap(),
        forgotten
    );

    // `approved_by` with `instruction: null` is refused.
    let mut no_inst = e.clone();
    no_inst.instruction = None;
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &no_inst).unwrap();
    corrupt_message(&entry(&String::from_utf8(bytes).unwrap()).unwrap_err());

    // Non-agent `approved_by` is refused.
    let mut daniel_approved = e;
    daniel_approved.approved_by = Some(crate::Author::user());
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &daniel_approved).unwrap();
    corrupt_message(&entry(&String::from_utf8(bytes).unwrap()).unwrap_err());

    // Direct agent entry with `approved_by` is refused.
    let direct_with_approved = format!(
        "---
format: {FORMAT}
kind: entry
id: preferences-1
tag: preferences
status: active
project: scribe
topics: []
related: []
provider: cli
host: host
author: {{kind: agent, name: opus}}
updated_by: null
created: 2026-11-01T10:00:00Z
updated: 2026-11-01T10:00:00Z
said: \"\"
content_from: {{kind: agent}}
cite: null
retirement_reason: null
superseded_by: null
review_after: null
quote: null
window: null
evidence: null
as_of: null
approved_by: {{kind: agent, name: aide}}
instruction: {{session: s-9, message: m-9, at: 2026-11-02T09:00:00Z, quote: \"approve p-1\"}}
---
direct agent text
"
    );
    corrupt_message(&entry(&direct_with_approved).unwrap_err());
}

#[test]
fn closed_reason_accepts_instructed_and_older_values_read_unchanged() {
    let sha = "0".repeat(64);
    let inst_cite = format!(
        r#"{{"session":"s-9","message":"m-9","at":"2026-11-03T10:00:00Z","prompt_sha256":"{sha}","line":1,"cwd":"/work"}}"#
    );
    let cand_cite = format!(
        r#"{{"session":"s-1","message":"m-1","at":"2026-11-01T10:00:00Z","prompt_sha256":"{sha}","line":1,"cwd":"/work"}}"#
    );
    let doc = format!(
        r#"{{
            "format": {FORMAT},
            "proposals": [
                {{
                    "id": "p-1",
                    "kind": "candidate",
                    "older": "preferences-1",
                    "candidate": {{"tag":"preferences","role":"user","quote":"I prefer tabs.","evidence":null,"cite":{cand_cite},"project":"scribe","provider":"claude-code","host":"host","admitted":null,"context":null,"statement":null,"subject":null}},
                    "newer": null,
                    "reason": "Unmarked preferences statement",
                    "quotes": [],
                    "state": "rejected",
                    "opened_at": "2026-11-01T10:00:00Z",
                    "settled_at": "2026-11-03T10:00:00Z",
                    "settled_by": {inst_cite},
                    "author": {{"kind":"scribe","name":"session-pass"}},
                    "updated_by": null,
                    "settled_author": {{"kind":"agent","name":"aide"}},
                    "closed_reason": "instructed",
                    "instruction": {{"session":"s-9","message":"m-9","at":"2026-11-03T10:00:00Z","quote":"reject p-1"}}
                }},
                {{
                    "id": "p-2",
                    "kind": "candidate",
                    "older": "preferences-2",
                    "candidate": {{"tag":"preferences","role":"user","quote":"I prefer short lines.","evidence":null,"cite":{cand_cite},"project":"scribe","provider":"claude-code","host":"host","admitted":null,"context":null,"statement":null,"subject":null}},
                    "newer": null,
                    "reason": "Unmarked preferences statement",
                    "quotes": [],
                    "state": "approved",
                    "opened_at": "2026-11-01T10:00:00Z",
                    "settled_at": "2026-11-03T10:05:00Z",
                    "settled_by": {inst_cite},
                    "author": {{"kind":"scribe","name":"session-pass"}},
                    "updated_by": null,
                    "settled_author": {{"kind":"agent","name":"aide"}},
                    "closed_reason": null,
                    "instruction": {{"session":"s-9","message":"m-9","at":"2026-11-03T10:00:00Z","quote":"approve p-2"}}
                }},
                {{
                    "id": "p-3",
                    "kind": "candidate",
                    "older": "decisions-1",
                    "candidate": {{"tag":"decisions","role":"user","quote":"I prefer squash.","evidence":null,"cite":{cand_cite},"project":"scribe","provider":"claude-code","host":"host","admitted":null,"context":null,"statement":null,"subject":null}},
                    "newer": null,
                    "reason": "Unmarked decisions statement",
                    "quotes": [],
                    "state": "rejected",
                    "opened_at": "2026-11-01T10:00:00Z",
                    "settled_at": "2026-11-02T10:00:00Z",
                    "settled_by": null,
                    "author": {{"kind":"scribe","name":"session-pass"}},
                    "updated_by": null,
                    "settled_author": {{"kind":"scribe","name":"tag-retired"}},
                    "closed_reason": "tag_retired",
                    "instruction": null
                }},
                {{
                    "id": "p-4",
                    "kind": "stale",
                    "older": "preferences-3",
                    "candidate": null,
                    "newer": null,
                    "reason": "review",
                    "quotes": [],
                    "state": "rejected",
                    "opened_at": "2026-11-01T10:00:00Z",
                    "settled_at": "2026-11-02T10:00:00Z",
                    "settled_by": null,
                    "author": {{"kind":"scribe","name":"consistency"}},
                    "updated_by": null,
                    "settled_author": {{"kind":"daniel","name":"daniel"}},
                    "closed_reason": null,
                    "instruction": null
                }}
            ]
        }}"#
    );
    let path = Path::new("proposals.json");
    let proposals = decode_proposals(path, doc.as_bytes()).unwrap();
    assert_eq!(proposals.len(), 4);
    assert_eq!(
        proposals[0].closed_reason,
        Some(crate::ClosedReason::Instructed)
    );
    assert_eq!(
        proposals[0].instruction.as_ref().unwrap().quote.as_deref(),
        Some("reject p-1")
    );
    assert_eq!(proposals[1].closed_reason, None);
    assert_eq!(
        proposals[1].instruction.as_ref().unwrap().quote.as_deref(),
        Some("approve p-2")
    );
    assert_eq!(
        proposals[2].closed_reason,
        Some(crate::ClosedReason::TagRetired)
    );
    assert_eq!(proposals[2].instruction, None);
    assert_eq!(proposals[3].closed_reason, None);
    assert_eq!(proposals[3].instruction, None);
}

#[test]
fn contradictory_proposal_instruction_metadata_is_refused() {
    let sha = "0".repeat(64);
    let path = Path::new("proposals.json");
    let inst_cite = serde_json::json!({
        "session": "s-9",
        "message": "m-9",
        "at": "2026-11-03T10:00:00Z",
        "prompt_sha256": sha,
        "line": 1,
        "cwd": "/work"
    });
    let cand_cite = serde_json::json!({
        "session": "s-1",
        "message": "m-1",
        "at": "2026-11-01T10:00:00Z",
        "prompt_sha256": sha,
        "line": 1,
        "cwd": "/work"
    });
    let base_candidate = || {
        serde_json::json!({
            "id": "p-1",
            "kind": "candidate",
            "older": "preferences-1",
            "candidate": {
                "tag": "preferences",
                "role": "user",
                "quote": "I prefer tabs.",
                "evidence": null,
                "cite": cand_cite,
                "project": "scribe",
                "provider": "claude-code",
                "host": "host",
                "admitted": null,
                "statement": null,
                "subject": null
            },
            "newer": null,
            "reason": "Unmarked preferences statement",
            "quotes": [],
            "state": "rejected",
            "opened_at": "2026-11-01T10:00:00Z",
            "settled_at": "2026-11-03T10:00:00Z",
            "settled_by": inst_cite,
            "author": {"kind": "scribe", "name": "session-pass"},
            "updated_by": null,
            "settled_author": {"kind": "agent", "name": "aide"},
            "closed_reason": "instructed",
            "instruction": {
                "session": "s-9",
                "message": "m-9",
                "at": "2026-11-03T10:00:00Z",
                "quote": "reject p-1"
            }
        })
    };

    let mut open_with_inst = base_candidate();
    open_with_inst["state"] = serde_json::json!("open");
    open_with_inst["closed_reason"] = serde_json::Value::Null;
    open_with_inst["settled_at"] = serde_json::Value::Null;

    let mut no_settled_author = base_candidate();
    no_settled_author["settled_author"] = serde_json::Value::Null;

    let mut daniel_settled_author = base_candidate();
    daniel_settled_author["settled_author"] =
        serde_json::json!({"kind": "daniel", "name": "daniel"});

    let mut no_instruction = base_candidate();
    no_instruction["instruction"] = serde_json::Value::Null;

    let mut no_settled_by = base_candidate();
    no_settled_by["settled_by"] = serde_json::Value::Null;

    let mut mismatched_message = base_candidate();
    mismatched_message["settled_by"]["message"] = serde_json::json!("m-other");

    let mut mismatched_session = base_candidate();
    mismatched_session["settled_by"]["session"] = serde_json::json!("s-other");

    let mut mismatched_at = base_candidate();
    mismatched_at["settled_by"]["at"] = serde_json::json!("2026-11-03T10:00:01Z");

    let mut rejected_inst_without_closed_reason = base_candidate();
    rejected_inst_without_closed_reason["closed_reason"] = serde_json::Value::Null;

    let mut instructed_approved = base_candidate();
    instructed_approved["state"] = serde_json::json!("approved");

    let stale_with_inst = serde_json::json!({
        "id": "p-2",
        "kind": "stale",
        "older": "preferences-1",
        "candidate": null,
        "newer": null,
        "reason": "review",
        "quotes": [],
        "state": "rejected",
        "opened_at": "2026-11-01T10:00:00Z",
        "settled_at": "2026-11-03T10:00:00Z",
        "settled_by": inst_cite,
        "author": {"kind": "scribe", "name": "consistency"},
        "updated_by": null,
        "settled_author": {"kind": "agent", "name": "aide"},
        "closed_reason": "instructed",
        "instruction": {
            "session": "s-9",
            "message": "m-9",
            "at": "2026-11-03T10:00:00Z",
            "quote": "reject p-2"
        }
    });

    for (label, bad) in [
        ("open_with_inst", open_with_inst),
        ("no_settled_author", no_settled_author),
        ("daniel_settled_author", daniel_settled_author),
        ("no_instruction", no_instruction),
        ("no_settled_by", no_settled_by),
        ("mismatched_message", mismatched_message),
        ("mismatched_session", mismatched_session),
        ("mismatched_at", mismatched_at),
        (
            "rejected_inst_without_closed_reason",
            rejected_inst_without_closed_reason,
        ),
        ("instructed_approved", instructed_approved),
        ("stale_with_inst", stale_with_inst),
    ] {
        let doc = serde_json::json!({"format": FORMAT, "proposals": [bad]}).to_string();
        let err = decode_proposals(path, doc.as_bytes()).unwrap_err();
        corrupt_message(&err);
        let _ = label;
    }
}

fn format_twelve_entry(extra: &str, project: &str) -> String {
    format!(
        "---
format: 12
kind: entry
id: preferences-1
tag: preferences
status: retired
project: {project}
topics: []
related: []
provider: claude-code
host: host
author: {{kind: daniel, name: daniel}}
updated_by: null
created: 2026-11-01T10:00:00Z
updated: 2026-11-03T08:30:00Z
{extra}said: \"#scribe preferences: run the gate\"
content_from: {{kind: instruction}}
cite: {{session: s-1, message: null, at: 2026-11-01T10:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
retirement_reason: null
superseded_by: null
review_after: null
quote: null
window: null
evidence: null
as_of: null
---
run the gate
",
        sha = "0".repeat(64)
    )
}

#[test]
fn a_format_twelve_entry_upgrades_with_its_inactive_since_and_null_instruction_fields() {
    let doc = format_twelve_entry("inactive_since: 2026-11-02T07:00:00Z\n", "scribe");
    let e = entry(&doc).unwrap();
    assert_eq!(
        e.inactive_since,
        Some(time::macros::datetime!(2026-11-02 07:00:00 UTC)),
        "format 12 keeps its own inactive_since, not `updated`"
    );
    assert_eq!((&e.approved_by, &e.instruction), (&None, &None));

    // Local YAML tags on a format-12 string survive the upgrade.
    let tagged = format_twelve_entry("inactive_since: 2026-11-02T07:00:00Z\n", "!local scribe");
    assert_eq!(entry(&tagged).unwrap().project.as_str(), "scribe");

    // Strict at format 12: the key is required, and the format-13 keys are
    // unknown there.
    let missing = format_twelve_entry("", "scribe");
    corrupt_message(&entry(&missing).unwrap_err());
    for stray in ["approved_by: null\n", "instruction: null\n"] {
        let doc = format_twelve_entry(
            &format!("inactive_since: 2026-11-02T07:00:00Z\n{stray}"),
            "scribe",
        );
        corrupt_message(&entry(&doc).unwrap_err());
    }
}

#[test]
fn legacy_tag_retired_on_a_non_candidate_rejected_proposal_stays_readable() {
    // The format-11 reader accepted `closed_reason: tag_retired` on any
    // rejected proposal; only `instructed` is limited to candidates.
    let stale = |format: u32| {
        format!(
            r#"{{"format":{format},"proposals":[{{"id":"p-1","kind":"stale","older":"preferences-1","candidate":null,"newer":null,"reason":"review","quotes":[],"state":"rejected","opened_at":"2026-11-01T10:00:00Z","settled_at":"2026-11-02T10:00:00Z","settled_by":null,"author":{{"kind":"scribe","name":"consistency"}},"updated_by":null,"settled_author":{{"kind":"scribe","name":"tag-retired"}},"closed_reason":"tag_retired"}}]}}"#
        )
    };
    let path = Path::new("proposals.json");
    for format in [11, 12] {
        let proposals = decode_proposals(path, stale(format).as_bytes()).unwrap();
        assert_eq!(
            proposals[0].closed_reason,
            Some(crate::ClosedReason::TagRetired),
            "format {format}"
        );
        // The format-13 rewrite keeps it readable.
        let encoded = crate::store::format::encode_proposals(path, &proposals).unwrap();
        let again = decode_proposals(path, &encoded).unwrap();
        assert_eq!(again, proposals, "format {format}");
    }
}

#[test]
fn instruction_metadata_requires_settled_at() {
    let sha = "0".repeat(64);
    let cite = |session: &str, message: &str, at: &str| {
        serde_json::json!({"session": session, "message": message, "at": at,
            "prompt_sha256": sha, "line": 1, "cwd": "/work"})
    };
    let proposal = |settled_at: serde_json::Value| {
        serde_json::json!({
            "id": "p-1", "kind": "candidate", "older": "preferences-1",
            "candidate": {"tag": "preferences", "role": "user", "quote": "I prefer tabs.",
                "evidence": null, "cite": cite("s-1", "m-1", "2026-11-01T10:00:00Z"),
                "project": "scribe", "provider": "claude-code", "host": "host",
                "admitted": null, "context": null, "statement": null, "subject": null},
            "newer": null, "reason": "Unmarked preferences statement", "quotes": [],
            "state": "rejected", "opened_at": "2026-11-01T10:00:00Z",
            "settled_at": settled_at,
            "settled_by": cite("s-9", "m-9", "2026-11-03T10:00:00Z"),
            "author": {"kind": "scribe", "name": "session-pass"}, "updated_by": null,
            "settled_author": {"kind": "agent", "name": "aide"},
            "closed_reason": "instructed",
            "instruction": {"session": "s-9", "message": "m-9",
                "at": "2026-11-03T10:00:00Z", "quote": "reject p-1"}
        })
    };
    let path = Path::new("proposals.json");
    let read = |p: serde_json::Value| {
        let doc = serde_json::json!({"format": FORMAT, "proposals": [p]}).to_string();
        decode_proposals(path, doc.as_bytes())
    };
    read(proposal(serde_json::json!("2026-11-03T10:00:00Z"))).unwrap();
    let message = corrupt_message(&read(proposal(serde_json::Value::Null)).unwrap_err());
    assert!(message.contains("settled_at"), "{message}");
}

fn format_thirteen_entry(extra: &str, project: &str) -> String {
    format!(
        "---
format: 13
kind: entry
id: preferences-1
tag: preferences
status: active
project: {project}
topics: [ci]
related: []
provider: claude-code
host: host
author: {{kind: daniel, name: daniel}}
updated_by: null
created: 2026-11-01T10:00:00Z
updated: 2026-11-01T10:00:00Z
inactive_since: null
said: \"#scribe approve p-1\"
content_from: {{kind: message, role: user, message: m-1, at: 2026-11-01T09:59:00Z}}
cite: {{session: s-1, message: m-1, at: 2026-11-01T09:59:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
retirement_reason: null
superseded_by: null
review_after: null
quote: null
window: null
evidence: null
as_of: null
approved_by: null
{extra}instruction: null
---
I prefer short updates.
",
        sha = "0".repeat(64)
    )
}

#[test]
fn a_format_thirteen_store_upgrades_with_null_statement_subject_and_stated_by_only() {
    // Entry: checked against the frozen format-13 shape, then `stated_by`
    // is null and nothing else changes.
    let old_entry = format_thirteen_entry("", "scribe");
    let decoded = entry(&old_entry).unwrap();
    assert_eq!(decoded.stated_by, None);
    assert_eq!(decoded.content, "I prefer short updates.");
    let encoded =
        String::from_utf8(crate::store::format::encode_entry(Path::new(ENTRY), &decoded).unwrap())
            .unwrap();
    assert!(encoded.starts_with(&format!("---\nformat: {FORMAT}\n")));
    assert!(encoded.contains("\nstated_by: null\n"), "{encoded}");
    let back = encoded
        .replacen(&format!("format: {FORMAT}\n"), "format: 13\n", 1)
        .replace("stated_by: null\n", "")
        .replace("outcome: null\n", "")
        .replace("removal_instruction: null\n", "");
    assert_eq!(entry(&back).unwrap(), decoded);
    // Local YAML tags on a format-13 string survive the upgrade.
    let tagged = format_thirteen_entry("", "!local scribe");
    assert_eq!(entry(&tagged).unwrap().project.as_str(), "scribe");
    // Strict at format 13: `stated_by` is a format-14 key.
    let stray = format_thirteen_entry("stated_by: null\n", "scribe");
    corrupt_message(&entry(&stray).unwrap_err());

    // Proposals: each candidate gains null `statement` and `subject`.
    let cite = format!(
        r#"{{"session":"s-1","message":"m-1","at":"2026-11-01T10:00:00Z","prompt_sha256":"{}","line":1,"cwd":"/work"}}"#,
        "0".repeat(64)
    );
    let inst_cite = r#"{"session":"s-9","message":"m-9","at":"2026-11-03T10:00:00Z","prompt_sha256":"1111111111111111111111111111111111111111111111111111111111111111","line":1,"cwd":"/work"}"#;
    let open = format!(
        r#"{{"id":"p-1","kind":"candidate","older":"preferences-1","candidate":{{"tag":"preferences","role":"user","quote":"I prefer short updates.","evidence":null,"cite":{cite},"project":"scribe","provider":"claude-code","host":"host","admitted":{{"trigger":"I prefer","matched":"I prefer","criteria":null}}}},"newer":null,"reason":"Unmarked preferences statement","quotes":[],"state":"open","opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null,"author":{{"kind":"scribe","name":"session-pass"}},"updated_by":null,"settled_author":null,"closed_reason":null,"instruction":null}}"#
    );
    let instructed = format!(
        r#"{{"id":"p-2","kind":"candidate","older":"preferences-2","candidate":{{"tag":"preferences","role":"user","quote":"I prefer tabs.","evidence":null,"cite":{cite},"project":"scribe","provider":"claude-code","host":"host","admitted":null}},"newer":null,"reason":"Unmarked preferences statement","quotes":[],"state":"rejected","opened_at":"2026-11-01T10:00:00Z","settled_at":"2026-11-03T10:00:00Z","settled_by":{inst_cite},"author":{{"kind":"scribe","name":"session-pass"}},"updated_by":null,"settled_author":{{"kind":"agent","name":"aide"}},"closed_reason":"instructed","instruction":{{"session":"s-9","message":"m-9","at":"2026-11-03T10:00:00Z","quote":"reject p-2"}}}}"#
    );
    let stale = r#"{"id":"p-3","kind":"stale","older":"preferences-3","candidate":null,"newer":null,"reason":"review","quotes":[],"state":"open","opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null,"author":{"kind":"scribe","name":"consistency"},"updated_by":null,"settled_author":null,"closed_reason":null,"instruction":null}"#;
    let old = format!(r#"{{"format":13,"proposals":[{open},{instructed},{stale}]}}"#);
    let path = Path::new("proposals.json");
    let proposals = decode_proposals(path, old.as_bytes()).unwrap();
    assert_eq!(proposals.len(), 3);
    for candidate in proposals.iter().filter_map(|p| p.candidate.as_ref()) {
        assert_eq!((&candidate.statement, &candidate.subject), (&None, &None));
    }
    let encoded = crate::store::format::encode_proposals(path, &proposals).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["format"], FORMAT);
    value["format"] = serde_json::json!(13);
    for p in value["proposals"].as_array_mut().unwrap() {
        if let Some(c) = p.get_mut("candidate").and_then(|c| c.as_object_mut()) {
            assert_eq!(c.remove("statement"), Some(serde_json::Value::Null));
            assert_eq!(c.remove("context"), Some(serde_json::Value::Null));
            assert_eq!(c.remove("subject"), Some(serde_json::Value::Null));
        }
    }
    let original: serde_json::Value = serde_json::from_str(&old).unwrap();
    assert_eq!(value, original, "no other change");
    // Strict at format 13: a candidate carrying a format-14 key is refused.
    for key in ["statement", "subject"] {
        let stray = old.replacen(
            r#""admitted":null}"#,
            &format!(r#""admitted":null,"{key}":null}}"#),
            1,
        );
        corrupt_message(&decode_proposals(path, stray.as_bytes()).unwrap_err());
    }
}

/// A format-14 entry as a format-14 build wrote it, `project` spelled
/// `project` (`!local scribe` is how a hand edit or another YAML writer may
/// tag it, which a format-14 build reads as `scribe`).
fn format_fourteen_entry(project: &str) -> String {
    format_thirteen_entry("", project)
        .replacen("format: 13\n", "format: 14\n", 1)
        .replacen(
            "instruction: null\n",
            "instruction: null\nstated_by: null\n",
            1,
        )
}

#[test]
fn a_format_fourteen_entry_with_local_yaml_tags_reads_as_under_a_format_fourteen_build() {
    // The 14 -> 15 step changes no stored value, so a format-14 entry
    // decodes in typed YAML as before: a local tag stays a string.
    let plain = entry(&format_fourteen_entry("scribe")).unwrap();
    let tagged = entry(&format_fourteen_entry("!local scribe")).unwrap();
    assert_eq!(tagged, plain);
    assert_eq!(tagged.project.as_str(), "scribe");
    assert_eq!(tagged.content, "I prefer short updates.");
    // Strict at format 14: a later key is refused.
    let stray = format_fourteen_entry("scribe").replacen(
        "stated_by: null\n",
        "stated_by: null\nremoval_instruction: null\n",
        1,
    );
    corrupt_message(&entry(&stray).unwrap_err());
}

#[test]
fn an_older_entry_keeps_an_approval_instruction_and_moves_a_removal_one() {
    let inst =
        "{session: s-9, message: m-9, at: 2026-11-03T10:00:00Z, quote: retire preferences-1}";
    // Daniel's entry retired on his instruction: through format 17 the one
    // `instruction` key held the retirement's, which now has its own key.
    let retired = format_fourteen_entry("!local scribe")
        .replacen("format: 14\n", "format: 15\n", 1)
        .replacen("status: active\n", "status: retired\n", 1)
        .replacen(
            "inactive_since: null\n",
            "inactive_since: 2026-11-03T10:05:00Z\n",
            1,
        )
        .replacen("instruction: null\n", &format!("instruction: {inst}\n"), 1);
    let decoded = entry(&retired).unwrap();
    assert_eq!(decoded.project.as_str(), "scribe");
    assert_eq!(decoded.instruction, None);
    let removal = decoded.removal_instruction.clone().unwrap();
    assert_eq!(
        (removal.message.as_str(), removal.quote.as_deref()),
        ("m-9", Some("retire preferences-1"))
    );
    let encoded =
        String::from_utf8(crate::store::format::encode_entry(Path::new(ENTRY), &decoded).unwrap())
            .unwrap();
    assert!(encoded.contains("\ninstruction: null\n"), "{encoded}");
    assert_eq!(entry(&encoded).unwrap(), decoded);
    // The JSON route of older formats splits the same way: the 17 -> 18
    // step (`UPGRADES[n - 1]` upgrades format n).
    let step = UPGRADES[17 - 1];
    let moved = step(
        crate::store::format::Document::Entry,
        serde_json::json!({"approved_by": null, "instruction": {"message": "m-9"}}),
    )
    .unwrap();
    assert_eq!(
        moved,
        serde_json::json!({"approved_by": null, "instruction": null,
            "removal_instruction": {"message": "m-9"}})
    );
    let kept = step(
        crate::store::format::Document::Entry,
        serde_json::json!({"approved_by": {"kind": "agent"}, "instruction": {"message": "m-9"}}),
    )
    .unwrap();
    assert_eq!(kept["instruction"]["message"], "m-9");
    assert_eq!(kept["removal_instruction"], serde_json::Value::Null);

    // An instructed approval keeps its `instruction` as the approval's, at
    // format 15 (no `outcome`) and at formats 16 and 17 (the same entry shape).
    for role in ["user", "assistant"] {
        let approved = instructed_approval(role).candidate_entry(None).unwrap();
        let text = String::from_utf8(
            crate::store::format::encode_entry(Path::new(ENTRY), &approved).unwrap(),
        )
        .unwrap();
        // Formats 19 to 21 changed no entry: a format-18 to -20 entry has
        // the current shape.
        for from in [18, 19, 20] {
            let older = text.replacen(
                &format!("format: {FORMAT}\n"),
                &format!("format: {from}\n"),
                1,
            );
            assert_eq!(entry(&older).unwrap(), approved, "{role} {from}");
        }
        let v17 = text
            .replacen(&format!("format: {FORMAT}\n"), "format: 17\n", 1)
            .replacen("removal_instruction: null\n", "", 1);
        assert_eq!(entry(&v17).unwrap(), approved, "{role}");
        let v16 = v17.replacen("format: 17\n", "format: 16\n", 1);
        assert_eq!(entry(&v16).unwrap(), approved, "{role}");
        let v15 =
            v16.replacen("format: 16\n", "format: 15\n", 1)
                .replacen("outcome: null\n", "", 1);
        assert_eq!(entry(&v15).unwrap(), approved, "{role}");
        // Strict at formats 16 and 17: the format-18 key is refused.
        for from in [16, 17] {
            let stray = text.replacen(
                &format!("format: {FORMAT}\n"),
                &format!("format: {from}\n"),
                1,
            );
            corrupt_message(&entry(&stray).unwrap_err());
        }
    }

    // Strict at format 18: an `instruction` without `approved_by`, or a
    // removal on an active entry, is refused.
    let mut stray = decoded.clone();
    stray.instruction = stray.removal_instruction.clone();
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &stray).unwrap();
    let message = corrupt_message(&entry(&String::from_utf8(bytes).unwrap()).unwrap_err());
    assert!(message.contains("approved_by"), "{message}");
    let mut active = decoded;
    active.status = crate::EntryStatus::Active;
    active.inactive_since = None;
    let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &active).unwrap();
    let message = corrupt_message(&entry(&String::from_utf8(bytes).unwrap()).unwrap_err());
    assert!(message.contains("removal_instruction"), "{message}");
}

/// A current-format entry holding scribe's statement of Daniel's turn.
fn stated_entry(status: &str, quote: &str, said: &str, stated_by: &str, body: &str) -> String {
    let inactive = if status == "active" {
        "null"
    } else {
        "2026-11-02T10:00:00Z"
    };
    format!(
        "---
format: {FORMAT}
kind: entry
id: preferences-1
tag: preferences
status: {status}
project: scribe
topics: []
related: []
provider: claude-code
host: host
author: {{kind: daniel, name: daniel}}
updated_by: null
created: 2026-11-01T10:00:00Z
updated: 2026-11-02T10:00:00Z
inactive_since: {inactive}
said: \"{said}\"
content_from: {{kind: message, role: user, message: m-1, at: 2026-11-01T09:59:00Z}}
cite: {{session: s-1, message: m-1, at: 2026-11-01T09:59:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
retirement_reason: null
superseded_by: null
review_after: null
quote: {quote}
window: null
evidence: null
as_of: null
approved_by: null
instruction: null
removal_instruction: null
stated_by: {stated_by}
outcome: null
---
{body}
",
        sha = "0".repeat(64)
    )
}

#[test]
fn a_stated_entry_is_daniels_with_his_quote_and_contradictions_are_refused() {
    let pass = "{kind: scribe, name: session-pass}";
    let statement = "Daniel prefers short progress updates while an agent works.";
    let ok = stated_entry(
        "active",
        "\"ok, short updates please\"",
        "#scribe approve p-1",
        pass,
        statement,
    );
    let e = entry(&ok).unwrap();
    assert_eq!(e.content, statement);
    assert_eq!(e.quote.as_deref(), Some("ok, short updates please"));
    assert_eq!(e.stated_by, Some(crate::Author::session_pass()));
    assert_eq!(e.author, crate::Author::user());
    let encoded = crate::store::format::encode_entry(Path::new(ENTRY), &e).unwrap();
    assert_eq!(entry(std::str::from_utf8(&encoded).unwrap()).unwrap(), e);
    // Forgotten: blank, its quote cleared, `stated_by` kept.
    let forgotten = stated_entry("forgotten", "null", "", pass, "");
    assert_eq!(entry(&forgotten).unwrap().quote, None);

    for (label, bad) in [
        (
            "no quote",
            stated_entry("active", "null", "#scribe approve p-1", pass, statement),
        ),
        (
            "not session-pass",
            stated_entry(
                "active",
                "\"ok\"",
                "#scribe approve p-1",
                "{kind: scribe, name: consistency}",
                statement,
            ),
        ),
        (
            "not an approval",
            stated_entry(
                "active",
                "\"ok\"",
                "#scribe preferences: x",
                pass,
                statement,
            ),
        ),
        (
            "quote without stated_by",
            stated_entry("active", "\"ok\"", "#scribe approve p-1", "null", statement),
        ),
        (
            "forgotten with a quote",
            stated_entry("forgotten", "\"ok\"", "", pass, ""),
        ),
    ] {
        corrupt_message(&entry(&bad).map(|_| ()).expect_err(label));
    }
    // An instruction line's content is his own words, never a statement.
    let typed = stated_entry("active", "null", "#scribe approve p-1", pass, statement).replace(
        "{kind: message, role: user, message: m-1, at: 2026-11-01T09:59:00Z}",
        "{kind: instruction}",
    );
    corrupt_message(&entry(&typed).unwrap_err());
}

#[test]
fn a_candidate_context_or_statement_needs_its_subject_and_a_statement_a_user_turn() {
    let cite = format!(
        r#"{{"session":"s-1","message":"m-1","at":"2026-11-01T10:00:00Z","prompt_sha256":"{}","line":1,"cwd":"/work"}}"#,
        "0".repeat(64)
    );
    let doc = |role: &str, context: &str, statement: &str, subject: &str| {
        format!(
            r#"{{"format":{FORMAT},"proposals":[{{"id":"p-1","kind":"candidate","older":"preferences-1","candidate":{{"tag":"preferences","role":"{role}","quote":"I prefer short updates.","context":{context},"statement":{statement},"subject":{subject},"evidence":null,"cite":{cite},"project":"scribe","provider":"claude-code","host":"host","admitted":null}},"newer":null,"reason":"r","quotes":[],"state":"open","opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null,"author":{{"kind":"scribe","name":"session-pass"}},"updated_by":null,"settled_author":null,"closed_reason":null,"instruction":null}}]}}"#
        )
    };
    let path = Path::new("proposals.json");
    let read = decode_proposals(
        path,
        doc(
            "user",
            "null",
            r#""Daniel prefers short updates.""#,
            r#""updates""#,
        )
        .as_bytes(),
    )
    .unwrap();
    let candidate = read[0].candidate.as_ref().unwrap();
    assert_eq!(
        candidate.statement.as_deref(),
        Some("Daniel prefers short updates.")
    );
    assert_eq!(candidate.subject.as_deref(), Some("updates"));
    decode_proposals(path, doc("user", "null", "null", "null").as_bytes()).unwrap();
    // A context (format 24) stands on either speaker's claim.
    for role in ["user", "assistant"] {
        let read = decode_proposals(
            path,
            doc(role, r#""The user and the agent chose an update style. The user prefers short updates.""#, "null", r#""updates""#).as_bytes(),
        )
        .unwrap();
        let candidate = read[0].candidate.as_ref().unwrap();
        assert!(candidate.context.is_some() && !candidate.is_older_claim());
    }
    for bad in [
        // A context without its subject, or beside a statement.
        doc(
            "user",
            r#""The user and the agent chose an update style. The user prefers short updates.""#,
            "null",
            "null",
        ),
        doc(
            "user",
            r#""The user and the agent chose an update style. The user prefers short updates.""#,
            r#""Daniel prefers short updates.""#,
            r#""updates""#,
        ),
        doc("user", "null", r#""Daniel prefers short updates.""#, "null"),
        doc("user", "null", "null", r#""updates""#),
        doc(
            "assistant",
            "null",
            r#""The assistant measured it.""#,
            r#""updates""#,
        ),
    ] {
        corrupt_message(&decode_proposals(path, bad.as_bytes()).unwrap_err());
    }
}

/// An approved candidate `p-1` reserving `preferences-1`, settled by the
/// agent `aide` on Daniel's instruction `m-9` (2_design.md §12.3), from a
/// turn of `role`, with a statement on a user claim.
fn instructed_approval(role: &str) -> crate::Proposal {
    let sha = "0".repeat(64);
    let statement = if role == "user" {
        serde_json::json!("Daniel prefers tabs.")
    } else {
        serde_json::Value::Null
    };
    let subject = if role == "user" {
        serde_json::json!("indentation")
    } else {
        serde_json::Value::Null
    };
    let doc = serde_json::json!({"format": FORMAT, "proposals": [{
        "id": "p-1", "kind": "candidate", "older": "preferences-1", "newer": null,
        "candidate": {"tag": "preferences", "role": role, "quote": "I prefer tabs, always.",
            "evidence": null, "context": null, "statement": statement, "subject": subject,
            "cite": {"session": "s-1", "message": "m-1", "at": "2026-11-01T10:00:00Z",
                "prompt_sha256": sha, "line": 1, "cwd": "/work"},
            "project": "scribe", "provider": "claude-code", "host": "host", "admitted": null},
        "reason": "Unmarked preferences statement", "quotes": [], "state": "approved",
        "opened_at": "2026-11-01T10:00:00Z", "settled_at": "2026-11-03T10:05:00Z",
        "settled_by": {"session": "s-9", "message": "m-9", "at": "2026-11-03T10:00:00Z",
            "prompt_sha256": sha, "line": 1, "cwd": ""},
        "author": {"kind": "scribe", "name": "session-pass"}, "updated_by": null,
        "settled_author": {"kind": "agent", "name": "aide"}, "closed_reason": null,
        "instruction": {"session": "s-9", "message": "m-9", "at": "2026-11-03T10:00:00Z",
            "quote": "approve p-1, it's right"}
    }]});
    decode_proposals(Path::new("proposals.json"), doc.to_string().as_bytes())
        .unwrap()
        .remove(0)
}

#[test]
fn an_instructed_approval_entry_rereads_at_format_15_and_is_refused_at_14() {
    for role in ["user", "assistant"] {
        let proposal = instructed_approval(role);
        let e = proposal.candidate_entry(None).unwrap();
        assert_eq!(
            e.said,
            "scribe proposal settle p-1 --approve --author aide --session s-9 --instruction 'approve p-1, it'\\''s right'"
        );
        assert_eq!(
            e.approved_by,
            Some(crate::Author::agent("aide".parse().unwrap()))
        );
        assert_eq!(e.instruction, proposal.instruction);
        let kind = if role == "user" {
            crate::AuthorKind::User
        } else {
            crate::AuthorKind::Agent
        };
        assert_eq!(e.author.kind, kind, "{role}");
        let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &e).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(entry(&text).unwrap(), e, "{role}");
        // Format 15 (no `outcome` yet) admitted the command as `said`; a
        // format-14 reader never did.
        let fifteen = text
            .replacen(&format!("format: {FORMAT}\n"), "format: 15\n", 1)
            .replace("outcome: null\n", "")
            .replace("removal_instruction: null\n", "");
        assert_eq!(entry(&fifteen).unwrap(), e, "{role}");
        let old = fifteen.replacen("format: 15\n", "format: 14\n", 1);
        corrupt_message(&entry(&old).unwrap_err());
        // Nor does this one without `approved_by`.
        let mut unattributed = e.clone();
        unattributed.approved_by = None;
        let bytes = crate::store::format::encode_entry(Path::new(ENTRY), &unattributed).unwrap();
        corrupt_message(&entry(&String::from_utf8(bytes).unwrap()).unwrap_err());
    }
}

/// An entry the session pass wrote itself (format 15), built from a stated
/// entry: the pass is its author, `said` is empty and it has `outcome`. The
/// store checks the shape, not the tag, so the helper's path is kept.
fn captured_entry(status: &str, quote: &str, outcome: &str, body: &str) -> String {
    let pass = "{kind: scribe, name: session-pass}";
    stated_entry(status, quote, "", pass, body)
        .replace(
            "author: {kind: daniel, name: daniel}",
            &format!("author: {pass}"),
        )
        .replace("outcome: null", &format!("outcome: {outcome}"))
}

#[test]
fn a_captured_idea_round_trips_with_its_outcome_and_contradictions_are_refused() {
    let statement = "Daniel suggests caching the routing table per host.";
    let parked = "{state: parked, reason: \"not now\", task: null}";
    let open = "{state: open, reason: null, task: null}";
    let ok = captured_entry(
        "active",
        "\"cache it per host? not now\"",
        parked,
        statement,
    );
    let e = entry(&ok).unwrap();
    assert_eq!(e.author, crate::Author::session_pass());
    let outcome = e.outcome.clone().unwrap();
    assert_eq!(outcome.state, crate::OutcomeState::Parked);
    assert_eq!(outcome.reason.as_deref(), Some("not now"));
    let encoded = crate::store::format::encode_entry(Path::new(ENTRY), &e).unwrap();
    assert_eq!(entry(std::str::from_utf8(&encoded).unwrap()).unwrap(), e);
    // Forgotten: blank, the quote and the parked words cleared, the state kept.
    let forgotten = captured_entry(
        "forgotten",
        "null",
        "{state: parked, reason: null, task: null}",
        "",
    );
    let f = entry(&forgotten).unwrap();
    assert_eq!(f.outcome.unwrap().state, crate::OutcomeState::Parked);

    for (label, bad) in [
        (
            "no outcome",
            captured_entry("active", "\"x\"", "null", statement),
        ),
        (
            "open with a reason",
            captured_entry(
                "active",
                "\"x\"",
                "{state: open, reason: \"y\", task: null}",
                statement,
            ),
        ),
        (
            "parked without words",
            captured_entry(
                "active",
                "\"x\"",
                "{state: parked, reason: null, task: null}",
                statement,
            ),
        ),
        (
            "task without an id",
            captured_entry(
                "active",
                "\"x\"",
                "{state: task, reason: null, task: null}",
                statement,
            ),
        ),
        (
            "no quote",
            captured_entry("active", "null", open, statement),
        ),
        (
            "a said line",
            captured_entry("active", "\"x\"", open, statement)
                .replace("said: \"\"", "said: \"#scribe idea: x\""),
        ),
        (
            "outcome on Daniel's entry",
            stated_entry(
                "active",
                "\"x\"",
                "#scribe approve p-1",
                "{kind: scribe, name: session-pass}",
                statement,
            )
            .replace("outcome: null", &format!("outcome: {open}")),
        ),
    ] {
        corrupt_message(&entry(&bad).map(|_| ()).expect_err(label));
    }
}

#[test]
fn closed_reason_covered_reads_only_on_a_settled_candidate_at_format_15() {
    let path = Path::new("proposals.json");
    let mut covered = instructed_approval("user");
    covered.state = crate::ProposalState::Rejected;
    covered.closed_reason = Some(crate::ClosedReason::Covered);
    let bytes = crate::store::format::encode_proposals(path, &[covered.clone()]).unwrap();
    assert_eq!(decode_proposals(path, &bytes).unwrap(), [covered.clone()]);
    // Rewound to format 14, without format 24's candidate `context`.
    let old = String::from_utf8(bytes)
        .unwrap()
        .replacen(&format!("\"format\": {FORMAT}"), "\"format\": 14", 1)
        .replacen("\"context\": null,", "", 1);
    assert!(old.contains("\"format\": 14"), "{old}");
    // Formats 16 to 23 changed no proposal, so a format-15 one reads as
    // written.
    let fifteen = old.replacen("\"format\": 14", "\"format\": 15", 1);
    assert_eq!(
        decode_proposals(path, fifteen.as_bytes()).unwrap(),
        [covered.clone()]
    );
    corrupt_message(&decode_proposals(path, old.as_bytes()).unwrap_err());
    // An instructed rejection with no reason, or covered on a proposal
    // never settled, is refused.
    let mut unexplained = covered.clone();
    unexplained.closed_reason = None;
    let bytes = crate::store::format::encode_proposals(path, &[unexplained]).unwrap();
    corrupt_message(&decode_proposals(path, &bytes).unwrap_err());
    let mut unsettled = covered;
    unsettled.instruction = None;
    unsettled.settled_by = None;
    unsettled.settled_at = None;
    let bytes = crate::store::format::encode_proposals(path, &[unsettled]).unwrap();
    corrupt_message(&decode_proposals(path, &bytes).unwrap_err());
}

#[test]
fn a_format_fourteen_or_fifteen_entry_reads_with_no_outcome_and_a_stray_outcome_is_refused() {
    let pass = "{kind: scribe, name: session-pass}";
    let current = stated_entry("active", "\"ok\"", "#scribe approve p-1", pass, "stmt");
    for from in [14, 15] {
        let older = current
            .replace(&format!("format: {FORMAT}"), &format!("format: {from}"))
            .replace("removal_instruction: null\n", "");
        let old = older.replace("outcome: null\n", "");
        let e = entry(&old).unwrap();
        assert_eq!(e.outcome, None, "{from}");
        assert_eq!(e.stated_by, Some(crate::Author::session_pass()), "{from}");
        corrupt_message(&entry(&older).map(|_| ()).unwrap_err());
    }
}

/// A frozen format-15 entry: an approval an agent ran on Daniel's checked
/// instruction (the shape format 15 admitted), stated by the pass, with no
/// `outcome` key. `extra` is inserted after `stated_by`.
fn format_fifteen_entry(extra: &str, project: &str) -> String {
    format!(
        "---
format: 15
kind: entry
id: preferences-1
tag: preferences
status: active
project: {project}
topics: [ci]
related: []
provider: claude-code
host: host
author: {{kind: daniel, name: daniel}}
updated_by: null
created: 2026-11-01T10:00:00Z
updated: 2026-11-01T10:00:00Z
inactive_since: null
said: \"scribe proposal settle p-1 --approve --author aide --session s-9 --instruction 'approve p-1'\"
content_from: {{kind: message, role: user, message: m-1, at: 2026-11-01T09:59:00Z}}
cite: {{session: s-1, message: m-1, at: 2026-11-01T09:59:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
retirement_reason: null
superseded_by: null
review_after: null
quote: \"I prefer short updates, always.\"
window: null
evidence: null
as_of: null
approved_by: {{kind: agent, name: aide}}
instruction: {{session: s-9, message: m-9, at: 2026-11-03T10:00:00Z, quote: \"approve p-1\"}}
stated_by: {{kind: scribe, name: session-pass}}
{extra}---
Daniel prefers short updates.
",
        sha = "0".repeat(64)
    )
}

#[test]
fn a_format_fifteen_store_upgrades_with_a_null_outcome_only() {
    // Checked against the frozen format-15 shape, then `outcome` is null and
    // nothing else changes, including a `!local` project tag.
    for project in ["scribe", "!local scribe"] {
        let decoded = entry(&format_fifteen_entry("", project)).unwrap();
        assert_eq!(decoded.outcome, None);
        assert_eq!(decoded.project.as_str(), "scribe");
        assert_eq!(decoded.stated_by, Some(crate::Author::session_pass()));
        assert_eq!(
            decoded.approved_by,
            Some(crate::Author::agent("aide".parse().unwrap()))
        );
        assert_eq!(
            decoded.quote.as_deref(),
            Some("I prefer short updates, always.")
        );
        assert_eq!(decoded.content, "Daniel prefers short updates.");
        let encoded = String::from_utf8(
            crate::store::format::encode_entry(Path::new(ENTRY), &decoded).unwrap(),
        )
        .unwrap();
        assert!(encoded.starts_with(&format!("---\nformat: {FORMAT}\n")));
        assert!(encoded.contains("\noutcome: null\n"), "{encoded}");
        assert_eq!(entry(&encoded).unwrap(), decoded);
        let back = encoded
            .replacen(&format!("format: {FORMAT}\n"), "format: 15\n", 1)
            .replace("outcome: null\n", "")
            .replace("removal_instruction: null\n", "");
        assert_eq!(entry(&back).unwrap(), decoded);
    }
    // Strict at format 15: `outcome` is a format-16 key.
    let stray = format_fifteen_entry("outcome: null\n", "scribe");
    corrupt_message(&entry(&stray).unwrap_err());
}

#[test]
fn a_format_sixteen_proposals_document_reads_at_the_current_shape() {
    // Formats 16 to 21 changed no proposal (16 and 18 changed only entries,
    // 17 and 19 added record kinds, 20 and 21 changed memory items, 22
    // added marks.json and the annotation entry), so a format-16 to
    // format-21 proposals.json
    // has the current shape: the current document with only its `format`
    // lowered reads back equal, and a stray key is still refused.
    let path = Path::new("proposals.json");
    let old = br#"{"format":3,"proposals":[{"id":"p-1","kind":"stale","older":"preferences-1","newer":null,"reason":"review","quotes":[],"state":"open","opened_at":"2026-11-01T10:00:00Z","settled_at":null,"settled_by":null}]}"#;
    let proposals = decode_proposals(path, old).unwrap();
    let encoded = crate::store::format::encode_proposals(path, &proposals).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["format"], FORMAT);
    for older in [16, 17, 18, 19, 20, 21] {
        value["format"] = serde_json::json!(older);
        assert_eq!(
            decode_proposals(path, &serde_json::to_vec(&value).unwrap()).unwrap(),
            proposals,
            "format {older}"
        );
    }
    value["stray"] = serde_json::json!(1);
    assert!(decode_proposals(path, &serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn a_format_24_proposals_document_reads_at_the_current_shape() {
    // Format 25 changed only memory files: a format-24 proposals.json,
    // context and all, reads back equal to the current one.
    let path = Path::new("proposals.json");
    let cite = serde_json::json!({"session":"7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02",
        "message":"u-1", "at":"2026-11-02T00:00:00Z",
        "prompt_sha256":"0".repeat(64), "line":1, "cwd":"/work"});
    let proposal = serde_json::json!({
        "id":"p-6", "kind":"candidate", "older":"preferences-3", "newer":null,
        "candidate": {
            "tag":"preferences", "role":"user", "quote":"show the newest five.",
            "context":"The user trims the popup.", "statement":null,
            "subject":"the popup", "evidence":null, "cite":cite,
            "project":"scribe", "provider":"claude-code", "host":"host", "admitted":null
        },
        "reason":"Unmarked preferences statement", "quotes":[],
        "state":"open", "opened_at":"2026-11-02T00:06:00Z", "settled_at":null,
        "settled_by":null, "author":{"kind":"scribe", "name":"session-pass"},
        "settled_author":null, "updated_by":null, "closed_reason":null, "instruction":null
    });
    let mut value = serde_json::json!({"format":24, "proposals":[proposal]});
    let old = decode_proposals(path, &serde_json::to_vec(&value).unwrap()).unwrap();
    value["format"] = serde_json::json!(FORMAT);
    assert_eq!(
        decode_proposals(path, &serde_json::to_vec(&value).unwrap()).unwrap(),
        old
    );
    value["format"] = serde_json::json!(24);
    value["stray"] = serde_json::json!(1);
    assert!(decode_proposals(path, &serde_json::to_vec(&value).unwrap()).is_err());
}

const MEMORY_FILE: &str = "memory/m-1.md";

/// A memory item at format `format`, with `extra` frontmatter lines, the
/// status `status` and the occurrences `occurrences` (a YAML list).
fn memory_text(format: u32, status: &str, extra: &str, occurrences: &str) -> String {
    format!(
        "---\nformat: {format}\nkind: memory\nid: m-1\nsection: now\nstatus: {status}\n\
         statement: Said the release ships on 2026-12-01.\nproject: null\n\
         event_date: 2026-12-01\nsensitive: false\nsensitive_kind: null\n\
         acknowledged: null\nretirement_reason: null\n{extra}\
         author: {{kind: scribe, name: session-pass}}\n\
         updated_by: {{kind: scribe, name: session-pass}}\n\
         created: 2026-11-21T05:40:00Z\nupdated: 2026-11-21T05:40:00Z\n\
         occurrences: {occurrences}\n---\n"
    )
}

const ONE_OCCURRENCE: &str = "[{n: 1, date: 2026-11-21T05:40:00Z, session: 0b8ef4a6-0c6a-4d77-9c41-6a4f7b2c1d10, message: u-1, host: mac, project: scribe, quote: \"the release ships on december 1\", author: {kind: scribe, name: session-pass}}]";

/// The keys format 20 added, empty.
const V20_LINKS: &str = "promoted_from: null\npromotion: null\nmerged_into: null\nsuperseded_by: null\nunrelated_to: []\n";

/// Every key of the current format, empty.
const NO_LINKS: &str = "promoted_from: null\npromotion: null\nmerged_into: null\nsuperseded_by: null\nunrelated_to: []\npreference_proposal: null\n";

fn memory(text: &str) -> crate::Result<crate::memory::legacy::MemoryItem> {
    crate::store::format::decode_memory(Path::new(MEMORY_FILE), text.as_bytes(), "m-1")
}

#[test]
fn a_format_nineteen_memory_item_reads_without_the_consolidation_keys_and_refuses_them() {
    let item = memory(&memory_text(19, "active", "", ONE_OCCURRENCE)).unwrap();
    assert_eq!(item.merged_into, None);
    assert!(item.unrelated_to.is_empty());
    let current = memory(&memory_text(FORMAT, "active", NO_LINKS, ONE_OCCURRENCE)).unwrap();
    assert_eq!(current, item);
    // Format 19 has neither the keys nor the statuses.
    corrupt_message(&memory(&memory_text(19, "active", NO_LINKS, ONE_OCCURRENCE)).unwrap_err());
    corrupt_message(&memory(&memory_text(19, "superseded", "", ONE_OCCURRENCE)).unwrap_err());
    // The current format requires every key.
    corrupt_message(&memory(&memory_text(FORMAT, "active", "", ONE_OCCURRENCE)).unwrap_err());
}

#[test]
fn a_format_twenty_memory_item_reads_without_preference_proposal_and_refuses_it() {
    let item = memory(&memory_text(20, "active", V20_LINKS, ONE_OCCURRENCE)).unwrap();
    assert_eq!(item.preference_proposal, None);
    let current = memory(&memory_text(FORMAT, "active", NO_LINKS, ONE_OCCURRENCE)).unwrap();
    assert_eq!(current, item);
    let linked = NO_LINKS.replacen("preference_proposal: null", "preference_proposal: p-4", 1);
    let item = memory(&memory_text(FORMAT, "active", &linked, ONE_OCCURRENCE)).unwrap();
    assert_eq!(item.preference_proposal.unwrap().to_string(), "p-4");
    // Format 20 has no `preference_proposal`; the current format needs it.
    corrupt_message(&memory(&memory_text(20, "active", NO_LINKS, ONE_OCCURRENCE)).unwrap_err());
    corrupt_message(
        &memory(&memory_text(FORMAT, "active", V20_LINKS, ONE_OCCURRENCE)).unwrap_err(),
    );
}

#[test]
fn only_a_merged_or_promoted_memory_item_may_hold_no_occurrence_and_it_names_its_target() {
    let merged = NO_LINKS.replacen("merged_into: null", "merged_into: m-2", 1);
    let item = memory(&memory_text(FORMAT, "merged", &merged, "[]")).unwrap();
    assert_eq!(item.merged_into.unwrap().as_str(), "m-2");
    let promoted = NO_LINKS.replacen(
        "promotion: null",
        "promotion: {rule: user_says, to: m-2, profile_from: null, profile_to: null}",
        1,
    );
    memory(&memory_text(FORMAT, "promoted", &promoted, "[]")).unwrap();
    // An active item needs an occurrence; a merged one names its target.
    corrupt_message(&memory(&memory_text(FORMAT, "active", NO_LINKS, "[]")).unwrap_err());
    corrupt_message(&memory(&memory_text(FORMAT, "merged", NO_LINKS, "[]")).unwrap_err());
}

/// A frozen format-21 entry Daniel typed, with every key format 21 has.
fn format_twenty_one_entry(stated_by: &str, quote: &str) -> String {
    format!(
        "---
format: 21
kind: entry
id: preferences-1
tag: preferences
status: active
project: scribe
topics: [ci]
related: []
provider: claude-code
host: host
author: {{kind: daniel, name: daniel}}
updated_by: null
created: 2026-11-01T10:00:00Z
updated: 2026-11-01T10:00:00Z
inactive_since: null
said: \"I want short updates #scribe\"
content_from: {{kind: message, role: user, message: m-1, at: 2026-11-01T09:59:00Z}}
cite: {{session: s-1, message: m-1, at: 2026-11-01T09:59:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
retirement_reason: null
superseded_by: null
review_after: null
quote: {quote}
window: null
evidence: null
as_of: null
approved_by: null
instruction: null
removal_instruction: null
stated_by: {stated_by}
outcome: null
---
Daniel wants short updates.
",
        sha = "0".repeat(64)
    )
}

const ANNOTATION: &str = "{kind: scribe, name: annotation}";

#[test]
fn a_format_twenty_one_store_upgrades_unchanged_and_only_format_twenty_two_admits_an_annotation() {
    // A frozen format-21 entry reads, and re-encodes at the current format
    // with nothing else changed.
    let plain = format_twenty_one_entry("null", "null").replace(
        "content_from: {kind: message, role: user, message: m-1, at: 2026-11-01T09:59:00Z}",
        "content_from: {kind: instruction}",
    );
    let decoded = entry(&plain).unwrap();
    let encoded =
        String::from_utf8(crate::store::format::encode_entry(Path::new(ENTRY), &decoded).unwrap())
            .unwrap();
    assert!(encoded.starts_with(&format!("---\nformat: {FORMAT}\n")));
    assert_eq!(entry(&encoded).unwrap(), decoded);
    assert_eq!(
        entry(&encoded.replacen(&format!("format: {FORMAT}\n"), "format: 21\n", 1)).unwrap(),
        decoded
    );

    // An annotation: scribe's statement of Daniel's quoted words.
    let at_twenty_one = format_twenty_one_entry(ANNOTATION, "\"I want short updates\"");
    corrupt_message(&entry(&at_twenty_one).unwrap_err());
    let current = at_twenty_one.replacen("format: 21\n", &format!("format: {FORMAT}\n"), 1);
    let annotation = entry(&current).unwrap();
    assert_eq!(annotation.stated_by, Some(crate::Author::annotation()));
    assert_eq!(annotation.author, crate::Author::user());
    assert_eq!(annotation.quote.as_deref(), Some("I want short updates"));
    // It needs his quote, his message, and no approval.
    corrupt_message(
        &entry(&current.replace("quote: \"I want short updates\"", "quote: null")).unwrap_err(),
    );
    corrupt_message(&entry(&current.replace("role: user", "role: assistant")).unwrap_err());
    corrupt_message(
        &entry(&current.replace(
            "approved_by: null",
            "approved_by: {kind: agent, name: aide}",
        ))
        .unwrap_err(),
    );
    // Forgotten, it keeps no words and still reads.
    let mut entries = vec![annotation.clone()];
    let forgotten = crate::consistency::forget(
        &annotation.id,
        &mut [],
        &mut entries,
        time::macros::datetime!(2026-11-22 10:00 UTC),
    );
    assert!(
        matches!(forgotten, crate::consistency::Settled::Forgotten(_)),
        "{forgotten:?}"
    );
    let encoded = crate::store::format::encode_entry(Path::new(ENTRY), &entries[0]).unwrap();
    let back = entry(std::str::from_utf8(&encoded).unwrap()).unwrap();
    assert_eq!(back.status, crate::EntryStatus::Forgotten);
    assert!(back.quote.is_none() && back.said.is_empty() && back.content.is_empty());
}

#[test]
fn marks_json_is_new_at_format_twenty_two_and_holds_consistent_outcomes() {
    let path = Path::new("marks.json");
    let doc = |format: u32, mark: &str| {
        format!("{{\"format\":{format},\"marks\":[{mark}]}}").into_bytes()
    };
    let entry_mark = r#"{"session":"s-1","message":"u-1","prompt_sha256":"aaaa","marker":1,"outcome":"entry","entry":"preferences-1","code":null,"attempts":1,"at":"2026-11-21T10:00:00Z"}"#;
    let marks = decode_marks(path, &doc(FORMAT, entry_mark)).unwrap();
    assert_eq!(marks.len(), 1);
    let encoded = encode_marks(path, &marks).unwrap();
    assert_eq!(decode_marks(path, &encoded).unwrap(), marks);
    // Format 21 has no marks.json, and a newer one is refused.
    assert!(decode_marks(path, &doc(21, entry_mark)).is_err());
    assert!(decode_marks(path, &doc(FORMAT + 1, entry_mark)).is_err());
    // An outcome whose fields disagree, or a stray key, is refused.
    let inconsistent = entry_mark.replace(r#""entry":"preferences-1""#, r#""entry":null"#);
    assert!(decode_marks(path, &doc(FORMAT, &inconsistent)).is_err());
    let stray = entry_mark.replace(r#""attempts":1"#, r#""attempts":1,"quote":"words""#);
    assert!(decode_marks(path, &doc(FORMAT, &stray)).is_err());
}

#[test]
fn a_format_22_store_reads_the_daniel_author_as_the_user_and_writes_user() {
    // 2_design.md §15.8: before format 23 the user's channel was stored as
    // `{kind: daniel, name: daniel}`. It reads as `Author::user()` in every
    // document, and the next write persists `user` at format 23.
    let legacy = serde_json::json!({"kind": "daniel", "name": "daniel"});
    let user = crate::record::Author::user();

    let current = entry(&entry_text("")).unwrap();
    let encoded = crate::store::format::encode_entry(Path::new(ENTRY), &current).unwrap();
    let encoded = String::from_utf8(encoded).unwrap();
    let (front, _) = split_frontmatter(Path::new(ENTRY), encoded.as_bytes()).unwrap();
    let mut front: serde_json::Value = yaml_serde::from_str(front).unwrap();
    assert_eq!(front["format"], serde_json::json!(FORMAT));
    assert_eq!(
        front["author"],
        serde_json::json!({"kind": "user", "name": "user"})
    );
    front["format"] = serde_json::json!(22);
    front["author"] = legacy.clone();
    let body = encoded.split_once("\n---\n").unwrap().1;
    let v22 = format!("---\n{}---\n{body}", yaml_serde::to_string(&front).unwrap());
    let read = entry(&v22).unwrap();
    assert_eq!(read.author, user);
    assert_eq!(read, current);

    // A settled proposal, written at the current format and rewound to 22.
    let legacy_doc = br#"{"format":3,"proposals":[
        {"id":"p-2","kind":"stale","older":"preferences-1","newer":null,"reason":"r",
         "quotes":[{"entry":"preferences-1","text":"t"}],"state":"rejected",
         "opened_at":"2026-11-01T10:00:00Z","settled_at":"2026-11-01T11:00:00Z","settled_by":null}]}"#;
    let list = decode_proposals(Path::new("proposals.json"), legacy_doc).unwrap();
    let encoded =
        crate::store::format::encode_proposals(Path::new("proposals.json"), &list).unwrap();
    let mut doc: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(doc["format"], serde_json::json!(FORMAT));
    assert_eq!(
        doc["proposals"][0]["settled_author"],
        serde_json::json!({"kind": "user", "name": "user"})
    );
    doc["format"] = serde_json::json!(22);
    doc["proposals"][0]["settled_author"] = legacy.clone();
    let read = decode_proposals(Path::new("proposals.json"), doc.to_string().as_bytes()).unwrap();
    assert_eq!(read, list);
    assert_eq!(read[0].settled_author, Some(user.clone()));

    // Documents outside the upgrade registry read through the same
    // `Author`: an agent named `daniel` stays an agent.
    let author: crate::record::Author = serde_json::from_value(legacy).unwrap();
    assert_eq!(author, user);
    let agent: crate::record::Author =
        serde_json::from_value(serde_json::json!({"kind": "agent", "name": "daniel"})).unwrap();
    assert_eq!(agent.kind, crate::AuthorKind::Agent);
    assert_eq!(agent.name.as_str(), "daniel");
    // An unknown field is still refused.
    assert!(
        serde_json::from_value::<crate::record::Author>(
            serde_json::json!({"kind": "user", "name": "user", "x": 1})
        )
        .is_err()
    );
}

/// The format-23 `proposals.json` of a store from before claims carried
/// context: an open older claim (a statement and subject on the user's
/// turn) and an open assistant claim with neither.
fn format_23_proposals() -> serde_json::Value {
    let cite = serde_json::json!({"session": "s-1", "message": "m-1",
        "at": "2026-11-01T10:00:00Z", "prompt_sha256": "0".repeat(64), "line": 1,
        "cwd": "/work"});
    let candidate = |n: u32,
                     role: &str,
                     statement: serde_json::Value,
                     subject: serde_json::Value| {
        serde_json::json!({
            "id": format!("p-{n}"), "kind": "candidate", "older": format!("preferences-{n}"),
            "newer": null,
            "candidate": {"tag": "preferences", "role": role, "quote": "I prefer short updates.",
                "statement": statement, "subject": subject, "evidence": null, "cite": cite,
                "project": "scribe", "provider": "claude-code", "host": "host",
                "admitted": null},
            "reason": "Unmarked preferences statement", "quotes": [], "state": "open",
            "opened_at": "2026-11-01T10:00:00Z", "settled_at": null, "settled_by": null,
            "author": {"kind": "scribe", "name": "session-pass"}, "updated_by": null,
            "settled_author": null, "closed_reason": null, "instruction": null
        })
    };
    serde_json::json!({"format": 23, "proposals": [
        candidate(1, "user", serde_json::json!("The user prefers short updates."),
            serde_json::json!("status updates")),
        candidate(2, "assistant", serde_json::Value::Null, serde_json::Value::Null),
    ]})
}

#[test]
fn a_format_23_store_upgrades_to_24_and_its_claims_read_with_no_context() {
    // 2_design.md §15.3: format 24 adds `context` to each claim. A
    // format-23 claim reads with none; one with a statement is an older
    // claim, shown with its statement in the context's place.
    let path = Path::new("proposals.json");
    let old = format_23_proposals();
    let proposals = decode_proposals(path, old.to_string().as_bytes()).unwrap();
    let older = proposals[0].candidate.as_ref().unwrap();
    assert_eq!(older.context, None);
    assert!(older.is_older_claim());
    assert_eq!(older.stated(), Some("The user prefers short updates."));
    let bare = proposals[1].candidate.as_ref().unwrap();
    assert_eq!((&bare.context, bare.stated()), (&None, None));
    assert!(!bare.is_older_claim());

    // The next write persists the current format, the document unchanged
    // but for its format and each candidate's null `context`.
    let encoded = crate::store::format::encode_proposals(path, &proposals).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["format"], serde_json::json!(FORMAT));
    value["format"] = serde_json::json!(23);
    for p in value["proposals"].as_array_mut().unwrap() {
        let c = p["candidate"].as_object_mut().unwrap();
        assert_eq!(c.remove("context"), Some(serde_json::Value::Null));
    }
    assert_eq!(value, old);

    // Strict at format 23: a candidate carrying `context` is refused.
    let mut early = old;
    early["proposals"][1]["candidate"]["context"] =
        serde_json::json!("The agent measured the build. It took four minutes.");
    early["proposals"][1]["candidate"]["subject"] = serde_json::json!("the build");
    corrupt_message(&decode_proposals(path, early.to_string().as_bytes()).unwrap_err());
}

#[test]
fn an_approved_context_claim_writes_the_context_as_content_and_needs_format_24() {
    // 2_design.md §15.3: approval writes the context as content, the turn's
    // words as `quote` and `stated_by` the session pass, for either speaker;
    // the assistant's entry stays an agent's, and only format 24 admits it.
    let context = "The user and the agent were choosing how scribe stores session \
                   records. The user asked for an append-only log.";
    for role in ["user", "assistant"] {
        let mut proposal = instructed_approval(role);
        let candidate = proposal.candidate.as_mut().unwrap();
        candidate.statement = None;
        candidate.context = Some(context.to_owned());
        candidate.subject = Some(String::from("session records"));
        let e = proposal.candidate_entry(None).unwrap();
        assert_eq!(e.content, context, "{role}");
        assert_eq!(e.quote.as_deref(), Some("I prefer tabs, always."), "{role}");
        assert_eq!(e.stated_by, Some(crate::Author::session_pass()), "{role}");
        let kind = if role == "user" {
            crate::AuthorKind::User
        } else {
            crate::AuthorKind::Agent
        };
        assert_eq!(e.author.kind, kind, "{role}");
        let text =
            String::from_utf8(crate::store::format::encode_entry(Path::new(ENTRY), &e).unwrap())
                .unwrap();
        assert_eq!(entry(&text).unwrap(), e, "{role}");
        let older = text.replacen(&format!("format: {FORMAT}\n"), "format: 23\n", 1);
        if role == "user" {
            // A user's stated entry reads at every format since 14.
            assert_eq!(entry(&older).unwrap(), e);
        } else {
            corrupt_message(&entry(&older).unwrap_err());
        }
    }
}

#[test]
fn a_spoken_instruction_agent_entry_reads_with_agent_stated_by_and_quote() {
    let statement = "The user prefers short progress updates.";
    let agent = "{kind: agent, name: opus}";
    let raw = stated_entry("active", "\"short updates please\"", "", agent, statement)
        .replace("author: {kind: daniel, name: daniel}", &format!("author: {agent}"))
        .replace(
            "content_from: {kind: message, role: user, message: m-1, at: 2026-11-01T09:59:00Z}",
            "content_from: {kind: agent}",
        )
        .replace(
            &format!(
                "cite: {{session: s-1, message: m-1, at: 2026-11-01T09:59:00Z, prompt_sha256: \"{}\", line: 1, cwd: /work}}",
                "0".repeat(64)
            ),
            "cite: null",
        );
    let e = entry(&raw).unwrap();
    assert_eq!(e.author, crate::Author::agent("opus".parse().unwrap()));
    assert_eq!(
        e.stated_by,
        Some(crate::Author::agent("opus".parse().unwrap()))
    );
    assert_eq!(e.quote.as_deref(), Some("short updates please"));
    let encoded = crate::store::format::encode_entry(Path::new(ENTRY), &e).unwrap();
    assert_eq!(entry(std::str::from_utf8(&encoded).unwrap()).unwrap(), e);

    // Forgotten keeps stated_by and clears quote and body.
    let forgotten = raw
        .replace("status: active", "status: forgotten")
        .replace(
            "inactive_since: null",
            "inactive_since: 2026-11-02T10:00:00Z",
        )
        .replace("quote: \"short updates please\"", "quote: null")
        .replace(statement, "");
    assert_eq!(entry(&forgotten).unwrap().quote, None);

    // A subsequent metadata-only edit by a third agent after another agent
    // rewrote the statement keeps the entry readable.
    let re_edited = raw
        .replace("updated_by: null", "updated_by: {kind: agent, name: haiku}")
        .replace(
            &format!("stated_by: {agent}"),
            "stated_by: {kind: agent, name: sonnet}",
        );
    let re_decoded = entry(&re_edited).unwrap();
    assert_eq!(
        re_decoded.stated_by,
        Some(crate::Author::agent("sonnet".parse().unwrap()))
    );
    assert_eq!(
        re_decoded.updated_by,
        Some(crate::Author::agent("haiku".parse().unwrap()))
    );

    // Retirement by the original author and forgetting by another agent keep
    // the last statement writer even though the updater has changed.
    for (status, updater) in [("retired", "opus"), ("forgotten", "delta")] {
        let mut record = re_edited
            .replace("status: active", &format!("status: {status}"))
            .replace(
                "inactive_since: null",
                "inactive_since: 2026-11-02T10:00:00Z",
            )
            .replace("name: haiku", &format!("name: {updater}"));
        if status == "forgotten" {
            record = record
                .replace("quote: \"short updates please\"", "quote: null")
                .replace(statement, "");
        }
        let decoded = entry(&record).unwrap();
        assert_eq!(
            decoded.stated_by,
            Some(crate::Author::agent("sonnet".parse().unwrap()))
        );
        assert_eq!(
            decoded.updated_by,
            Some(crate::Author::agent(updater.parse().unwrap()))
        );
    }
    let non_agent_update = re_edited.replace(
        "updated_by: {kind: agent, name: haiku}",
        "updated_by: {kind: scribe, name: consistency}",
    );
    corrupt_message(&entry(&non_agent_update).unwrap_err());

    // Active without quote, or mismatched/scribe stated_by, is refused.
    corrupt_message(
        &entry(&raw.replace("quote: \"short updates please\"", "quote: null")).unwrap_err(),
    );
    corrupt_message(
        &entry(&raw.replace(
            &format!("stated_by: {agent}"),
            "stated_by: {kind: agent, name: other}",
        ))
        .unwrap_err(),
    );
    corrupt_message(
        &entry(&raw.replace(
            &format!("stated_by: {agent}"),
            "stated_by: {kind: scribe, name: session-pass}",
        ))
        .unwrap_err(),
    );
}
