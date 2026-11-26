use crate::commands::{ListNotice, list_output};
use crate::output::{Output, Record, Records, TagPayload, TagShowPayload};
use scribe_core::apps::tag::Outcome;

/// A `tag` outcome as its output.
pub(super) fn output(outcome: Outcome) -> Output {
    match outcome {
        Outcome::List(found) => list(found),
        Outcome::Show(shown) => Output::Show {
            record: Record::TagRules(Box::new(TagShowPayload::new(
                &shown.def,
                shown.source,
                &shown.config_file,
            ))),
        },
        Outcome::Added(tag) => Output::written(Record::Tag(tag), true),
        Outcome::Updated(tag, changed) => Output::written(Record::Tag(tag), changed),
    }
}

fn list(found: scribe_core::TagList) -> Output {
    let notice = ListNotice {
        noun: "tags",
        criteria: Vec::new(),
        empty: "no tags: config.toml declares none, so no #scribe line can be filed",
    };
    let total = found.tags.len();
    let records = Records::Tags(found.tags.iter().map(TagPayload::from).collect());
    let mut output = list_output(records, total, &found.skipped, &notice);
    if let Output::List { notices, .. } = &mut output {
        notices.extend(found.warnings);
    }
    output
}
