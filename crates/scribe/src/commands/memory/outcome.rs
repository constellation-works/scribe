use crate::output::{
    MemoryBlockPayload, MemoryPayload, Output, Record, RefreshPayload, RefreshRetryPayload,
    RevisionPayload,
};
use scribe_core::apps::memory::{ApplyRefreshOutcome, Outcome, Shown};
use scribe_core::composition::Context;

/// The output of `outcome`.
pub(super) fn output(outcome: Outcome, ctx: &Context) -> Output {
    match outcome {
        Outcome::Show(shown) => match *shown {
            Shown::Revision(revision) => Output::Show {
                record: Record::MemoryRevision(Box::new(RevisionPayload {
                    revision: revision.n,
                    created: revision.created,
                    text: revision.content,
                })),
            },
            Shown::Item(shown) => Output::Show {
                record: Record::Memory(Box::new(MemoryPayload::from_shown(&shown))),
            },
            Shown::Block(view) => Output::Show {
                record: Record::MemoryBlock(Box::new(MemoryBlockPayload::from_view(
                    &view,
                    ctx.root(),
                ))),
            },
        },
        Outcome::Acked(item, changed) => {
            Output::written(Record::Memory(Box::new(MemoryPayload::now(&item))), changed)
        }
        Outcome::Forgotten(shown) => Output::written(
            Record::Memory(Box::new(MemoryPayload::from_shown(&shown))),
            true,
        ),
        Outcome::Restored(shown, written) => Output::written(
            Record::Memory(Box::new(MemoryPayload::from_shown(&shown))),
            written,
        ),
        Outcome::RefreshedNothingNew => Output::written(
            Record::MemoryRefresh(Box::new(RefreshPayload {
                result: "nothing_new",
                revision: None,
                sessions: 0,
                backlog: 0,
                milestones: Vec::new(),
                removed: Vec::new(),
            })),
            false,
        ),
        Outcome::Prepared(prepared) => Output::Show {
            record: Record::MemoryPrepare(prepared),
        },
        Outcome::Applied(outcome) => match *outcome {
            ApplyRefreshOutcome::Written(written) => Output::written(
                Record::MemoryRefresh(Box::new(RefreshPayload {
                    result: "written",
                    revision: written.revision,
                    sessions: written.sessions,
                    backlog: written.backlog,
                    milestones: written.milestones,
                    removed: written.removed,
                })),
                true,
            ),
            ApplyRefreshOutcome::Retry { code, correction } => Output::Show {
                record: Record::MemoryRefreshRetry(Box::new(RefreshRetryPayload {
                    status: "retry",
                    code,
                    correction,
                })),
            },
        },
    }
}
