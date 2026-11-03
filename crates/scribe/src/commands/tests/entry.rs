use crate::commands::entry::criteria;
use scribe_core::{EntryFilter, EntryStatus};
use std::num::NonZeroUsize;

#[test]
fn criteria_are_the_flags_that_set_each_filter_and_never_the_limit() {
    assert!(criteria(&EntryFilter::default()).is_empty());
    let filter = EntryFilter {
        tag: Some("decisions".parse().unwrap()),
        status: Some(EntryStatus::Retired),
        project: Some("orbit".parse().unwrap()),
        topic: Some("ci".parse().unwrap()),
        limit: NonZeroUsize::new(3),
    };
    assert_eq!(
        criteria(&filter),
        [
            "--tag decisions",
            "--status retired",
            "--project orbit",
            "--topic ci"
        ]
    );
}
