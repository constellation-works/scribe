use crate::consistency;
use crate::entry::Entry;
use crate::proposal::ProposalKind;
use scribe_store::fixtures::entry;
use time::macros::{date, datetime};
use time::{Duration, OffsetDateTime};

const T0: OffsetDateTime = datetime!(2026-10-01 09:00 UTC);

/// An active entry `id`, recorded `days` after T0, saying `content`.
fn with(id: &str, days: i64, content: &str) -> Entry {
    let mut e = entry(id, T0 + Duration::days(days));
    e.content = content.to_owned();
    e.said = format!("#scribe {}: {content}", e.tag);
    e
}

#[test]
fn a_stale_draft_quotes_the_start_of_the_entry() {
    let mut e = with("preferences-3", 6, "the release train leaves on Thursdays");
    e.review_after = Some(date!(2026 - 11 - 01));
    let draft = consistency::stale(&e);
    assert_eq!(draft.kind, ProposalKind::Stale);
    assert_eq!(draft.newer, None);
    assert_eq!(
        draft.quotes[0].text,
        "the release train leaves on Thursdays"
    );
    assert!(draft.reason.contains("2026-11-01"), "{}", draft.reason);
}
