//! Entry listing, display and audited direct agent writes.

mod add;
mod answer;
mod close;
mod forget;
mod list;
mod output;
mod pickup;
mod records;
mod retire;
mod show;
mod update;

use add::AddArgs;
use answer::AnswerArgs;
use close::CloseArgs;
use forget::ForgetArgs;
use list::ListArgs;
use pickup::PickupArgs;
use retire::RetireArgs;
use show::ShowArgs;
use update::UpdateArgs;

use crate::{error::CliError, output::Output};
use clap::Subcommand;
use clap::builder::{PossibleValuesParser, TypedValueParser};
use scribe_core::composition::Context;
use scribe_core::{EntryStatus, ItemState};

#[cfg(test)]
pub(super) use output::criteria;

pub(super) const ENTRY_EXAMPLES: &str = "\
Examples:
  scribe entry list --tag preferences
  scribe entry list --status active --project scribe
  scribe entry show <id>";

/// Verbs on the `entry` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum EntryCommand {
    /// Add an agent note, decision or open item
    #[command(after_help = add::ADD_EXAMPLES)]
    Add(Box<AddArgs>),
    /// Update an entry's content or metadata
    ///
    /// Updates content or metadata; user entries and approved claims allow metadata only.
    #[command(after_help = update::UPDATE_EXAMPLES)]
    Update(UpdateArgs),
    /// Retire an entry with a reason
    ///
    /// Retires an agent note, or the user's entry on their instruction.
    #[command(after_help = retire::RETIRE_EXAMPLES)]
    Retire(RetireArgs),
    /// Forget an entry for good on the user's instruction
    ///
    /// Blanks the entry's text and keeps its id.
    #[command(after_help = forget::FORGET_EXAMPLES)]
    Forget(ForgetArgs),
    /// List entries, oldest first
    #[command(after_help = list::LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one entry in full: what was said, its content and its cite
    #[command(after_help = show::SHOW_EXAMPLES)]
    Show(ShowArgs),
    /// Record pickup or rejection of an action entry
    ///
    /// Records that a consumer picked up an action entry, or that its result was rejected.
    #[command(after_help = pickup::PICKUP_EXAMPLES)]
    Pickup(PickupArgs),
    /// Record the user's checked answer to an open item
    #[command(after_help = answer::ANSWER_EXAMPLES)]
    Answer(AnswerArgs),
    /// Close an open item once carried out, with its decision or a note
    #[command(after_help = close::CLOSE_EXAMPLES)]
    Close(CloseArgs),
}

/// An RFC 3339 instant.
fn parse_instant(text: &str) -> Result<time::OffsetDateTime, String> {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .map_err(|_| format!("'{text}' is not an RFC 3339 time, such as 2026-11-15T09:00:00Z"))
}

/// Allowed statuses come from the domain type, so help and parsing cannot
/// drift from it (STD-01 R25).
fn status_parser() -> impl TypedValueParser<Value = EntryStatus> {
    PossibleValuesParser::new(EntryStatus::NAMES).try_map(|s| s.parse::<EntryStatus>())
}

/// Allowed open-item states come from the domain type (STD-01 R25).
fn state_parser() -> impl TypedValueParser<Value = ItemState> {
    PossibleValuesParser::new(ItemState::NAMES).try_map(|s| s.parse::<ItemState>())
}

pub(super) fn run(command: EntryCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        EntryCommand::Add(args) => add::run(args, ctx),
        EntryCommand::Update(args) => update::run(args, ctx),
        EntryCommand::Retire(args) => retire::run(args, ctx),
        EntryCommand::Forget(args) => forget::run(args, ctx),
        EntryCommand::List(args) => list::run(args, ctx),
        EntryCommand::Show(args) => show::run(args, ctx),
        EntryCommand::Pickup(args) => pickup::run(args, ctx),
        EntryCommand::Answer(args) => answer::run(args, ctx),
        EntryCommand::Close(args) => close::run(args, ctx),
    }
}
