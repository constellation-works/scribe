//! The `admin` app's outcomes as output: `init`, `store migrate` and
//! `export` (each handler builds its command in its own module).

use crate::output::{ExportPayload, MigratePayload, Output, Record};
use scribe_core::ExportFormat;
use scribe_core::apps::admin::Outcome;

/// An `admin` outcome as its output.
pub(super) fn output(outcome: Outcome) -> Output {
    match outcome {
        Outcome::Initialized(report) => Output::Init(*report),
        Outcome::Migrated(moved) => {
            let moved = *moved;
            Output::written(
                Record::StoreMigrate(Box::new(MigratePayload {
                    sessions: moved.sessions,
                    switches: moved.switches,
                    cursors: moved.cursors,
                    moved_to: moved.moved_to,
                    entries: moved.entries,
                    claims: moved.claims,
                    tags: moved.tags,
                    marks: moved.marks,
                    pickups: moved.pickups,
                    audit_lines: moved.audit_lines,
                    audit_torn: moved.audit_torn,
                    archives: moved.archives,
                    archived: moved.archived,
                })),
                true,
            )
        }
        Outcome::Exported(written) => {
            let written = *written;
            let exported = written.exported;
            Output::Show {
                record: Record::Export(Box::new(ExportPayload {
                    dir: written.dir.display().to_string(),
                    format: match written.format {
                        ExportFormat::Markdown => "markdown",
                        ExportFormat::Jsonl => "jsonl",
                    },
                    sessions: exported.sessions,
                    entries: exported.entries,
                    claims: exported.claims,
                    audit_lines: exported.audit_lines,
                    files: exported.files,
                    skipped: exported
                        .skipped
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect(),
                })),
            }
        }
    }
}
