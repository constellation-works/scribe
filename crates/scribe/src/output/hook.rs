//! Payloads and rendering for `scribe hook`.

use super::common::console::Sink;
use super::common::table::Result;
use serde::Serialize;
use std::io::Write;

pub(crate) use scribe_core::apps::hook::HookReply;

/// `hookSpecificOutput`.
#[derive(Serialize)]
struct HookSpecificJson<'a> {
    #[serde(rename = "hookEventName")]
    hook_event_name: &'a str,
    #[serde(rename = "additionalContext")]
    additional_context: &'a str,
}

/// The JSON a hook prints (Claude Code's hook output).
#[derive(Serialize)]
struct HookJson<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
    #[serde(rename = "systemMessage", skip_serializing_if = "Option::is_none")]
    system_message: Option<&'a str>,
    #[serde(rename = "hookSpecificOutput", skip_serializing_if = "Option::is_none")]
    hook_specific_output: Option<HookSpecificJson<'a>>,
}

/// A hook's reply: one line of Claude Code hook JSON, or nothing when the
/// hook has nothing to say. The mode is ignored: Claude Code reads this, so
/// the sink it goes through is one that writes compact JSON.
pub(crate) fn emit<O: Write, E: Write>(sink: &mut Sink<O, E>, reply: &HookReply) -> Result {
    if let Some(reason) = &reply.block {
        let json = HookJson {
            decision: Some("block"),
            reason: Some(reason),
            system_message: None,
            hook_specific_output: None,
        };
        return sink.emit_text(&json, |_| String::new());
    }
    if reply.message.is_none() && reply.context.is_none() {
        return Ok(());
    }
    let json = HookJson {
        decision: None,
        reason: None,
        system_message: reply.message.as_deref(),
        hook_specific_output: reply.context.as_deref().map(|context| HookSpecificJson {
            hook_event_name: reply.event,
            additional_context: context,
        }),
    };
    sink.emit_text(&json, |_| String::new())
}
