use crate::{MarkRecord, MarkState};
use time::macros::datetime;

// --- marks.json records ---------------------------------------------------

#[test]
fn a_mark_record_is_consistent_only_with_the_fields_its_outcome_has() {
    let record = |outcome, entry: Option<&str>, code: Option<&str>| MarkRecord {
        session: "0b8ef4a6-0c6a-4d77-9c41-6a4f7b2c1d10".parse().unwrap(),
        message: String::from("u-1"),
        prompt_sha256: "0".repeat(64),
        marker: 1,
        outcome,
        entry: entry.map(|e| e.parse().unwrap()),
        code: code.map(str::to_owned),
        attempts: 1,
        at: datetime!(2026-11-21 10:00 UTC),
    };
    assert!(record(MarkState::Entry, Some("preferences-1"), None).is_consistent());
    assert!(record(MarkState::None, None, None).is_consistent());
    assert!(record(MarkState::Refused, None, Some("model_failed")).is_consistent());
    assert!(record(MarkState::Pending, None, None).is_consistent());
    assert!(!record(MarkState::Entry, None, None).is_consistent());
    assert!(!record(MarkState::Refused, None, None).is_consistent());
    assert!(!record(MarkState::None, Some("preferences-1"), None).is_consistent());
    assert!(MarkState::None.is_final() && !MarkState::Pending.is_final());
}
