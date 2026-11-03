//! `#scribe` capture: finding the instructions in a typed turn and parsing
//! each one with the model-free grammar (docs/design/scribe/2_design.md §4.1,
//! §4.2; 4_decisions.md, "The #scribe grammar is model-free").
//!
//! [`scan`] reads a prompt line by line. A line is an instruction when it
//! starts with `#scribe` (after optional leading whitespace) and is not inside
//! a code fence, a blockquote or a `<pasted_content>` block, so a pasted log or
//! a quoted message that happens to contain `#scribe` never writes. A bare
//! `#decision` or any other hashtag is not an instruction. Each instruction is
//! parsed on its own, so a turn with two lines yields two results.
//!
//! Forms handled here (no model):
//!
//! 1. `#scribe <tag>: <content>`
//! 2. `#scribe [add|record|note] <content> (to|in|under) [my|the] <tag>`
//!
//! Hashtags at the end of the line are the entry's topics
//! (`#scribe decisions: squash-merge agent PRs #merge-policy`). A referent
//! (`#scribe add this to decisions`, form 3) and the settling verbs
//! (`approve`, `reject`, `retire`, `forget`, form 4) are recognised and
//! refused as not supported by this build, never misread as form 2.
//!
//! Pure functions over text: no I/O (STD-02 R5).

use crate::entry::{Cite, ContentFrom, NewEntry, TagName};
use crate::record::{Label, Project, Topic, Topics};
use crate::session::SessionId;
use crate::tag::TagTable;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fmt::Write as _;
use time::OffsetDateTime;

/// The marker that opens an instruction line.
pub const MARKER: &str = "#scribe";

/// Words that point at an earlier message instead of giving the content
/// (form 3, resolved by a model in a later slice).
const REFERENTS: [&str; 4] = ["this", "that", "it", "these"];
/// The optional verbs that open form 2.
const VERBS: [&str; 3] = ["add", "record", "note"];
/// The prepositions before the tag in form 2.
const PREPOSITIONS: [&str; 3] = ["to", "in", "under"];
/// The optional determiners before the tag in form 2.
const DETERMINERS: [&str; 2] = ["my", "the"];
/// Verbs that settle a proposal or hide an entry (form 4).
const SETTLING: [&str; 4] = ["approve", "reject", "retire", "forget"];

/// The forms, as the refusal of an unparsed instruction lists them.
pub const FORMS: &str = "\"#scribe <tag>: <content>\" or \"#scribe add <content> to <tag>\"";

/// One `#scribe` line that parsed: where it goes and what it records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    /// The line's number within the prompt, from 1.
    pub line: u32,
    /// The whole line exactly as typed, without its line break.
    pub said: String,
    /// The tag it resolved to.
    pub tag: TagName,
    /// What it records: a verbatim substring of `said`.
    pub content: String,
    /// The hashtags at the end of the line.
    pub topics: Topics,
}

/// Why a `#scribe` line was not recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The tag word matched no configured name or alias.
    UnknownTag {
        /// The word as typed.
        word: String,
        /// Every configured tag, for the reply.
        tags: Vec<TagName>,
    },
    /// The line matched no form.
    Unparsed,
    /// The line uses a form this build does not handle yet.
    Unsupported {
        /// Which form, for the reply.
        form: &'static str,
    },
    /// More than [`Topics::MAX`] distinct hashtags.
    TooManyTopics,
}

impl Refusal {
    /// The stable code the audit log and the reply carry (STD-01 R19).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTag { .. } => "unknown_tag",
            Self::Unparsed | Self::TooManyTopics => "unparsed_instruction",
            Self::Unsupported { .. } => "unsupported_instruction",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTag { word, tags } => {
                let names: Vec<&str> = tags.iter().map(TagName::as_str).collect();
                write!(f, "no tag \"{word}\"; tags: {}", names.join(", "))
            }
            Self::Unparsed => write!(f, "no form matches; use {FORMS}"),
            Self::Unsupported { form } => write!(
                f,
                "{form} is not supported yet; write the content itself: {FORMS}"
            ),
            Self::TooManyTopics => {
                write!(f, "at most {} topic hashtags fit on one entry", Topics::MAX)
            }
        }
    }
}

