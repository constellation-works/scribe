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
//! and a new line type must not break capture. A uuid names one message:
//! Claude Code rewrites earlier lines into the file again (with their uuid,
//! timestamp and text) when a session is resumed, so a line whose uuid an
//! earlier line already used is a replay and only its first copy is kept.
//!
//! Bytes in, messages out: no I/O (STD-02 R5). A survey of the same bytes
//! counts Daniel's turns the way a pass does, for `transcript list`.

use crate::control;
use crate::entry::Speaker;
use crate::record::Project;
use serde::Deserialize;
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
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

/// The messages of a transcript, in file order, each uuid once: a replayed
/// line keeps the position of its first copy.
#[must_use]
pub fn parse(bytes: &[u8]) -> Vec<Message> {
    analyze(bytes).messages
}

/// The working directory the transcript's first message was written in
/// (each line carries Claude Code's `cwd`), if any line names one.
#[must_use]
pub fn cwd(bytes: &[u8]) -> Option<String> {
    analyze(bytes).cwd
}

/// Survey and cwd from a single pass over `bytes`.
#[must_use]
pub fn survey_with_cwd(bytes: &[u8]) -> (Survey, Option<String>) {
    let analysis = analyze(bytes);
    (survey_from(&analysis.messages, bytes.len()), analysis.cwd)
}

struct Analysis {
    messages: Vec<Message>,
    cwd: Option<String>,
}

/// One pass over `bytes`: Codex is detected from the first parseable line,
/// and Claude lines are deserialized into a typed borrowed view, so no
/// `serde_json::Value` DOM is built per line.
fn analyze(bytes: &[u8]) -> Analysis {
    let mut seen = std::collections::HashSet::new();
    let mut messages = Vec::new();
    let mut cwd: Option<String> = None;
    let mut decided = false;
    for line in bytes.split(|&b| b == b'\n') {
        let raw = match serde_json::from_slice::<RawLine<'_>>(line) {
            Ok(raw) => raw,
            Err(_) => {
                if !decided && serde_json::from_slice::<IgnoredAny>(line).is_ok() {
                    decided = true;
                }
                continue;
            }
        };
        if !decided {
            decided = true;
            if matches!(
                raw.kind.as_deref(),
                Some("session_meta" | "response_item" | "turn_context")
            ) {
                return match crate::codex::parse(bytes) {
                    Ok(rollout) => Analysis {
                        messages: rollout.messages,
                        cwd: Some(rollout.cwd),
                    },
                    Err(_) => Analysis {
                        messages: Vec::new(),
                        cwd: None,
                    },
                };
            }
        }
        if let Some(message) = message_from(&raw) {
            if cwd.is_none() {
                cwd = raw
                    .cwd
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
            }
            if seen.insert(message.uuid.clone()) {
                messages.push(message);
            }
        }
    }
    Analysis { messages, cwd }
}

fn message_from(line: &RawLine<'_>) -> Option<Message> {
    let role = match line.kind.as_deref()? {
        "user" => Speaker::User,
        "assistant" => Speaker::Assistant,
        _ => return None,
    };
    if line.is_sidechain.0 || line.is_meta.0 {
        return None;
    }
    let uuid = line.uuid.as_deref()?.to_owned();
    let at = OffsetDateTime::parse(line.timestamp.as_deref()?, &Rfc3339).ok()?;
    let blocks = match line.message.as_ref()?.content.as_ref()? {
        RawContent::Text(text) => vec![text.clone().into_owned()],
        RawContent::Blocks(items) => items
            .iter()
            .filter_map(|block| match block {
                RawBlock::Text(text) => Some(text.clone().into_owned()),
                RawBlock::Other => None,
            })
            .collect(),
        RawContent::Other => return None,
    };
    (!blocks.is_empty()).then_some(Message {
        uuid,
        role,
        at,
        blocks,
    })
}

