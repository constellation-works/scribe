use crate::output::{MemoryRulesPayload, Record};
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::composition::Context;

pub(super) const RULES_EXAMPLES: &str = "\
Prints memory's fixed rules: how many items a pass writes, how many undated
candidates a session holds, how long a now item stays after its date, the
milestone lines shown, the profile's length and the phrases that make an
event a milestone. Memory has no switches.

Examples:
  scribe memory rules
  scribe memory rules --json | jq '.rules[] | {rule, value}'";

/// Arguments to `memory rules`: none.
#[derive(Debug, Args)]
pub(crate) struct RulesArgs {}

pub(super) fn run(RulesArgs {}: RulesArgs, _ctx: &Context) -> Result<Output, CliError> {
    Ok(Output::Show {
        record: Record::MemoryRules(Box::new(MemoryRulesPayload::new())),
    })
}
