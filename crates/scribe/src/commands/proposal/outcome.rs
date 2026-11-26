use crate::commands::{DateArgs, ListNotice, list_output};
use crate::output::{Output, ProposalPayload, Record, Records};
use scribe_core::ProposalFilter;
use scribe_core::apps::proposal::Outcome;
use scribe_core::composition::Context;

/// The output of `outcome`; a list echoes `dates` as criteria.
pub(super) fn output(outcome: Outcome, dates: Option<&DateArgs>, ctx: &Context) -> Output {
    match outcome {
        Outcome::Listed { filter, proposals } => list(&filter, &proposals, dates, ctx),
        Outcome::Shown(proposal) => Output::Show {
            record: Record::Proposal(Box::new(ProposalPayload::from(&*proposal))),
        },
        Outcome::Written(proposal, changed) => written(&proposal, changed),
    }
}

fn list(
    filter: &ProposalFilter,
    proposals: &[scribe_core::Proposal],
    dates: Option<&DateArgs>,
    ctx: &Context,
) -> Output {
    let empty = format!(
        "no proposals in {}; the consistency routine opens them",
        ctx.root().display()
    );
    let mut criteria = Vec::new();
    if let Some(state) = filter.state {
        criteria.push(format!("--state {}", state.as_str()));
    }
    if let Some(dates) = dates {
        criteria.extend(dates.criteria());
    }
    if let Some(entry) = &filter.entry {
        criteria.push(format!("--entry {entry}"));
    }
    if let Some(kind) = filter.author_kind {
        criteria.push(format!("--author-kind {}", kind.as_str()));
    }
    let notice = ListNotice {
        noun: "proposals",
        criteria,
        empty: &empty,
    };
    let total = proposals.len();
    let records = Records::Proposals(proposals.iter().map(ProposalPayload::from).collect());
    list_output(records, total, &[], &notice)
}

fn written(proposal: &scribe_core::Proposal, changed: bool) -> Output {
    Output::written(
        Record::Proposal(Box::new(ProposalPayload::from(proposal))),
        changed,
    )
}
