//! Command dispatch: the noun enum, the one chokepoint every command runs
//! through, and one module per noun.
//!
//! [`run`] is where cross-cutting behaviour attaches, once, for every command
//! (STD-02 R24): today the audit declaration each app's `exec` audits under.
//! Each noun lives in its own module with its subcommands, flags, help and
//! handlers (`consistency.rs`, `brief.rs`, `store.rs`, `export.rs`,
//! `plugin.rs`). Nouns with four or more verbs use a directory with
//! `<noun>/mod.rs` and one file per verb, as `entry/`, `group/`, `memory/`,
//! `proposal/` and `tag/` do. `hook/` is different: `hook/mod.rs` declares
//! and dispatches all four event verbs; its sibling files provide helpers
//! rather than one file per verb (STD-02 R18, R49).

pub(in crate::commands) mod brief;
mod consistency;
mod entry;
mod export;
mod group;
mod hook;
mod init;
mod instruction;
mod mark;
pub(in crate::commands) mod memory;
mod plugin;
mod proposal;
mod search;
mod session;
mod store;
mod tag;
mod transcript;

use crate::error::CliError;
use crate::output::{Output, Records, WarningPayload};
use clap::{Args, Subcommand};
#[cfg(test)]
pub(crate) use scribe_core::RunLimits;
use scribe_core::composition::Context;
use scribe_core::{AuthorKind, DateRange, Skipped};
use time::Date;

pub(crate) use consistency::ConsistencyCommand;
pub(crate) use entry::EntryCommand;
pub(crate) use hook::HookCommand;
pub(crate) use memory::MemoryCommand;
pub(crate) use plugin::PluginCommand;
pub(crate) use proposal::ProposalCommand;
pub(crate) use session::SessionCommand;
pub(crate) use store::StoreCommand;
pub(crate) use tag::TagCommand;

/// Top-level nouns.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Scaffold the embedded skill and selected provider hooks
    ///
    /// Requires --provider, preserves existing edits, and never trusts hooks.
    Init(init::InitArgs),
    /// Read entries and write agent notes
    #[command(subcommand, after_help = entry::ENTRY_EXAMPLES)]
    Entry(EntryCommand),
    /// Group sessions into named seats for a shared piece of work
    #[command(subcommand, after_help = group::EXAMPLES)]
    Group(group::GroupCommand),
    /// List, show and write session records cataloged with /scribe on
    ///
    /// Lists and shows cataloged session records, and writes them with pass and catalog.
    #[command(subcommand, after_help = session::SESSION_EXAMPLES)]
    Session(SessionCommand),
    /// List uncataloged provider transcripts and show one, writing nothing
    #[command(subcommand, after_help = transcript::EXAMPLES)]
    Transcript(transcript::TranscriptCommand),
    /// List the #scribe markers not cataloged yet and retry them
    ///
    /// Lists the #scribe markers whose model call is pending or gave up, and catalogs a session's again from a shell with model credentials.
    #[command(subcommand, after_help = mark::EXAMPLES)]
    Mark(mark::MarkCommand),
    /// Read, open, update and settle proposals
    #[command(subcommand, after_help = proposal::PROPOSAL_EXAMPLES)]
    Proposal(ProposalCommand),
    /// Show what scribe noticed about the user and manage memory items
    ///
    /// Shows what scribe has noticed about the user from switched-on sessions, and lets you acknowledge, retire or forget an item.
    #[command(subcommand, after_help = memory::MEMORY_EXAMPLES)]
    Memory(MemoryCommand),
    /// Search every stored record by words, with no model call
    ///
    /// Ranks entries, session headers and segments for words, best first, with no model call.
    #[command(after_help = search::SEARCH_EXAMPLES)]
    Search(search::SearchArgs),
    /// Propose retiring entries due for review and deactivate stale sessions
    ///
    /// Proposes retiring entries due for review and deactivates sessions past the age limit, with no model call.
    #[command(subcommand, after_help = consistency::CONSISTENCY_EXAMPLES)]
    Consistency(ConsistencyCommand),
    /// Import the file store's archives into scribe.db
    ///
    /// Imports the pre-sqlite-<date>/ and pre-issue-<date>/ archives into scribe.db's archive tables now, then removes them; every write does it on its own.
    #[command(subcommand, after_help = store::EXAMPLES)]
    Store(StoreCommand),
    /// Write every session to a directory as Markdown or JSONL
    ///
    /// Writes every session to a directory you name, as Markdown in the old record files' shape or as JSONL, one line per row; the backup and the way out, writing nothing to the store.
    #[command(after_help = export::EXAMPLES)]
    Export(export::ExportArgs),
    /// List tags and manage agent-created store tags
    #[command(subcommand, after_help = tag::TAG_EXAMPLES)]
    Tag(TagCommand),
    /// Show the session-start page: memory, open items, sessions, claims
    ///
    /// Shows the session-start page the hook injects: the memory page, the open items across projects, the last sessions (this project's first) and every open claim grouped by tag with the id that settles it; model-free, writing nothing.
    #[command(after_help = brief::EXAMPLES)]
    Brief(brief::BriefArgs),
    /// Run a provider hook for session and prompt events
    ///
    /// Runs a provider hook to record #scribe lines and settle proposals, switch /scribe on and off, mark session passes due, and report open proposals.
    #[command(subcommand, after_help = hook::HOOK_EXAMPLES)]
    Hook(HookCommand),
    /// Serve the Orbit plugin's tools (the plugin backend runs this)
    #[command(subcommand, after_help = plugin::PLUGIN_EXAMPLES)]
    Plugin(PluginCommand),
}

