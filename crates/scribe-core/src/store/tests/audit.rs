use crate::store::audit::{AUDIT_FORMAT, AuditLine, encode_line, issue_stats};
use crate::store::{AuditEvent, AuditStatus, IssueAudit};
use time::OffsetDateTime;
use time::macros::{date, datetime};

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
    let line = encode_line(&failed_add()).unwrap();
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
    let line = encode_line(&failed_add()).unwrap();
    let back: AuditLine = serde_json::from_slice(&line).unwrap();
    assert_eq!(back.format, AUDIT_FORMAT);
    assert_eq!(back.event, failed_add());
}

#[test]
fn an_event_has_no_field_for_arguments_or_error_text() {
    // STD-05 R13: the shape itself keeps record contents and paths out.
    let line = encode_line(&failed_add()).unwrap();
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

// --- issue stats (2_design.md §11.4) ---

const S: &str = "s-1";

fn event(at: OffsetDateTime, status: AuditStatus, issue: IssueAudit) -> AuditEvent {
    AuditEvent {
        at,
        command: String::from("issue add"),
        status,
        target: None,
        error_code: None,
        author: None,
        instruction_session: None,
        instruction_message: None,
        issue: Some(issue),
        duration_ms: 1,
    }
}

fn fired(at: OffsetDateTime, sha: &str) -> AuditEvent {
    event(
        at,
        AuditStatus::Success,
        IssueAudit {
            verb: String::from("fire"),
            session: Some(S.to_owned()),
            trigger: Some(String::from("root cause")),
            request_sha256: vec![sha.to_owned()],
            ..IssueAudit::default()
        },
    )
}

fn wrote(at: OffsetDateTime, verb: &str, id: &str, n: u32, shas: &[&str]) -> AuditEvent {
    event(
        at,
        AuditStatus::Success,
        IssueAudit {
            verb: verb.to_owned(),
            id: Some(id.to_owned()),
            occurrence: Some(n),
            session: Some(S.to_owned()),
            request_message: Some(String::from("u-1")),
            request_sha256: shas.iter().map(|s| (*s).to_owned()).collect(),
            ..IssueAudit::default()
        },
    )
}

fn withdrew(at: OffsetDateTime, verb: &str, id: &str, n: Option<u32>) -> AuditEvent {
    event(
        at,
        AuditStatus::Success,
        IssueAudit {
            verb: verb.to_owned(),
            id: Some(id.to_owned()),
            occurrence: n,
            ..IssueAudit::default()
        },
    )
}

#[test]
fn a_write_is_matched_to_the_fire_on_its_turn_and_withdrawals_count_once() {
    let t = |h: i64| datetime!(2026-11-21 09:00 UTC) + time::Duration::hours(h);
    let events = vec![
        fired(t(0), "a"),
        // A failed write never counts.
        event(
            t(0),
            AuditStatus::Failure,
            IssueAudit {
                verb: String::from("add"),
                ..IssueAudit::default()
            },
        ),
        // Matched through one of its turn's block hashes.
        wrote(t(1), "add", "issue-1", 1, &["whole", "a"]),
        fired(t(2), "b"),
        // A write before its fire is not that fire's.
        wrote(t(3), "occur", "issue-1", 2, &["c"]),
        fired(t(4), "c"),
        // Moved, then its new home forgotten: withdrawn once.
        withdrew(t(5), "move", "issue-1", Some(2)),
        withdrew(t(6), "forget", "issue-1", None),
    ];
    let s = issue_stats(&events, None, 0);
    assert_eq!(
        (s.fires, s.fires_with_write, s.writes, s.writes_without_fire),
        (3, 1, 2, 1)
    );
    // The whole issue's forget withdraws both writes; the move only #2.
    assert_eq!(
        (s.writes_forgotten, s.writes_moved, s.writes_withdrawn),
        (2, 1, 2)
    );
    assert_eq!(s.precision(), Some((0, 2)));
    assert_eq!(s.trigger_yield(), Some((1, 3)));
    // `since` keeps later fires and writes; withdrawals count whenever.
    let later = issue_stats(&events, Some(date!(2026 - 11 - 21)), 2);
    assert_eq!(later.fires, 3);
    assert_eq!(later.skipped_lines, 2);
    let none = issue_stats(&events, Some(date!(2026 - 11 - 22)), 0);
    assert_eq!((none.fires, none.writes), (0, 0));
    assert_eq!((none.precision(), none.trigger_yield()), (None, None));
}
