//! Unit tests for the command modules, one file per source file (STD-02 R19).

mod entry;
mod search;
mod transcript;

use crate::cli::Cli;
use crate::commands::ListNotice;
use clap::Parser;

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
fn only_the_hook_commands_are_audited_because_every_other_command_reads() {
    // STD-01 R31: a read writes nothing, not even an audit line.
    for args in [
        &["scribe", "entry", "list"][..],
        &["scribe", "entry", "show", "preferences-1"],
        &["scribe", "session", "list"],
        &["scribe", "session", "show", "s-1"],
        &["scribe", "proposal", "list"],
        &["scribe", "tag", "list"],
        &["scribe", "transcript", "list", "--since", "2026-10-01"],
        &["scribe", "transcript", "show", "abcd"],
    ] {
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(cli.command.audit().is_none(), "{args:?}");
    }
}

#[test]
fn both_hook_commands_are_audited_under_their_command_path() {
    for (args, path) in [
        (
            &["scribe", "hook", "user-prompt-submit"][..],
            "hook user-prompt-submit",
        ),
        (&["scribe", "hook", "stop"], "hook stop"),
        (
            &["scribe", "session", "catalog", "abcd", "--author", "daniel"],
            "session catalog",
        ),
    ] {
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.command.audit().map(|m| m.command), Some(path));
    }
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