/// A `#scribe` line that was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// The line's number within the prompt, from 1.
    pub line: u32,
    /// The whole line exactly as typed.
    pub said: String,
    /// Why.
    pub reason: Refusal,
}

/// The result for one `#scribe` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    /// It parsed and can be recorded.
    Instruction(Instruction),
    /// It was refused.
    Refused(Refused),
}

impl Parsed {
    /// The line's number within the prompt.
    #[must_use]
    pub fn line(&self) -> u32 {
        match self {
            Self::Instruction(i) => i.line,
            Self::Refused(r) => r.line,
        }
    }
}

/// Every `#scribe` instruction line of `prompt`, in order, each parsed
/// against `tags`. Lines in code fences, blockquotes and
/// `<pasted_content>` blocks are skipped.
#[must_use]
pub fn scan(prompt: &str, tags: &TagTable) -> Vec<Parsed> {
    lines(prompt)
        .into_iter()
        .map(|(line, said)| parse(line, said, tags))
        .collect()
}

/// The instruction lines of `prompt`, unparsed: `(number from 1, the line
/// as typed)`. Lines in code fences, blockquotes and `<pasted_content>`
/// blocks are skipped. Empty for a prompt with no `#scribe` line, which
/// is most of them.
#[must_use]
pub fn lines(prompt: &str) -> Vec<(u32, &str)> {
    let mut found = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut pasted = false;
    for (index, raw) in prompt.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = line.trim_start();
        if pasted {
            if line.contains("</pasted_content>") {
                pasted = false;
            }
            continue;
        }
        if let Some((mark, len)) = fence {
            if fence_marker(trimmed).is_some_and(|(m, l)| m == mark && l >= len)
                && trimmed.trim_start_matches(mark).trim().is_empty()
            {
                fence = None;
            }
            continue;
        }
        if let Some(open) = fence_marker(trimmed) {
            fence = Some(open);
            continue;
        }
        if let Some(at) = line.find("<pasted_content") {
            pasted = !line[at..].contains("</pasted_content>");
            continue;
        }
        if trimmed.starts_with('>') {
            continue;
        }
        if is_instruction(trimmed) {
            let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
            found.push((number, line));
        }
    }
    found
}

/// A fence opener or closer: three or more backticks or tildes.
fn fence_marker(trimmed: &str) -> Option<(char, usize)> {
    let mark = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = trimmed.chars().take_while(|&c| c == mark).count();
    (len >= 3).then_some((mark, len))
}

fn is_instruction(trimmed: &str) -> bool {
    trimmed
        .strip_prefix(MARKER)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// A word of the line and where it sits, so content can be cut from the line
/// itself and stay a verbatim substring of `said`.
#[derive(Clone, Copy)]
struct Word<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

fn words(text: &str) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                out.push(Word {
                    text: &text[s..i],
                    start: s,
                    end: i,
                });
                start = None;
            }
            (false, None) => start = Some(i),
            (true, None) | (false, Some(_)) => {}
        }
    }
    if let Some(s) = start {
        out.push(Word {
            text: &text[s..],
            start: s,
            end: text.len(),
        });
    }
    out
}

fn is(word: &str, set: &[&str]) -> bool {
    let lower = word.to_lowercase();
    set.contains(&lower.as_str())
}

