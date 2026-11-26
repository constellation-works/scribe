use super::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::Topic;
use scribe_core::apps::entry::{Command, exec};
use scribe_core::composition::Context;

pub(super) const UPDATE_EXAMPLES: &str =
    "Examples:\n  scribe entry update <id> --author opus --add-topic api";

#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Entry id or unique prefix (at least four characters)
    id: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Replacement content
    #[arg(long)]
    content: Option<String>,
    /// Replace a decision's options in full, repeatable
    #[arg(long = "option")]
    options: Vec<String>,
    /// Clear a decision's options and recommendation
    #[arg(long, conflicts_with_all = ["options", "recommended"])]
    clear_options: bool,
    /// Set the 1-based recommendation for a decision
    #[arg(long)]
    recommended: Option<usize>,
    /// Replace an agent decision's rationale
    #[arg(long)]
    rationale: Option<String>,
    /// Topic to add, repeatable
    #[arg(long)]
    add_topic: Vec<Topic>,
    /// Topic to remove, repeatable
    #[arg(long)]
    remove_topic: Vec<Topic>,
    /// Reference to add, repeatable
    #[arg(long)]
    add_related: Vec<scribe_core::Related>,
    /// Reference to remove, repeatable
    #[arg(long)]
    remove_related: Vec<scribe_core::Related>,
}

pub(super) fn run(args: UpdateArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Update {
        id: args.id,
        patch: Box::new(scribe_core::EntryPatch {
            content: args.content,
            decision_options: (args.clear_options || !args.options.is_empty())
                .then_some(args.options),
            decision_recommended: args.recommended,
            decision_rationale: args.rationale,
            add_topics: args.add_topic,
            remove_topics: args.remove_topic,
            add_related: args.add_related,
            remove_related: args.remove_related,
        }),
        author: args.author,
        provider: ctx.provider(),
    };
    Ok(output::output(exec(ctx, command)?, false, None, ctx))
}
