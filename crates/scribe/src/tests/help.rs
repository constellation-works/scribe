use std::collections::BTreeSet;
use std::ffi::OsString;

use clap::CommandFactory;
use clap::error::ErrorKind;

use crate::cli::Cli;
use crate::help::{self, ROOT_GROUPS};

/// Every declared root command is in one group, and every grouped name is a
/// command. Clap adds its `help` subcommand only while building, so it is not
/// part of this set; the rendered page leaves it off too.
#[test]
fn root_commands_belong_to_exactly_one_group() {
    let cmd = Cli::command();
    let declared: BTreeSet<&str> = cmd
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .map(|sub| sub.get_name())
        .collect();

    let mut seen = BTreeSet::new();
    for group in ROOT_GROUPS {
        assert!(!group.heading.is_empty(), "a group needs a heading");
        assert!(
            !group.commands.is_empty(),
            "{} lists no commands",
            group.heading
        );
        for name in group.commands {
            assert!(seen.insert(*name), "`{name}` is in more than one group");
            assert!(
                cmd.find_subcommand(name).is_some(),
                "`{name}` is grouped but is not a command"
            );
        }
    }

    let missing: Vec<&str> = declared.difference(&seen).copied().collect();
    assert!(missing.is_empty(), "in no group: {missing:?}");
    let extra: Vec<&str> = seen.difference(&declared).copied().collect();
    assert!(extra.is_empty(), "not a command: {extra:?}");
}

#[test]
fn piped_root_help_uses_one_column_and_the_declaration() {
    let source = Cli::command();
    let mut bare = Cli::command();
    // The shipped root page uses the short flag layout for both `-h` and `--help`.
    let bare_help = bare.render_help().to_string();
    let mut grouped = help::install_root_help(Cli::command(), None);
    let help = grouped.render_long_help().to_string();
    let short = grouped.render_help().to_string();
    assert_eq!(short, help, "-h and --help diverged");

    let bare_head = bare_help.split("\nCommands:\n").next().unwrap();
    let grouped_head = help.split("\nSetup:\n").next().unwrap();
    assert_eq!(grouped_head, bare_head, "about and usage changed");

    let bare_options = bare_help.split("\nOptions:\n").nth(1).unwrap();
    let grouped_options = help.split("\nOptions:\n").nth(1).unwrap();
    assert_eq!(grouped_options, bare_options, "options or examples changed");

    assert!(!help.contains("\nCommands:\n"));
    assert!(!help.contains("\n  help "));

    let mut column = None;
    let mut seen = 0usize;
    for group in ROOT_GROUPS {
        let heading = format!("\n{}:\n", group.heading);
        assert!(help.contains(&heading), "missing {}", group.heading);
        for name in group.commands {
            let needle = format!("\n  {name} ");
            assert_eq!(help.matches(&needle).count(), 1, "`{name}` listed once");
            let about = source
                .find_subcommand(name)
                .and_then(|sub| sub.get_about())
                .map(|text| text.to_string())
                .unwrap();
            let line = help
                .lines()
                .find(|line| line.starts_with(&format!("  {name} ")))
                .unwrap();
            let summary = line.trim_start().strip_prefix(name).unwrap().trim_start();
            assert_eq!(summary, about, "`{name}` summary is not its declaration");
            let at = line.find(about.as_str()).unwrap();
            match column {
                None => column = Some(at),
                Some(column) => assert_eq!(at, column, "`{name}` leaves the description column"),
            }
            seen += 1;
        }
    }
    assert_eq!(
        seen,
        source
            .get_subcommands()
            .filter(|sub| !sub.is_hide_set())
            .count()
    );
    assert!(column.is_some());
}

#[test]
fn narrow_terminal_wraps_under_the_shared_column() {
    let source = Cli::command();
    let about = source
        .find_subcommand("transcript")
        .and_then(|sub| sub.get_about())
        .map(|text| text.to_string())
        .unwrap();
    let mut grouped = help::install_root_help(Cli::command(), Some(40));
    let help = grouped.render_long_help().to_string();
    assert!(
        !help.lines().any(|line| line.contains(&about)),
        "a 40-column terminal should wrap the transcript summary"
    );

    let start = help.find("\n  transcript ").unwrap();
    let mut lines = help[start + 1..].lines();
    let first = lines.next().unwrap();
    // The row ends at the next command. A continuation is indented to the
    // description column, which is deeper than the two-space command indent.
    let lines = lines.take_while(|line| line.starts_with("   "));
    let block = std::iter::once(first)
        .chain(lines)
        .collect::<Vec<_>>()
        .join("\n");
    let mut lines = block.lines();
    let first = lines.next().unwrap();
    let column = first
        .find(about.split_whitespace().next().unwrap())
        .unwrap();
    let mut words = about.split_whitespace();
    for word in first[column..].split_whitespace() {
        assert_eq!(words.next(), Some(word), "first line drifted: {block}");
    }
    for line in lines {
        let Some(text) = line.strip_prefix(&" ".repeat(column)) else {
            panic!("continuation left column {column}: {line:?}");
        };
        assert!(
            text.starts_with(|c: char| !c.is_whitespace()),
            "continuation is blank: {line:?}"
        );
        for word in text.split_whitespace() {
            assert_eq!(words.next(), Some(word), "wrapped words drifted: {block}");
        }
    }
    assert_eq!(
        words.next(),
        None,
        "the wrapped summary dropped words: {block}"
    );
}

#[test]
fn builtin_help_subcommand_still_runs() {
    let args: Vec<OsString> = ["scribe", "help", "entry"]
        .into_iter()
        .map(OsString::from)
        .collect();
    let err = Cli::parse_args(&args, None).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::DisplayHelp);
    assert_eq!(err.exit_code(), 0);
}
