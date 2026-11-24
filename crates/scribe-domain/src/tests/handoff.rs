//! Unit tests for the handoff contract: headings, order, size limit,
//! detection and session lookup.

use super::{T0, entry};
use crate::entry::EntryStatus;
use crate::handoff::{self, HandoffError, HandoffSessionError, MAX_CHARS};
use crate::record::{Author, Related};
use time::Duration;

fn valid_body() -> String {
    "\
## State
Store schema is migrated; `make ci-fast` passes.

## In flight
Uncommitted edits in `crates/scribe-core/src/handoff.rs`.

## Next
Write the integration tests and run `make ci`.

## Waiting on you
Nothing.

## Do not touch
Leave `crates/scribe/tests/goldens/` until the contract review finishes.

## Pointers
- `crates/scribe-core/src/handoff.rs`
- `docs/design/scribe/2_design.md`
"
    .to_owned()
}

#[test]
fn a_well_formed_handoff_passes_including_fenced_headings_inside_sections() {
    assert!(handoff::check(&valid_body()).is_ok());

    let with_fence = format!(
        "{}\n```markdown\n## Not a heading\n## State\n```\n",
        valid_body().trim_end()
    );
    assert!(handoff::check(&with_fence).is_ok());
    assert!(handoff::is_handoff(&with_fence));
}

#[test]
fn a_handoff_over_the_size_limit_is_refused_naming_the_limit() {
    let filler = "a".repeat(MAX_CHARS);
    let body = format!("{}\n{filler}", valid_body());
    let err = handoff::check(&body).unwrap_err();
    assert!(
        matches!(err, HandoffError::TooLong { chars, max } if chars > MAX_CHARS && max == MAX_CHARS),
        "{err:?}"
    );
    assert!(err.to_string().contains(&MAX_CHARS.to_string()), "{err}");

    // Exactly MAX_CHARS is allowed.
    let base = valid_body();
    let pad = MAX_CHARS - base.chars().count();
    let exact = format!("{base}{}", "x".repeat(pad));
    assert_eq!(exact.chars().count(), MAX_CHARS);
    assert!(handoff::check(&exact).is_ok());
}

#[test]
fn missing_headings_are_refused_naming_each_missing_heading() {
    let body = "\
## State
Done.

## Next
Run tests.
";
    let err = handoff::check(body).unwrap_err();
    assert!(
        matches!(err, HandoffError::MissingHeadings(ref missing) if missing == &["In flight", "Waiting on you", "Do not touch", "Pointers"]),
        "{err:?}"
    );
    let msg = err.to_string();
    for heading in ["In flight", "Waiting on you", "Do not touch", "Pointers"] {
        assert!(msg.contains(heading), "{msg}");
    }
}

#[test]
fn out_of_order_headings_are_refused_naming_the_fault() {
    let body = "\
## State
Done.

## Next
Run tests.

## In flight
WIP.

## Waiting on you
Nothing.

## Do not touch
Nothing.

## Pointers
None.
";
    let err = handoff::check(body).unwrap_err();
    assert!(
        matches!(err, HandoffError::OutOfOrder(ref found) if found == &["State", "Next", "In flight", "Waiting on you", "Do not touch", "Pointers"]),
        "{err:?}"
    );
    let msg = err.to_string();
    assert!(msg.contains("out of order"), "{msg}");
    assert!(msg.contains("Next"), "{msg}");
    assert!(msg.contains("In flight"), "{msg}");
}

#[test]
fn preamble_duplicate_and_unknown_headings_are_refused() {
    let with_preamble = format!("Intro text.\n\n{}", valid_body());
    assert!(matches!(
        handoff::check(&with_preamble).unwrap_err(),
        HandoffError::Preamble
    ));

    let with_duplicate = format!("{}\n## State\nAgain.\n", valid_body());
    assert!(matches!(
        handoff::check(&with_duplicate).unwrap_err(),
        HandoffError::DuplicateHeading(ref heading) if heading == "State"
    ));

    let with_unknown = format!("{}\n## Extras\nSomething.\n", valid_body());
    assert!(matches!(
        handoff::check(&with_unknown).unwrap_err(),
        HandoffError::UnknownHeading(ref heading) if heading == "Extras"
    ));
}

#[test]
fn for_session_returns_only_active_handoffs_for_that_session_newest_first() {
    let make = |id: &str, session: &str, mins: i64, status: EntryStatus| {
        let mut e = entry(id, T0 + Duration::minutes(mins));
        e.status = status;
        e.author = Author::agent("opus".parse().unwrap());
        e.related = vec![format!("session:{session}").parse().unwrap()];
        e
    };
    let entries = [
        make("handoff-1", "s-1", 10, EntryStatus::Active),
        make("handoff-2", "s-1", 30, EntryStatus::Active),
        make("handoff-3", "s-1", 40, EntryStatus::Retired),
        make("handoff-4", "s-2", 50, EntryStatus::Active),
        make("preferences-1", "s-1", 60, EntryStatus::Active),
    ];
    let s1: crate::session::SessionId = "s-1".parse().unwrap();
    let found = handoff::for_session(&entries, &s1);
    let ids: Vec<String> = found.iter().map(|e| e.id.to_string()).collect();
    assert_eq!(ids, ["handoff-2", "handoff-1"]);
}

#[test]
fn check_related_requires_exactly_one_valid_session_ref() {
    let rel = |s: &str| -> Related { s.parse().unwrap() };

    assert_eq!(
        handoff::check_related(&[]).unwrap_err(),
        HandoffSessionError::Missing
    );
    assert_eq!(
        handoff::check_related(&[rel("task:ORB-1")]).unwrap_err(),
        HandoffSessionError::Missing
    );
    assert!(matches!(
        handoff::check_related(&[rel("session:../bad")]).unwrap_err(),
        HandoffSessionError::Invalid { ref reference, .. } if reference == "session:../bad"
    ));
    assert!(matches!(
        handoff::check_related(&[rel("session:s-1"), rel("session:../bad")]).unwrap_err(),
        HandoffSessionError::Invalid { ref reference, .. } if reference == "session:../bad"
    ));
    assert_eq!(
        handoff::check_related(&[rel("session:s-1"), rel("session:s-2")]).unwrap_err(),
        HandoffSessionError::Multiple { count: 2 }
    );

    // Single valid session (even repeated or alongside non-session references) succeeds.
    assert_eq!(
        handoff::check_related(&[rel("session:s-1"), rel("task:ORB-1")])
            .unwrap()
            .as_str(),
        "s-1"
    );
    assert_eq!(
        handoff::check_related(&[rel("session:s-1"), rel("session:s-1")])
            .unwrap()
            .as_str(),
        "s-1"
    );
}

#[test]
fn session_refs_changed_detects_any_session_ref_addition_removal_or_swap() {
    let rel = |s: &str| -> Related { s.parse().unwrap() };
    let base = [rel("session:s-1"), rel("task:ORB-1")];

    assert!(!handoff::session_refs_changed(
        &base,
        &[rel("session:s-1"), rel("repo:scribe")]
    ));
    assert!(!handoff::session_refs_changed(&base, &[rel("session:s-1")]));
    assert!(handoff::session_refs_changed(&base, &[rel("task:ORB-1")]));
    assert!(handoff::session_refs_changed(
        &base,
        &[rel("session:s-1"), rel("session:s-2")]
    ));
    assert!(handoff::session_refs_changed(
        &base,
        &[rel("session:s-2"), rel("task:ORB-1")]
    ));
}
