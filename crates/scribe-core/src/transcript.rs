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
//! Bytes in, messages out: no I/O (STD-02 R5). A survey of the same bytes
//! counts Daniel's turns the way a pass does, for `transcript list`.

use crate::control;
use crate::entry::Speaker;
use crate::record::Project;
use serde_json::Value;
use std::collections::BTreeMap;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// How far `first_prompt` and each preview turn are cut, in characters.
pub const PROMPT_CHARS: usize = 300;
/// Turns `transcript show` returns.
pub const FIRST_TURNS: usize = 5;
/// A transcript whose last message is at least this old has ended.
pub const ENDED_AFTER: time::Duration = time::Duration::hours(24);

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
    if is_codex(bytes) {
        return crate::codex::parse(bytes)
            .map(|r| r.messages)
            .unwrap_or_default();
    }

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
    if is_codex(bytes) {
        return crate::codex::parse(bytes).ok().map(|r| r.cwd);
    }

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

fn is_codex(bytes: &[u8]) -> bool {
    bytes
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .any(|row| {
            matches!(
                row["type"].as_str(),
                Some("session_meta" | "response_item" | "turn_context")
            )
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

/// User turns Claude Code writes itself, not Daniel. A turn whose text
/// starts with one of these tags is dropped, the same rule a pass uses.
const SYNTHETIC: &[&str] = &[
    "<task-notification>",
    "<command-name>",
    "<command-message>",
    "<command-args>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<local-command-caveat>",
    "<system-reminder>",
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
    "<user-prompt-submit-hook>",
];

const PASTED_OPEN: &str = "<pasted_content";
const PASTED_CLOSE: &str = "</pasted_content>";

/// One of Daniel's turns: his own words, and the blocks pasted into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserTurn {
    /// Words he typed, pasted blocks removed.
    pub own: String,
    /// Pasted blocks, in order, trimmed and without the tags.
    pub pasted: Vec<String>,
}

impl UserTurn {
    /// His own words when he typed any, otherwise the pasted text, on one
    /// line and cut to [`PROMPT_CHARS`].
    #[must_use]
    pub fn prompt(&self) -> String {
        let source = if self.own.is_empty() {
            self.pasted.join(" ")
        } else {
            self.own.clone()
        };
        flatten_cut(&source, PROMPT_CHARS)
    }
}

/// Daniel's turn, or `None` when `message` is not one: an assistant line,
/// a synthetic tag, a `/scribe` control line, or a turn with no words.
#[must_use]
pub fn daniel_turn(message: &Message) -> Option<UserTurn> {
    if message.role != Speaker::User {
        return None;
    }
    let text = message.text();
    let start = text.trim_start();
    if SYNTHETIC.iter().any(|tag| start.starts_with(tag)) || control::parse(&text).is_some() {
        return None;
    }
    let (own, pasted) = split_pasted(&text);
    (!own.is_empty() || !pasted.is_empty()).then_some(UserTurn { own, pasted })
}

/// Daniel's turns, in transcript order.
#[must_use]
pub fn daniel_turns(messages: &[Message]) -> Vec<UserTurn> {
    messages.iter().filter_map(daniel_turn).collect()
}

/// A model-free reading of a transcript (docs/design/scribe/2_design.md §5.8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Survey {
    /// The first kept message, if the file has one.
    pub started: Option<OffsetDateTime>,
    /// The last kept message.
    pub ended: Option<OffsetDateTime>,
    /// The file's length.
    pub bytes: u64,
    /// Daniel's turns ([`daniel_turn`]).
    pub daniel_turns: u32,
    /// His first turn, cut to [`PROMPT_CHARS`]. Empty when he has none.
    pub first_prompt: String,
    /// His first [`FIRST_TURNS`] turns, each cut the same way.
    pub first_turns: Vec<String>,
}

/// Survey `bytes`. Times come from kept messages; turn counts use the pass's
/// rule for whose turn is whose.
#[must_use]
pub fn survey(bytes: &[u8]) -> Survey {
    let messages = parse(bytes);
    let turns = daniel_turns(&messages);
    let first_turns: Vec<String> = turns
        .iter()
        .take(FIRST_TURNS)
        .map(UserTurn::prompt)
        .collect();
    Survey {
        started: messages.first().map(|m| m.at),
        ended: messages.last().map(|m| m.at),
        bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        daniel_turns: u32::try_from(turns.len()).unwrap_or(u32::MAX),
        first_prompt: first_turns.first().cloned().unwrap_or_default(),
        first_turns,
    }
}

/// Whether `last` is at least [`ENDED_AFTER`] before `now`. A message in
/// the future has not ended.
#[must_use]
pub fn is_day_old(last: OffsetDateTime, now: OffsetDateTime) -> bool {
    (now - last).max(time::Duration::ZERO) >= ENDED_AFTER
}

/// How often each configured project's paths occur in `bytes`, summed per
/// project. Counts are non-overlapping within one path. Every project is
/// present, including those with a count of zero. Duplicate paths of one
/// project are counted once.
#[must_use]
pub fn mention_counts(
    bytes: &[u8],
    projects: &BTreeMap<Project, Vec<String>>,
) -> BTreeMap<Project, u32> {
    let text = String::from_utf8_lossy(bytes);
    projects
        .iter()
        .map(|(project, paths)| {
            let mut seen = Vec::new();
            let mut count = 0u32;
            for path in paths {
                if path.is_empty() || seen.iter().any(|seen: &String| seen == path) {
                    continue;
                }
                seen.push(path.clone());
                count = count.saturating_add(count_non_overlapping(&text, path));
            }
            (project.clone(), count)
        })
        .collect()
}

/// The project with the highest mention count, or `cwd` when every count is
/// zero. A tie takes the lexicographically smaller slug.
#[must_use]
pub fn attribute(counts: &BTreeMap<Project, u32>, cwd: &Project) -> Project {
    let mut best: Option<(&Project, u32)> = None;
    for (project, count) in counts {
        if *count == 0 {
            continue;
        }
        match best {
            Some((_, n)) if *count > n => best = Some((project, *count)),
            None => best = Some((project, *count)),
            Some(_) => {}
        }
    }
    best.map_or_else(|| cwd.clone(), |(project, _)| project.clone())
}

/// Whitespace collapsed to single spaces, then cut to `limit` characters.
fn flatten_cut(text: &str, limit: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

/// A user turn's own words and the blocks pasted into it. An unclosed block
/// runs to the end of the turn.
fn split_pasted(text: &str) -> (String, Vec<String>) {
    let mut own = String::new();
    let mut pasted = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find(PASTED_OPEN) {
        own.push_str(&rest[..open]);
        let after = &rest[open..];
        let body_start = after.find('>').map_or(after.len(), |i| i + 1);
        let body = &after[body_start..];
        match body.find(PASTED_CLOSE) {
            Some(close) => {
                pasted.push(body[..close].trim().to_owned());
                rest = &body[close + PASTED_CLOSE.len()..];
            }
            None => {
                pasted.push(body.trim().to_owned());
                rest = "";
            }
        }
    }
    own.push_str(rest);
    pasted.retain(|p| !p.is_empty());
    (own.trim().to_owned(), pasted)
}

fn count_non_overlapping(haystack: &str, needle: &str) -> u32 {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0u32;
    let mut rest = haystack;
    while let Some(at) = rest.find(needle) {
        count = count.saturating_add(1);
        rest = &rest[at + needle.len()..];
    }
    count
}