/// A string field that is `None` when the JSON value is not a string; the
/// rest of the value is consumed without allocation.
#[derive(Default)]
struct MaybeStr<'a>(Option<Cow<'a, str>>);

impl MaybeStr<'_> {
    fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl<'de, 'a> Deserialize<'de> for MaybeStr<'a>
where
    'de: 'a,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V<'a>(std::marker::PhantomData<&'a ()>);
        impl<'de, 'a> Visitor<'de> for V<'a>
        where
            'de: 'a,
        {
            type Value = MaybeStr<'a>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string or any other JSON value")
            }
            fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E> {
                Ok(MaybeStr(Some(Cow::Borrowed(value))))
            }
            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(MaybeStr(Some(Cow::Owned(value.to_owned()))))
            }
            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(MaybeStr(Some(Cow::Owned(value))))
            }
            fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
                Ok(MaybeStr(None))
            }
            fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
                Ok(MaybeStr(None))
            }
            fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
                Ok(MaybeStr(None))
            }
            fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
                Ok(MaybeStr(None))
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(MaybeStr(None))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(MaybeStr(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(MaybeStr(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(MaybeStr(None))
            }
        }
        deserializer.deserialize_any(V(std::marker::PhantomData))
    }
}

/// A boolean flag that is `false` when the JSON value is not a boolean.
#[derive(Clone, Copy, Default)]
struct LooseBool(bool);

impl<'de> Deserialize<'de> for LooseBool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = LooseBool;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a boolean or any other JSON value")
            }
            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(LooseBool(value))
            }
            fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
                Ok(LooseBool(false))
            }
            fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
                Ok(LooseBool(false))
            }
            fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
                Ok(LooseBool(false))
            }
            fn visit_borrowed_str<E>(self, _value: &str) -> Result<Self::Value, E> {
                Ok(LooseBool(false))
            }
            fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
                Ok(LooseBool(false))
            }
            fn visit_string<E>(self, _value: String) -> Result<Self::Value, E> {
                Ok(LooseBool(false))
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(LooseBool(false))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(LooseBool(false))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(LooseBool(false))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(LooseBool(false))
            }
        }
        deserializer.deserialize_any(V)
    }
}

#[derive(Deserialize)]
struct RawLine<'a> {
    #[serde(rename = "type", default, borrow)]
    kind: MaybeStr<'a>,
    #[serde(default, borrow)]
    uuid: MaybeStr<'a>,
    #[serde(default, borrow)]
    timestamp: MaybeStr<'a>,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: LooseBool,
    #[serde(rename = "isMeta", default)]
    is_meta: LooseBool,
    #[serde(default, borrow)]
    cwd: MaybeStr<'a>,
    #[serde(default, borrow)]
    message: Option<RawMessage<'a>>,
}

struct RawMessage<'a> {
    content: Option<RawContent<'a>>,
}

impl<'de, 'a> Deserialize<'de> for RawMessage<'a>
where
    'de: 'a,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V<'a>(std::marker::PhantomData<&'a ()>);
        impl<'de, 'a> Visitor<'de> for V<'a>
        where
            'de: 'a,
        {
            type Value = RawMessage<'a>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a message object or any other JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut content = None;
                while let Some(key) = map.next_key::<Cow<'_, str>>()? {
                    if key.as_ref() == "content" {
                        content = Some(map.next_value::<RawContent<'a>>()?);
                    } else {
                        let _ = map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(RawMessage { content })
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(RawMessage { content: None })
            }
            fn visit_borrowed_str<E>(self, _value: &str) -> Result<Self::Value, E> {
                Ok(RawMessage { content: None })
            }
            fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
                Ok(RawMessage { content: None })
            }
            fn visit_string<E>(self, _value: String) -> Result<Self::Value, E> {
                Ok(RawMessage { content: None })
            }
            fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
                Ok(RawMessage { content: None })
            }
            fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
                Ok(RawMessage { content: None })
            }
            fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
                Ok(RawMessage { content: None })
            }
            fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
                Ok(RawMessage { content: None })
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(RawMessage { content: None })
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(RawMessage { content: None })
            }
        }
        deserializer.deserialize_any(V(std::marker::PhantomData))
    }
}

