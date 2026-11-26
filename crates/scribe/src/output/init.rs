//! Payloads and rendering for `scribe init`.

use super::common::console::Sink;
use super::common::table::Result;
use std::io::Write;

/// Render scaffold artifacts and actionable next steps without claiming capture.
pub(crate) fn emit<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    report: &scribe_core::apps::init::InitReport,
) -> Result {
    sink.emit_text(report, |report| {
        let mut text = String::new();
        for file in &report.files {
            text.push_str(&format!("{}\t{}\n", file.status, file.path.display()));
        }
        for step in &report.next {
            text.push_str(step);
            text.push('\n');
        }
        text
    })
}
