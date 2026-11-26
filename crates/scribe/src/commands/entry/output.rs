use super::list::ListArgs;
use crate::commands::{ListNotice, list_output};
use crate::output::{EntryPayload, Output, Record, Records};
use scribe_core::EntryFilter;
use scribe_core::apps::entry::{self, Outcome};
use scribe_core::composition::Context;

/// The output of `outcome`: `window` is `show --window`, and a list
/// echoes `args` as criteria.
pub(super) fn output(
    outcome: Outcome,
    window: bool,
    args: Option<&ListArgs>,
    ctx: &Context,
) -> Output {
    match outcome {
        Outcome::Written(written) => {
            let written = *written;
            let payload = EntryPayload::from(&written.entry).with_action(written.action.as_ref());
            let mut output = Output::written(Record::Entry(Box::new(payload)), written.changed);
            if let Output::Written { notices, .. } = &mut output {
                notices.extend(written.notices);
            }
            output
        }
        Outcome::Shown(shown) => {
            let shown = *shown;
            let actions = Actions(shown.actions);
            Output::Show {
                record: Record::Entry(Box::new(
                    actions
                        .payload(&shown.entry, window)
                        .with_admitted(shown.admitted),
                )),
            }
        }
        Outcome::Listed(listed) => list(*listed, args, ctx),
    }
}

/// `entry list`'s output, echoing `args` as criteria.
fn list(listed: entry::Listed, args: Option<&ListArgs>, ctx: &Context) -> Output {
    let (filter, group, found) = (listed.filter, listed.group, listed.found);
    let empty = format!(
        "no entries in {}; they are recorded by a #scribe line in a Claude Code session",
        ctx.root().display()
    );
    let mut criteria =
        criteria_with_related(&filter, args.map_or(&filter.related, |args| &args.related));
    if let Some(group) = group {
        criteria.push(format!("--group {}", group.id));
    }
    if let Some(args) = args {
        criteria.extend(args.dates.criteria());
    }
    if let Some(after) = args.and_then(|args| args.after)
        && let Ok(text) = after.format(&time::format_description::well_known::Rfc3339)
    {
        criteria.push(format!("--after {text}"));
    }
    let notice = ListNotice {
        noun: "entries",
        criteria,
        empty: &empty,
    };
    let actions = Actions(listed.actions);
    let records = Records::Entries(
        found
            .listing
            .items
            .iter()
            .map(|e| actions.payload(e, args.is_some_and(|args| args.window)))
            .collect(),
    );
    list_output(records, found.listing.total, &found.skipped, &notice)
}

/// The filters applied, as the flags that set them (the dates aside, which
/// the caller adds from its flags).
#[cfg(test)]
pub(in crate::commands) fn criteria(filter: &EntryFilter) -> Vec<String> {
    criteria_with_related(filter, &filter.related)
}

fn criteria_with_related(filter: &EntryFilter, related: &[scribe_core::Related]) -> Vec<String> {
    let mut criteria = Vec::new();
    if let Some(tag) = &filter.tag {
        criteria.push(format!("--tag {tag}"));
    }
    if let Some(status) = filter.status {
        criteria.push(format!("--status {status}"));
    }
    if let Some(project) = &filter.project {
        criteria.push(format!("--project {project}"));
    }
    for topic in &filter.topics {
        criteria.push(format!("--topic {topic}"));
    }
    for related in related {
        criteria.push(format!("--related {related}"));
    }
    if let Some(author) = &filter.author {
        criteria.push(format!("--author {author}"));
    }
    if let Some(kind) = filter.author_kind {
        criteria.push(format!("--author-kind {}", kind.as_str()));
    }
    for state in &filter.states {
        criteria.push(format!("--state {state}"));
    }
    criteria
}

struct Actions(entry::Actions);

impl Actions {
    /// `entry` as output, with its action state when its tag is an action
    /// tag and its window when `window`.
    fn payload(&self, entry: &scribe_core::Entry, window: bool) -> EntryPayload {
        let state = self.0.state(entry);
        let payload = EntryPayload::from(entry).with_action(state.as_ref());
        if window {
            payload.with_window(entry)
        } else {
            payload
        }
    }
}