enum RawContent<'a> {
    Text(Cow<'a, str>),
    Blocks(Vec<RawBlock<'a>>),
    Other,
}

impl<'de, 'a> Deserialize<'de> for RawContent<'a>
where
    'de: 'a,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V<'a>(std::marker::PhantomData<&'a ()>);
        impl<'de, 'a> Visitor<'de> for V<'a>
        where
            'de: 'a,
        {
            type Value = RawContent<'a>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("message content")
            }
            fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E> {
                Ok(RawContent::Text(Cow::Borrowed(value)))
            }
            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(RawContent::Text(Cow::Owned(value.to_owned())))
            }
            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(RawContent::Text(Cow::Owned(value)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut blocks = Vec::new();
                while let Some(block) = seq.next_element::<RawBlock<'a>>()? {
                    blocks.push(block);
                }
                Ok(RawContent::Blocks(blocks))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(RawContent::Other)
            }
            fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
                Ok(RawContent::Other)
            }
            fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
                Ok(RawContent::Other)
            }
            fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
                Ok(RawContent::Other)
            }
            fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
                Ok(RawContent::Other)
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(RawContent::Other)
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(RawContent::Other)
            }
        }
        deserializer.deserialize_any(V(std::marker::PhantomData))
    }
}

enum RawBlock<'a> {
    Text(Cow<'a, str>),
    Other,
}

impl<'de, 'a> Deserialize<'de> for RawBlock<'a>
where
    'de: 'a,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V<'a>(std::marker::PhantomData<&'a ()>);
        impl<'de, 'a> Visitor<'de> for V<'a>
        where
            'de: 'a,
        {
            type Value = RawBlock<'a>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a content block or any other JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut kind: Option<Cow<'_, str>> = None;
                let mut text: Option<Cow<'_, str>> = None;
                while let Some(key) = map.next_key::<Cow<'_, str>>()? {
                    match key.as_ref() {
                        "type" => kind = map.next_value::<MaybeStr<'a>>()?.0,
                        "text" => text = map.next_value::<MaybeStr<'a>>()?.0,
                        _ => {
                            let _ = map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                match (kind.as_deref(), text) {
                    (Some("text"), Some(value)) => Ok(RawBlock::Text(value)),
                    (Some("text"), None) => Ok(RawBlock::Other),
                    (Some(_), _) => Ok(RawBlock::Other),
                    (None, _) => Ok(RawBlock::Other),
                }
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(RawBlock::Other)
            }
            fn visit_borrowed_str<E>(self, _value: &str) -> Result<Self::Value, E> {
                Ok(RawBlock::Other)
            }
            fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
                Ok(RawBlock::Other)
            }
            fn visit_string<E>(self, _value: String) -> Result<Self::Value, E> {
                Ok(RawBlock::Other)
            }
            fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
                Ok(RawBlock::Other)
            }
            fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
                Ok(RawBlock::Other)
            }
            fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
                Ok(RawBlock::Other)
            }
            fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
                Ok(RawBlock::Other)
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(RawBlock::Other)
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(RawBlock::Other)
            }
        }
        deserializer.deserialize_any(V(std::marker::PhantomData))
    }
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
    survey_from(&parse(bytes), bytes.len())
}

fn survey_from(messages: &[Message], len: usize) -> Survey {
    let turns = daniel_turns(messages);
    let first_turns: Vec<String> = turns
        .iter()
        .take(FIRST_TURNS)
        .map(UserTurn::prompt)
        .collect();
    Survey {
        started: messages.first().map(|m| m.at),
        ended: messages.last().map(|m| m.at),
        bytes: u64::try_from(len).unwrap_or(u64::MAX),
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