/// What a list needs to say on stderr beside its records.
pub(super) struct ListNotice<'a> {
    /// The plural noun, such as `entries`.
    pub(super) noun: &'static str,
    /// The filters applied, as the flags that set them (`--tag preferences`).
    pub(super) criteria: Vec<String>,
    /// The line for an empty store with no filters.
    pub(super) empty: &'a str,
}

impl ListNotice<'_> {
    /// The stderr line for a list, if it needs one: nothing matched (STD-01
    /// R16, naming what was searched), or the limit cut the list (R34). A
    /// full, non-empty list has none.
    pub(super) fn line(&self, shown: usize, total: usize) -> Option<String> {
        if total == 0 && self.criteria.is_empty() {
            Some(self.empty.to_owned())
        } else if total == 0 {
            Some(format!(
                "no {} match {}",
                self.noun,
                self.criteria.join(" ")
            ))
        } else if shown < total {
            Some(format!(
                "showing {shown} of {total} matching {}; raise or drop --limit to see the rest",
                self.noun
            ))
        } else {
            None
        }
    }
}

/// `--since` and `--until`, shared by every list and the search.
#[derive(Debug, Args)]
pub(crate) struct DateArgs {
    /// Only records on or after this day, YYYY-MM-DD [default: no lower bound]
    #[arg(long, value_name = "DATE", value_parser = day_parser)]
    pub(crate) since: Option<Date>,

    /// Only records on or before this day, YYYY-MM-DD [default: no upper bound]
    #[arg(long, value_name = "DATE", value_parser = day_parser)]
    pub(crate) until: Option<Date>,
}

impl DateArgs {
    /// The range, or a usage error when it ends before it starts.
    pub(crate) fn range(&self) -> Result<DateRange, CliError> {
        DateRange::checked(self.since, self.until).map_err(CliError::Usage)
    }

    /// The flags that set the range, for a list's "no match" line.
    pub(crate) fn criteria(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(since) = self.since {
            out.push(format!("--since {}", scribe_core::DateRange::day(since)));
        }
        if let Some(until) = self.until {
            out.push(format!("--until {}", scribe_core::DateRange::day(until)));
        }
        out
    }
}

/// The channel an entry was written through, or a proposal opened through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(in crate::commands) enum AuthorKindArg {
    // The user, through a typed #scribe line or an approval.
    User,
    // An agent, through a CLI write verb.
    Agent,
    // scribe itself, through its session pass or consistency routine.
    Scribe,
}

