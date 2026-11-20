use crate::commands::entry::criteria;
use scribe_core::{AuthorKind, EntryFilter, EntryStatus};
use std::num::NonZeroUsize;

#[test]
fn criteria_are_the_flags_that_set_each_filter_and_never_the_limit() {
    assert!(criteria(&EntryFilter::default()).is_empty());
    let filter = EntryFilter {
        tag: Some("preferences".parse().unwrap()),
        status: Some(EntryStatus::Retired),
        project: Some("orbit".parse().unwrap()),
        topics: vec!["ci".parse().unwrap(), "release".parse().unwrap()],
        related: vec![String::from("task:t-1").try_into().unwrap()],
        author: Some("opus".parse().unwrap()),
        author_kind: Some(AuthorKind::Agent),
        limit: NonZeroUsize::new(3),
        ..EntryFilter::default()
    };
    assert_eq!(
        criteria(&filter),
        [
            "--tag preferences",
            "--status retired",
            "--project orbit",
            "--topic ci",
            "--topic release",
            "--related task:t-1",
            "--author opus",
            "--author-kind agent",
        ]
    );
}
