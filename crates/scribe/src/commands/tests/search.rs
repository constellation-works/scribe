//! Unit tests for `search`: the hint's command passes each filter value as one
//! literal word, both in a POSIX shell and to the real parser.

use crate::cli::Cli;
use crate::commands::DateArgs;
use crate::commands::search::message_command;
use clap::Parser;
use scribe_core::Project;
use scribe_core::search::SearchQuery;
use std::num::NonZeroUsize;
use std::path::Path;

fn query(project: Option<&str>) -> SearchQuery {
    SearchQuery {
        terms: String::from("quartzlark"),
        kind: None,
        tag: None,
        project: project.map(|p| Project::try_from(p.to_owned()).unwrap()),
        topics: Vec::new(),
        dates: scribe_core::DateRange::default(),
        include_hidden: false,
        limit: NonZeroUsize::MIN,
    }
}

fn no_dates() -> DateArgs {
    DateArgs {
        since: None,
        until: None,
    }
}

/// The words `command` names, split the way `sh` splits them. `printf` is a
/// builtin, so only the shell's own quoting and splitting run.
#[cfg(unix)]
fn shell_words(command: &str) -> Vec<String> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("printf '%s\\n' {command}"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_plain_project_keeps_its_bare_form() {
    let command = message_command(
        Path::new("/store"),
        &query(Some("scribe")),
        None,
        &no_dates(),
    );
    assert!(command.contains(" --project scribe -- "), "{command}");
}

#[cfg(unix)]
#[test]
fn a_hint_passes_each_project_as_one_literal_word() {
    // Labels a shell would split, quote, expand or substitute, and labels clap
    // would read as an option after a bare `--project`. Each substitution is
    // harmless if the quoting regresses: it prints or fails, it runs nothing.
    let labels = [
        "editor's-project",
        "$HOME",
        "$(true)",
        "a`true`b",
        "semi;colon",
        "amp&ersand",
        "star*",
        "question?",
        "quote\"d",
        "back\\slash",
        "pipe|bar",
        "paren(x)",
        "brace{a,b}",
        "tilde~",
        "hash#tag",
        "bang!",
        "lt<gt>",
        "-dash-first",
        "-",
    ];
    for label in labels {
        let command = message_command(Path::new("/store"), &query(Some(label)), None, &no_dates());
        let words = shell_words(&command);

        let value = words
            .windows(2)
            .find(|pair| pair[0] == "--project")
            .map(|pair| pair[1].clone())
            .or_else(|| {
                words
                    .iter()
                    .find_map(|word| word.strip_prefix("--project=").map(str::to_owned))
            })
            .unwrap_or_else(|| panic!("no project word in {command}"));
        assert_eq!(value, label, "{command}");

        // The same words parse as the search the hint names.
        Cli::try_parse_from(words.iter().map(String::as_str))
            .unwrap_or_else(|e| panic!("{command}: {e}"));
    }
}
