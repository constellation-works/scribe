//! Claude Code transcripts: the session's JSONL file, read into typed
//! messages (docs/design/scribe/2_design.md §2).
//!
//! Each line of a transcript is one JSON object. The lines this module keeps
//! are `type: "user"` and `type: "assistant"` with a `uuid`, a `timestamp`
//! and a `message` whose `content` is a string or a list of blocks. Only text
//! is kept: tool calls, tool results, thinking and images are dropped, and a
//! line left with no text (a tool result) is not a message. Sidechain lines
//! (a subagent's) and meta lines (`isMeta`, the harness's own) are not the
//! conversation and are dropped too. A line that does not parse, or has a
//! shape this build does not know, is skipped: the file is Claude Code's,
//! and a new line type must not break capture.
//!
//! Bytes in, messages out: no I/O (STD-02 R5).

use crate::entry::Speaker;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// One message of the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// The line's `uuid`.
    pub uuid: String,
    /// Whose words they are.
    pub role: Speaker,
    /// When the line was written.
    pub at: OffsetDateTime,
    /// Its text blocks, in order: one for a string `content`.
    pub blocks: Vec<String>,
}

impl Message {
    /// The whole text, blocks joined by a blank line.
    #[must_use]
    pub fn text(&self) -> String {
        self.blocks.join("\n\n")
    }
}

/// The messages of a transcript, in file order.
#[must_use]
pub fn parse(bytes: &[u8]) -> Vec<Message> {
    bytes
        .split(|&b| b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter_map(|value| message(&value))
        .collect()
}

/// The working directory the transcript's first message was written in
/// (each line carries Claude Code's `cwd`), if any line names one.
#[must_use]
pub fn cwd(bytes: &[u8]) -> Option<String> {
    bytes
        .split(|&b| b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter(|value| message(value).is_some())
        .find_map(|value| {
            value
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.is_empty())
                .map(str::to_owned)
        })
}

fn message(line: &Value) -> Option<Message> {
    let role = match line.get("type")?.as_str()? {
        "user" => Speaker::User,
        "assistant" => Speaker::Assistant,
        _ => return None,
    };
    let flag = |key: &str| line.get(key).and_then(Value::as_bool).unwrap_or(false);
    if flag("isSidechain") || flag("isMeta") {
        return None;
    }
    let uuid = line.get("uuid")?.as_str()?.to_owned();
    let at = OffsetDateTime::parse(line.get("timestamp")?.as_str()?, &Rfc3339).ok()?;
    let blocks = match line.get("message")?.get("content")? {
        Value::String(text) => vec![text.clone()],
        Value::Array(items) => items
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .map(str::to_owned)
            .collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => return None,
    };
    (!blocks.is_empty()).then_some(Message {
        uuid,
        role,
        at,
        blocks,
    })
}
