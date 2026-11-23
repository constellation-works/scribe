use crate::store::audit::{AUDIT_FORMAT, AuditLine, encode_line};
use crate::store::{AuditEvent, AuditStatus};
use time::macros::datetime;

fn failed_add() -> AuditEvent {
    AuditEvent {
        at: datetime!(2026-09-26 12:00:00 UTC),
        command: String::from("hook user-prompt-submit"),
        status: AuditStatus::Failure,
        target: None,
        error_code: Some(String::from("store_locked")),
        author: None,
        instruction_session: None,
        instruction_message: None,
        issue: None,
        duration_ms: 42,
    }
}

#[test]
fn a_line_is_one_json_object_with_its_format_and_a_newline() {
    let line = encode_line(AUDIT_FORMAT, &failed_add()).unwrap();
    assert_eq!(line.last(), Some(&b'\n'));
    assert_eq!(line.iter().filter(|&&b| b == b'\n').count(), 1);
    let value: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(value["format"], AUDIT_FORMAT);
    assert_eq!(value["status"], "failure");
    assert_eq!(value["at"], "2026-09-26T12:00:00Z");
}

#[test]
fn a_line_round_trips_losslessly() {
    // STD-02 R16.
    let line = encode_line(AUDIT_FORMAT, &failed_add()).unwrap();
    let back: AuditLine = serde_json::from_slice(&line).unwrap();
    assert_eq!(back.format, AUDIT_FORMAT);
    assert_eq!(back.event, failed_add());
}

#[test]
fn an_event_has_no_field_for_arguments_or_error_text() {
    // STD-05 R13: the shape itself keeps record contents and paths out.
    let line = encode_line(AUDIT_FORMAT, &failed_add()).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&line).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "at",
            "author",
            "command",
            "duration_ms",
            "error_code",
            "format",
            "instruction_message",
            "instruction_session",
            "issue",
            "status",
            "target"
        ]
    );
}

#[test]
fn a_format_one_line_reads_with_no_author() {
    // STD-02 R16: the log written before `author` existed still reads.
    let old = br#"{"format":1,"at":"2026-09-26T12:00:00Z","command":"hook stop","status":"success","target":null,"error_code":null,"duration_ms":3}"#;
    let line: AuditLine = serde_json::from_slice(old).unwrap();
    assert_eq!(line.format, 1);
    assert_eq!(line.event.author, None);
    assert_eq!(line.event.command, "hook stop");
}

#[test]
fn a_format_two_line_reads_with_no_instruction() {
    // STD-02 R16: the log written before the instruction ids existed reads.
    let old = br#"{"format":2,"at":"2026-09-26T12:00:00Z","command":"proposal settle","status":"success","target":"p-1","error_code":null,"author":"opus","duration_ms":3}"#;
    let line: AuditLine = serde_json::from_slice(old).unwrap();
    assert_eq!(line.format, 2);
    assert_eq!(line.event.author.as_deref(), Some("opus"));
    assert_eq!(line.event.instruction_session, None);
    assert_eq!(line.event.instruction_message, None);
}

#[test]
fn a_format_five_issue_line_still_reads_as_history() {
    // Issue records left with 2_design.md §15.9 phase 2; the lines written
    // before keep their issue fields, read and never counted.
    let old = br#"{"format":5,"at":"2026-11-21T09:00:00Z","command":"hook user-prompt-submit","status":"success","target":"session:s-1","error_code":null,"author":null,"issue":{"verb":"fire","session":"s-1","trigger":"root cause","request_sha256":["ab"]},"duration_ms":2}"#;
    let line: AuditLine = serde_json::from_slice(old).unwrap();
    let issue = line.event.issue.unwrap();
    assert_eq!(issue.verb, "fire");
    assert_eq!(issue.trigger.as_deref(), Some("root cause"));
}