/// Parse one instruction line (already known to start with the marker).
fn parse(line: u32, said: &str, tags: &TagTable) -> Parsed {
    let refuse = |reason| {
        Parsed::Refused(Refused {
            line,
            said: said.to_owned(),
            reason,
        })
    };
    let all = words(said);
    // all[0] is the marker itself.
    let mut body: Vec<Word<'_>> = all.iter().skip(1).copied().collect();

    // Trailing hashtags are topics.
    let mut topics: Vec<Topic> = Vec::new();
    while let Some(last) = body.last() {
        let Some(topic) = last
            .text
            .strip_prefix('#')
            .and_then(|t| Topic::try_from(t.to_lowercase()).ok())
        else {
            break;
        };
        if !topics.contains(&topic) {
            topics.insert(0, topic);
        }
        body.pop();
    }
    let topics = match Topics::try_from(topics) {
        Ok(topics) => topics,
        Err(_) => return refuse(Refusal::TooManyTopics),
    };
    let (Some(first), Some(last)) = (body.first().copied(), body.last().copied()) else {
        return refuse(Refusal::Unparsed);
    };
    let text = &said[first.start..last.end];

    // Form 4: a settling verb and one id.
    if body.len() == 2 && is(first.text, &SETTLING) {
        return refuse(Refusal::Unsupported {
            form: "settling a proposal or an entry",
        });
    }
    if is(first.text, &["distill"]) {
        return refuse(Refusal::Unsupported {
            form: "distilling a discussion",
        });
    }

    let instruction = |tag: TagName, content: &str| {
        Parsed::Instruction(Instruction {
            line,
            said: said.to_owned(),
            tag,
            content: content.to_owned(),
            topics: topics.clone(),
        })
    };
    let unknown = |word: &str| Refusal::UnknownTag {
        word: word.to_owned(),
        tags: tags.tags().iter().map(|t| t.name.clone()).collect(),
    };

    // Form 1: one word, a colon, the content.
    if let Some(colon) = text.find(':') {
        let head = &text[..colon];
        if !head.is_empty() && !head.contains(char::is_whitespace) {
            let content = text[colon + 1..].trim();
            if content.is_empty() {
                return refuse(Refusal::Unparsed);
            }
            return match tags.resolve(head) {
                Some(def) => instruction(def.name.clone(), content),
                None => refuse(unknown(head)),
            };
        }
    }

    // Form 2: [verb] <content> (to|in|under) [my|the] <tag>.
    let tag_word = last.text.trim_end_matches(['.', '!', ',', ';']);
    let mut end = body.len().saturating_sub(1);
    if end >= 1 && is(body[end - 1].text, &DETERMINERS) {
        end -= 1;
    }
    if end < 1 || !is(body[end - 1].text, &PREPOSITIONS) {
        return refuse(Refusal::Unparsed);
    }
    let prep = end - 1;
    let begin = usize::from(is(first.text, &VERBS));
    if begin >= prep {
        return refuse(Refusal::Unparsed);
    }
    let content = &said[body[begin].start..body[prep - 1].end];
    let Some(def) = tags.resolve(tag_word) else {
        return refuse(unknown(tag_word));
    };
    if prep - begin == 1 && is(content, &REFERENTS) {
        return refuse(Refusal::Unsupported {
            form: "pointing at an earlier message (\"this\", \"it\")",
        });
    }
    instruction(def.name.clone(), content)
}

/// SHA-256 of a prompt's text, lowercase hex: the cite's `prompt_sha256`,
/// which the `Stop` hook matches against the transcript.
#[must_use]
pub fn prompt_sha256(prompt: &str) -> String {
    let digest = Sha256::digest(prompt.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Where and when a prompt was typed: everything an entry records about its
/// turn besides the instruction itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    /// The Claude Code session.
    pub session: SessionId,
    /// When the hook received the prompt.
    pub at: OffsetDateTime,
    /// [`prompt_sha256`] of the whole prompt.
    pub prompt_sha256: String,
    /// The session's working directory.
    pub cwd: String,
    /// The repository the cwd belongs to.
    pub project: Project,
    /// The tool it was typed in, such as `claude-code`.
    pub provider: Label,
    /// The host it was typed on.
    pub host: Label,
}

/// The entry an instruction records, before the store gives it an id. Its
/// `review_after` comes from the tag's `stale_after_days`.
#[must_use]
pub fn draft(instruction: &Instruction, turn: &Turn, tags: &TagTable) -> NewEntry {
    NewEntry {
        tag: instruction.tag.clone(),
        project: turn.project.clone(),
        topics: instruction.topics.clone(),
        related: Vec::new(),
        provider: turn.provider.clone(),
        host: turn.host.clone(),
        created: turn.at,
        said: instruction.said.clone(),
        content: instruction.content.clone(),
        content_from: ContentFrom::Instruction,
        cite: Cite {
            session: turn.session.clone(),
            message: None,
            at: turn.at,
            prompt_sha256: turn.prompt_sha256.clone(),
            line: instruction.line,
            cwd: turn.cwd.clone(),
        },
        review_after: tags
            .get(&instruction.tag)
            .and_then(|def| def.review_after(turn.at.date())),
    }
}
