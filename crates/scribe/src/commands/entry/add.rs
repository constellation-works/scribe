use super::output;
use super::records::RecordArgs;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::entry::{self, Command, exec};
use scribe_core::composition::Context;
use scribe_core::{Project, TagName, Topic};

pub(super) const ADD_EXAMPLES: &str = "\
Examples:
  scribe entry add --tag preferences --author opus 'Use the stable API'
  scribe entry add --tag preferences --author opus --related session:<id> --quote '<their words>' --spoken 'The user prefers short updates.'
  scribe entry add --tag idea --author opus --related session:<id> 'Cache the manifest between runs'
  scribe entry add --tag standard --author opus --related session:<id> --quote '<quoted words>' 'Name the unit in every size'
  scribe entry add --tag decision --author opus --decided-by user --quote '<their words>' --related session:<id> --related task:<id> 'Keep the stable API'
  scribe entry add --tag open-item --author opus --asked-by 0-chief --blocked 'Local checks' --answer-in hand 'Restart the dashboard'";

#[derive(Debug, Args)]
pub(crate) struct AddArgs {
    /// Content to record
    content: String,
    /// Tag name or alias
    #[arg(long)]
    tag: TagName,
    /// Agent name (always recorded as the agent channel; reserved names are refused)
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Project slug
    #[arg(long, default_value = "global")]
    project: Project,
    /// Topic, repeatable (at most five)
    #[arg(long)]
    topic: Vec<Topic>,
    /// Related reference, repeatable (task:, pr:, repo:, doc:, session:); name the source session for checked quotes or the writing session for action tags
    #[arg(long)]
    related: Vec<scribe_core::Related>,
    /// The user's verbatim words from a related session; required for a user decision and by quote tags except a verified self-refresh handoff
    #[arg(long)]
    quote: Option<String>,
    /// Handoff refresh mode: continue or new
    #[arg(long)]
    mode: Option<scribe_core::handoff::Mode>,
    /// Evidence pointer (a path, task, run, experiment or commit); required under a require_evidence tag, refused under any other tag
    #[arg(long, value_name = "POINTER")]
    evidence: Option<String>,
    /// The date a require_evidence entry's result was true, as YYYY-MM-DD [default: today]
    #[arg(long, value_name = "DATE", value_parser = scribe_core::parse_day)]
    as_of: Option<time::Date>,
    /// Record an ordinary memory note on the user's spoken instruction; sets stated_by to --author and requires --quote and --related session:<id>
    #[arg(long)]
    spoken: bool,
    #[command(flatten)]
    record: RecordArgs,
}

pub(super) fn run(args: Box<AddArgs>, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Add(Box::new(entry::AddInput {
        tag: args.tag,
        content: args.content,
        author: args.author,
        project: args.project,
        topics: args.topic,
        related: args.related,
        quote: args.quote,
        mode: args.mode,
        evidence: args.evidence,
        as_of: args.as_of,
        spoken: args.spoken,
        record: args.record.into_fields(),
        provider: ctx.provider(),
        host: ctx.host(),
        current_session: ctx.claude_session(),
    }));
    Ok(output::output(exec(ctx, command)?, false, None, ctx))
}
