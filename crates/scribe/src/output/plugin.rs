//! Payloads and rendering for `scribe plugin`.

use super::common::console::Sink;
use super::common::table::Result;
use super::consistency::ScanReport;
use serde::Serialize;
use std::io::Write;

/// A failed plugin call, as the envelope's `error` object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PluginError {
    /// A stable `snake_case` code.
    pub(crate) code: &'static str,
    pub(crate) message: String,
    /// Whether the same call may succeed later.
    pub(crate) retryable: bool,
}

/// A plugin tool call's reply: Orbit's exec-backend envelope, one line on
/// stdout whatever happened (`{"ok":true,"output":...}` or
/// `{"ok":false,"error":{...}}`), in every output mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PluginReply {
    /// `scribe.consistency-scan` ran.
    Scan(Box<ScanReport>),
    /// A deterministic tool result.
    Data(serde_json::Value),
    /// The call failed; the envelope carries the code.
    Error(PluginError),
}

/// A plugin call's envelope: one JSON line, whatever the mode, since the
/// Orbit host reads it.
pub(crate) fn emit<O: Write, E: Write>(sink: &mut Sink<O, E>, reply: &PluginReply) -> Result {
    #[derive(Serialize)]
    struct Ok<'a, T: Serialize> {
        ok: bool,
        output: &'a T,
    }
    #[derive(Serialize)]
    struct Failed<'a> {
        ok: bool,
        error: &'a PluginError,
    }
    match reply {
        PluginReply::Scan(report) => sink.emit_text(
            &Ok {
                ok: true,
                output: report.as_ref(),
            },
            |_| String::new(),
        ),
        PluginReply::Data(value) => sink.emit_text(
            &Ok {
                ok: true,
                output: value,
            },
            |_| String::new(),
        ),
        PluginReply::Error(error) => {
            sink.emit_text(&Failed { ok: false, error }, |_| String::new())
        }
    }
}