impl From<AuthorKindArg> for AuthorKind {
    fn from(kind: AuthorKindArg) -> Self {
        match kind {
            AuthorKindArg::User => Self::User,
            AuthorKindArg::Agent => Self::Agent,
            AuthorKindArg::Scribe => Self::Scribe,
        }
    }
}

/// Display names for session ids in non-JSON output, unique across stored
/// sessions, group members and local transcripts. Cataloged sessions use their
/// stable aliases; others use prefixes. Failed reads leave full ids available.
pub(crate) fn short_ids(output: &Output, ctx: &Context) -> scribe_core::ShortIds {
    use crate::output::Record;
    let prints_sessions = match output {
        Output::List { records, .. } => matches!(
            records,
            Records::Sessions { .. }
                | Records::SessionTree { .. }
                | Records::Transcripts(_)
                | Records::Marks(_)
                | Records::Groups(_)
                | Records::Hits(_)
                | Records::Proposals(_)
        ),
        Output::Pass(_) | Output::Scan(_) | Output::GroupBackfill(_) => true,
        Output::Show { record } | Output::Written { record, .. } => {
            matches!(
                record,
                Record::Group(_)
                    | Record::Entry(_)
                    | Record::Proposal(_)
                    | Record::Session(_)
                    | Record::SessionHandoffs(_)
                    | Record::Transcript(_)
            )
        }
        Output::Init(_)
        | Output::Hook(_)
        | Output::Plugin(_)
        | Output::PassStep(_)
        | Output::GroupEvidence(_) => false,
    };
    if !prints_sessions {
        return scribe_core::ShortIds::default();
    }
    let mut extra = Vec::new();
    if let Output::Show {
        record: Record::Group(group),
    }
    | Output::Written {
        record: Record::Group(group),
        ..
    } = output
    {
        extra.extend(group.members().iter().map(|id| id.to_string()));
        extra.push(group.opener.clone());
    }
    if let Output::List {
        records: Records::Groups(groups),
        ..
    } = output
    {
        extra.extend(
            groups
                .iter()
                .flat_map(|g| g.members().into_iter().map(|id| id.to_string())),
        );
        extra.extend(groups.iter().map(|g| g.opener.clone()));
    }
    ctx.short_ids(extra)
}

/// Run one command: its arguments mapped to its app's command, whose `exec`
/// declares whether it is audited and runs it inside the audit guard.
pub(crate) fn run(command: Command, ctx: &Context) -> Result<Output, CliError> {
    match command {
        Command::Init(args) => init::run(args, ctx),
        Command::Group(command) => group::run(command, ctx),
        Command::Entry(command) => entry::run(command, ctx),
        Command::Session(command) => session::run(command, ctx),
        Command::Transcript(command) => transcript::run(command, ctx),
        Command::Mark(command) => mark::run(command, ctx),
        Command::Proposal(command) => proposal::run(command, ctx),
        Command::Memory(command) => memory::run(command, ctx),
        Command::Search(args) => search::run(args, ctx),
        Command::Consistency(command) => consistency::run(command, ctx),
        Command::Store(command) => store::run(command, ctx),
        Command::Export(args) => export::run(&args, ctx),
        Command::Tag(command) => tag::run(command, ctx),
        Command::Hook(command) => hook::run(command, ctx),
        Command::Brief(args) => brief::run(&args, ctx),
        Command::Plugin(command) => plugin::run(command),
    }
}

/// `YYYY-MM-DD`; anything else is a usage error (exit 2).
pub(crate) fn day_parser(text: &str) -> Result<Date, String> {
    scribe_core::parse_day(text)
}

/// A list's output: the records, the files skipped, and its notice.
pub(super) fn list_output(
    records: Records,
    total: usize,
    skipped: &[Skipped],
    notice: &ListNotice<'_>,
) -> Output {
    Output::List {
        notices: notice.line(records.len(), total).into_iter().collect(),
        records,
        total,
        warnings: skipped.iter().map(WarningPayload::from).collect(),
        message_matches: None,
    }
}

#[cfg(test)]
mod tests;
