//! Unit tests for the command modules, one file per source file (STD-02 R19).

mod entry;

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
        notice(&["--tag decisions", "--topic ci"])
            .line(0, 0)
            .as_deref(),
        Some("no entries match --tag decisions --topic ci")
    );
}

#[test]
fn a_cut_list_says_how_many_matched_and_a_whole_one_says_nothing() {
    // STD-01 R34.
    assert_eq!(
        notice(&[]).line(2, 5).as_deref(),
        Some("showing 2 of 5 matching entries; raise or drop --limit to see the rest")
    );
    assert_eq!(notice(&["--tag decisions"]).line(3, 3), None);
}

#[test]
fn only_the_hook_commands_are_audited_because_every_other_command_reads() {
    // STD-01 R31: a read writes nothing, not even an audit line.
    for args in [
        &["scribe", "entry", "list"][..],
        &["scribe", "entry", "show", "decisions-1"],
        &["scribe", "session", "list"],
        &["scribe", "session", "show", "s-1"],
        &["scribe", "proposal", "list"],
        &["scribe", "tag", "list"],
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
    ] {
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.command.audit().map(|m| m.command), Some(path));
    }
}
