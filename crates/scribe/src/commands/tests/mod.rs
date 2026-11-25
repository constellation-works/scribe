//! Unit tests for the command modules, one file per source file (STD-02 R19).

mod entry;
mod search;
mod transcript;

use crate::commands::ListNotice;

fn notice(criteria: &[&str]) -> ListNotice<'static> {
    ListNotice {
        noun: "entries",
        criteria: criteria.iter().map(|c| (*c).to_owned()).collect(),
        empty: "no entries in the store",
    }
}

#[test]
fn an_empty_store_says_where_records_come_from() {
    assert_eq!(
        notice(&[]).line(0, 0).as_deref(),
        Some("no entries in the store")
    );
}

#[test]
fn an_empty_filtered_list_names_the_filters() {
    // STD-01 R16.
    assert_eq!(
        notice(&["--tag preferences", "--topic ci"])
            .line(0, 0)
            .as_deref(),
        Some("no entries match --tag preferences --topic ci")
    );
}

#[test]
fn a_cut_list_says_how_many_matched_and_a_whole_one_says_nothing() {
    // STD-01 R34.
    assert_eq!(
        notice(&[]).line(2, 5).as_deref(),
        Some("showing 2 of 5 matching entries; raise or drop --limit to see the rest")
    );
    assert_eq!(notice(&["--tag preferences"]).line(3, 3), None);
}

#[test]
fn run_limits_cap_the_units_and_start_only_the_first_after_the_deadline() {
    use crate::commands::RunLimits;
    use std::time::{Duration, Instant};
    let open = RunLimits::new(2);
    assert!(open.may_start(0) && open.may_start(1));
    assert!(!open.may_start(2), "at most max");
    let Some(past) = Instant::now().checked_sub(Duration::from_secs(1)) else {
        return;
    };
    let late = RunLimits::new(5).start_by(past);
    assert!(late.may_start(0), "the first unit always starts");
    assert!(!late.may_start(1), "no later unit after the deadline");
    let early = RunLimits::new(5).start_by(Instant::now() + Duration::from_secs(600));
    assert!(early.may_start(4));
}
