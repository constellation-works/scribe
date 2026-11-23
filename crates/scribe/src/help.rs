//! Grouped root help, in the shape of Orbit's root page.
//!
//! Each group names commands. The name and the summary printed beside it come
//! from that command's one clap declaration (the doc comment on the noun),
//! never from a second copy of the summary (STD-01 R25). Every group shares
//! one description column, taken from the longest name. A pipe keeps one line
//! per command so the root golden does not depend on the machine (STD-01 R17,
//! R24). A terminal wraps a long summary under that column, to `COLUMNS` or,
//! when that is unset, the terminal's own width.

use clap::Command;

/// Spaces before a command name.
const INDENT: usize = 2;
/// Spaces between the longest name and its summary.
const GAP: usize = 2;

/// One heading and the commands that belong under it, in display order.
pub(crate) struct Group {
    /// Heading printed above the commands, without the trailing colon.
    pub(crate) heading: &'static str,
    /// Command names, each exactly the clap subcommand name.
    pub(crate) commands: &'static [&'static str],
}

/// Root nouns, in the order `scribe --help` shows them.
///
/// `brief` leads Find: it is what a session reads first. `search` sits with Find: it searches stored records, it is not one of them.
/// `transcript` and `tmux` sit there too, as discovery and browse. Clap's
/// built-in `help` subcommand is not a declared noun; it stays runnable and
/// is left off this page, as Orbit's root help leaves it off.
pub(crate) const ROOT_GROUPS: &[Group] = &[
    Group {
        heading: "Setup",
        commands: &["init"],
    },
    Group {
        heading: "Records",
        commands: &["entry", "session", "group", "proposal", "memory", "tag"],
    },
    Group {
        heading: "Find",
        commands: &["brief", "ask", "question", "search", "transcript", "tmux"],
    },
    Group {
        heading: "Upkeep",
        commands: &["consistency", "store", "export"],
    },
    Group {
        heading: "Integration",
        commands: &["hook", "plugin"],
    },
];

/// Install the grouped root template.
///
/// `width` is the sink width: `None` (or `0`) keeps every command on one
/// line. Flag text and the examples footer stay clap's own rendering, read
/// back from the same command, so they are not written out again here.
pub(crate) fn install_root_help(cmd: Command, width: Option<usize>) -> Command {
    let groups = render_groups(&cmd, width);
    let options = options_block(&cmd);
    let mut template =
        String::from("{before-help}{about-with-newline}\n{usage-heading} {usage}\n\n");
    template.push_str(&template_literal(&groups));
    template.push_str("\n\n");
    template.push_str(&template_literal(&options));
    template.push_str("{after-help}");
    cmd.help_template(template)
}

/// The options and global-options sections, without the command list or the
/// examples footer. Taken from clap's own help for this command.
fn options_block(cmd: &Command) -> String {
    let mut bare = cmd
        .clone()
        .disable_help_subcommand(true)
        .mut_subcommands(|sub| sub.hide(true))
        .after_help(None::<&str>)
        .after_long_help(None::<&str>);
    // `scribe --help` and `scribe -h` render the same short flag lines. Long
    // help would put each flag's text on the next line, which this page does not.
    let rendered = bare.render_help().to_string();
    let Some((_, rest)) = rendered.split_once("Options:\n") else {
        return String::from("Options:");
    };
    let mut block = String::from("Options:\n");
    block.push_str(rest);
    // Clap's flag list does not end in a newline; `{after-help}` supplies the
    // blank line before the examples.
    block.truncate(block.trim_end_matches('\n').len());
    block
}

fn render_groups(cmd: &Command, width: Option<usize>) -> String {
    let rows: Vec<(&str, Vec<(&str, String)>)> = ROOT_GROUPS
        .iter()
        .map(|group| {
            let commands = group
                .commands
                .iter()
                .map(|name| (*name, command_about(cmd, name)))
                .collect();
            (group.heading, commands)
        })
        .collect();
    let longest = rows
        .iter()
        .flat_map(|(_, commands)| commands.iter().map(|(name, _)| name_width(name)))
        .max()
        .unwrap_or(0);
    let column = INDENT + longest + GAP;
    let term = match width {
        Some(0) | None => None,
        Some(width) => Some(width),
    };
    rows.iter()
        .map(|(heading, commands)| {
            let mut section = String::from(*heading);
            section.push_str(":\n");
            let lines: Vec<String> = commands
                .iter()
                .flat_map(|(name, about)| render_row(name, about, longest, column, term))
                .collect();
            section.push_str(&lines.join("\n"));
            section
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn command_about(cmd: &Command, name: &str) -> String {
    cmd.find_subcommand(name)
        .and_then(|sub| sub.get_about().or_else(|| sub.get_long_about()))
        .map(ToString::to_string)
        .unwrap_or_default()
}

fn name_width(name: &str) -> usize {
    name.chars().count()
}

fn render_row(
    name: &str,
    about: &str,
    longest: usize,
    column: usize,
    width: Option<usize>,
) -> Vec<String> {
    let mut prefix = " ".repeat(INDENT);
    prefix.push_str(name);
    let pad = longest.saturating_sub(name_width(name)) + GAP;
    prefix.push_str(&" ".repeat(pad));
    match width {
        None => vec![format!("{prefix}{about}")],
        Some(width) if prefix.chars().count() + about.chars().count() <= width => {
            vec![format!("{prefix}{about}")]
        }
        Some(width)
            if width <= column
                || about
                    .split_whitespace()
                    .next()
                    .is_some_and(|word| word.chars().count() > width.saturating_sub(column)) =>
        {
            stack_row(name, about, width)
        }
        Some(width) => wrap_row(&prefix, about, column, width),
    }
}

/// The name on its own line and the summary beneath it, when the column
/// leaves no room for the first word.
fn stack_row(name: &str, about: &str, width: usize) -> Vec<String> {
    let name_indent = INDENT.min(width.saturating_sub(name_width(name)));
    let indent = INDENT.min(width.saturating_sub(1));
    let mut out = vec![format!("{}{name}", " ".repeat(name_indent))];
    let room = width.saturating_sub(indent);
    for part in wrap_text(about, room.max(1)) {
        out.push(format!("{}{part}", " ".repeat(indent)));
    }
    out
}

fn wrap_row(prefix: &str, about: &str, column: usize, width: usize) -> Vec<String> {
    let available = width.saturating_sub(column).max(1);
    let mut out = Vec::new();
    for (index, part) in wrap_text(about, available).into_iter().enumerate() {
        if index == 0 {
            out.push(format!("{prefix}{part}"));
        } else {
            out.push(format!("{}{part}", " ".repeat(column)));
        }
    }
    out
}

fn wrap_text(text: &str, available: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split(' ') {
        let word_len = word.chars().count();
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word_len <= available {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Clap treats `{tag}` in a help template as a placeholder and drops a `{`
/// that has no `}`. Inserted text is literal: a brace pair is an unknown tag
/// that clap prints back, with a word joiner so it cannot match a real tag.
fn template_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start + '{'.len_utf8()..];
        match rest.find('}') {
            Some(end) => {
                out.push('{');
                out.push('\u{2060}');
                out.push_str(&rest[..end]);
                out.push('}');
                rest = &rest[end + '}'.len_utf8()..];
            }
            None => out.push('\u{FF5B}'),
        }
    }
    out.push_str(rest);
    out
}
