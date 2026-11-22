//! The `issue` trigger in the prompt hook (docs/design/scribe/2_design.md
//! §11.4): [`fire`] matches the `issue` tag's triggers against Daniel's own
//! words in a typed prompt, without a model, and [`hint`] is the one line
//! the hook then gives the agent. Each fire is one audit line, which
//! `Store::issue_stats` counts. Pure values and no I/O (STD-02 R5).

use crate::tag::{CaptureFrom, CaptureRoute, CaptureRules};
use crate::trigger::{self, Fired};

/// The audit verb of a trigger fire.
pub const FIRE_VERB: &str = "fire";

/// The `issue` trigger that fires in `prompt`, a turn Daniel typed: the
/// first of the rules' triggers, in list order, found in his own words.
/// `None` when the rules are not Daniel's `agent` route, when the turn is a
/// harness or `/scribe` line, or when only pasted, fenced, indented, quoted
/// or `#scribe` lines would match.
#[must_use]
pub fn fire(rules: &CaptureRules, prompt: &str) -> Option<Fired> {
    let from = rules.from.as_ref()?.value;
    let route = rules.route.as_ref()?.value;
    if (from, route) != (CaptureFrom::Daniel, CaptureRoute::Agent) {
        return None;
    }
    let triggers = rules.triggers.as_ref()?;
    let own = crate::transcript::typed_own(prompt)?;
    trigger::first_fired(&triggers.value, &crate::pass::own_lines(&own))
}

/// The line the hook adds to the turn's `additionalContext` when `fired`:
/// that the turn asks for a root cause, the trigger, and the commands.
#[must_use]
pub fn hint(fired: &Fired, session: &str) -> String {
    format!(
        "scribe: this turn asks for a root cause (issue trigger \"{}\" fired). Before diagnosing, run `scribe issue search '<symptoms or error text>'`. When you answer, record it: `scribe issue occur <issue-id>` on a matching issue, else `scribe issue add`, each with `--author <your name> --related session:{session} --request '<the user's words, verbatim>'` (leave `--cause` out when no cause is found: it reads \"cause not found\"), then tell the user in one line the id and that `#scribe forget <issue-id>` cancels it.",
        fired.trigger
    )
}
