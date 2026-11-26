use crate::commands::{ListNotice, list_output};
use crate::output::{Output, Record, Records};
use scribe_core::apps::group::{GroupBackfilled, Outcome};

/// A `group` outcome as its output.
pub(super) fn output(outcome: Outcome) -> Output {
    match outcome {
        Outcome::Written(written) => {
            let written = *written;
            Output::Written {
                record: Record::Group(Box::new(written.group)),
                changed: written.changed,
                notices: written.notices,
            }
        }
        Outcome::Backfilled(report) => backfilled(*report),
        Outcome::Evidence(value) => Output::GroupEvidence(value),
        Outcome::Shown(group) => Output::Show {
            record: Record::Group(group),
        },
        Outcome::Listed(groups) => {
            let total = groups.len();
            list_output(
                Records::Groups(groups),
                total,
                &[],
                &ListNotice {
                    noun: "groups",
                    criteria: Vec::new(),
                    empty: "no session groups in the store",
                },
            )
        }
    }
}

/// What `group backfill` stored.
fn backfilled(report: GroupBackfilled) -> Output {
    Output::GroupBackfill(crate::output::GroupBackfill {
        group: report.group,
        members: report
            .members
            .into_iter()
            .map(|m| crate::output::GroupBackfillMember {
                seat: m.seat,
                session: m.session,
                stored: m.stored,
                skipped: m.skipped,
                transcript_unavailable: m.transcript_unavailable,
            })
            .collect(),
        stored: report.stored,
        skipped: report.skipped,
    })
}
