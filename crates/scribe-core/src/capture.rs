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
//! 3. `#scribe [add|record] (this|that|it|these) (to|in|under) [my|the] <tag>`
//!
//! A line whose opener is [`GLOBAL_OPENER`] instead of `#scribe` is the same
//! form and records `project: global`. Anything else after `#scribe/` is
//! refused. Hashtags at the end of the line are the entry's topics
//! (`#scribe decisions: squash-merge agent PRs #merge-policy`). Form 3 parses
//! to a [`Referent`]: the content is an earlier message's passage, which a
//! model picks from the [`window`] before the prompt and [`resolve`] checks
//! verbatim, falling back to the previous assistant message when the model
//! fails (§4.3).
//!
//! 4. `#scribe approve|reject <p-id>` · `#scribe retire|forget <entry-id>`
//!
//! Form 4 parses to a [`Settle`]: settling a proposal or hiding or blanking
//! an entry, which [`crate::consistency::settle`] applies. `distill` is
//! recognised and refused as not supported by this build, never misread as
//! form 2.
//!
//! A line under an action tag also keeps an [`action_window`]: the turns
//! since the previous `#scribe` line of the session, capped by
//! [`WINDOW_TURNS`] and [`WINDOW_CHARS`] and cut from the oldest turn (§4.6).
//! It takes no model.
//!
//! Pure functions over text: no I/O (STD-02 R5).

use crate::consistency::Settle;
use crate::entry::{Cite, ContentFrom, EntryId, NewEntry, Speaker, TagName, Window, WindowTurn};
use crate::models::{Candidate, ModelError, REFERENT_BUDGET, ReferentAnswer, ReferentRequest};
use crate::proposal::ProposalId;
use crate::record::{Label, Project, Topic, Topics};
use crate::session::SessionId;
use crate::tag::TagTable;
use crate::transcript::Message;
use crate::verbatim;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fmt::Write as _;
use time::OffsetDateTime;

/// The marker that opens an instruction line.
pub const MARKER: &str = "#scribe";

/// The opener that records the entry with `project: global` instead of the
/// cwd's project. It replaces [`MARKER`] as the line's first word
/// (`#scribe/global decisions: …`). It is not a topic hashtag and not a tag.
pub const GLOBAL_OPENER: &str = "#scribe/global";

/// Words that point at an earlier message instead of giving the content
/// (form 3, resolved by a model).
const REFERENTS: [&str; 4] = ["this", "that", "it", "these"];
/// The optional verbs that open form 2.
const VERBS: [&str; 3] = ["add", "record", "note"];
/// The prepositions before the tag in form 2.
const PREPOSITIONS: [&str; 3] = ["to", "in", "under"];
/// The optional determiners before the tag in form 2.
const DETERMINERS: [&str; 2] = ["my", "the"];
/// Verbs that settle a proposal or hide an entry (form 4).
const SETTLING: [&str; 4] = ["approve", "reject", "retire", "forget"];

/// How many messages before the prompt a referent may point at (§4.3).
pub const WINDOW: usize = 12;
/// How much of the previous assistant message a guessed referent records,
/// in characters (§4.3).
pub const FALLBACK_CHARS: usize = 600;

/// The most turns an action entry's window keeps (§4.6).
pub const WINDOW_TURNS: usize = 40;
/// The most characters of turn text an action entry's window keeps, all
/// turns together (§4.6).
pub const WINDOW_CHARS: usize = 12_000;
/// The most characters one turn of a window keeps; a longer turn keeps its
/// first this many and is marked `cut`.
pub const WINDOW_TURN_CHARS: usize = 4_000;

/// The forms, as the refusal of an unparsed instruction lists them.
///
/// The global opener is part of this text so every refusal that prints it
/// teaches how to record an entry for every repository.
pub const FORMS: &str = "\"#scribe <tag>: <content>\" or \"#scribe add <content> to <tag>\"; \
\"#scribe/global\" in place of \"#scribe\" records the entry for every repository";

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
    /// The line opened with [`GLOBAL_OPENER`], so the entry's project is
    /// `global` rather than the cwd's.
    pub global: bool,
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
    /// A referent ("this", "it") with no earlier message in the session to
    /// point at.
    NoReferent,
    /// A settling verb whose id is not the kind it takes.
    BadId {
        /// The verb as typed, lowercased.
        verb: String,
        /// What it takes, for the reply.
        expected: &'static str,
    },
    /// The line opened with `#scribe/` but not [`GLOBAL_OPENER`].
    MalformedMarker,
    /// [`GLOBAL_OPENER`] on a line that does not record an entry.
    MarkerNotOnEntry,
}

impl Refusal {
    /// The stable code the audit log and the reply carry (STD-01 R19).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTag { .. } => "unknown_tag",
            Self::Unparsed
            | Self::TooManyTopics
            | Self::BadId { .. }
            | Self::MalformedMarker
            | Self::MarkerNotOnEntry => "unparsed_instruction",
            Self::Unsupported { .. } => "unsupported_instruction",
            Self::NoReferent => "no_referent",
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
            Self::NoReferent => write!(
                f,
                "there is no earlier message in this session to point at; write the content itself: {FORMS}"
            ),
            Self::BadId { verb, expected } => {
                write!(f, "#scribe {verb} takes {expected}")
            }
            Self::MalformedMarker => {
                write!(f, "\"#scribe/\" is not the global marker; use {FORMS}")
            }
            Self::MarkerNotOnEntry => write!(
                f,
                "the global marker records an entry, and this line does not; use {FORMS}"
            ),
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
    /// The line carried [`GLOBAL_OPENER`] or a malformed `#scribe/` opener,
    /// so the reply includes [`FORMS`] when the reason does not already.
    pub marked: bool,
}

impl Refused {
    /// The text the hook echoes. A marked line's reply includes [`FORMS`],
    /// which teaches the global opener.
    #[must_use]
    pub fn message(&self) -> String {
        let text = self.reason.to_string();
        if self.marked && !text.contains(GLOBAL_OPENER) {
            format!("{text}; use {FORMS}")
        } else {
            text
        }
    }
}

/// A form-3 line: its content is a passage of an earlier message, which
/// [`resolve`] picks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Referent {
    /// The line's number within the prompt, from 1.
    pub line: u32,
    /// The whole line exactly as typed.
    pub said: String,
    /// The tag it resolved to.
    pub tag: TagName,
    /// The hashtags at the end of the line.
    pub topics: Topics,
    /// The line opened with [`GLOBAL_OPENER`].
    pub global: bool,
}

/// A form-4 line: settle a proposal, or hide or blank an entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settling {
    /// The line's number within the prompt, from 1.
    pub line: u32,
    /// The whole line exactly as typed.
    pub said: String,
    /// What it does.
    pub action: Settle,
}

/// The result for one `#scribe` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    /// It parsed and can be recorded.
    Instruction(Instruction),
    /// It points at an earlier message (form 3).
    Referent(Referent),
    /// It settles a proposal or changes an entry's status (form 4).
    Settle(Settling),
    /// It was refused.
    Refused(Refused),
}

impl Parsed {
    /// The line's number within the prompt.
    #[must_use]
    pub fn line(&self) -> u32 {
        match self {
            Self::Instruction(i) => i.line,
            Self::Referent(r) => r.line,
            Self::Settle(s) => s.line,
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
    // A `#scribe/` line is an attempted global marker, including a
    // malformed one, so it is refused rather than ignored.
    if trimmed.starts_with("#scribe/") {
        return true;
    }
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
    let all = words(said);
    let opener = all.first().map(|word| word.text).unwrap_or("");
    let (global, malformed) = if opener == GLOBAL_OPENER {
        (true, false)
    } else if opener.starts_with("#scribe/") {
        (true, true)
    } else {
        (false, false)
    };
    let refuse = |reason| {
        Parsed::Refused(Refused {
            line,
            said: said.to_owned(),
            reason,
            marked: global || malformed,
        })
    };
    if malformed {
        return refuse(Refusal::MalformedMarker);
    }
    // all[0] is the opener (`#scribe` or `#scribe/global`).
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

    // Form 4: a settling verb and one id; anything else after the verb is
    // refused, never read as another form. The global opener records an
    // entry's project, so it does not belong on a line that settles one.
    if is(first.text, &SETTLING) {
        if global {
            return refuse(Refusal::MarkerNotOnEntry);
        }
        let verb = first.text.to_lowercase();
        let id = if body.len() == 2 {
            last.text.trim_end_matches(['.', '!', ',', ';'])
        } else {
            ""
        };
        let action = match verb.as_str() {
            "approve" | "reject" => id.parse::<ProposalId>().ok().map(|p| {
                if verb == "approve" {
                    Settle::Approve(p)
                } else {
                    Settle::Reject(p)
                }
            }),
            _ => id.parse::<EntryId>().ok().map(|e| {
                if verb == "retire" {
                    Settle::Retire(e)
                } else {
                    Settle::Forget(e)
                }
            }),
        };
        return match action {
            Some(action) => Parsed::Settle(Settling {
                line,
                said: said.to_owned(),
                action,
            }),
            None => {
                let expected = if matches!(verb.as_str(), "approve" | "reject") {
                    "a proposal id, such as p-4"
                } else {
                    "an entry id, such as decisions-12"
                };
                refuse(Refusal::BadId { verb, expected })
            }
        };
    }
    if is(first.text, &["distill"]) {
        return if global {
            refuse(Refusal::MarkerNotOnEntry)
        } else {
            refuse(Refusal::Unsupported {
                form: "distilling a discussion",
            })
        };
    }

    let instruction = |tag: TagName, content: &str| {
        Parsed::Instruction(Instruction {
            line,
            said: said.to_owned(),
            tag,
            content: content.to_owned(),
            topics: topics.clone(),
            global,
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
        return Parsed::Referent(Referent {
            line,
            said: said.to_owned(),
            tag: def.name.clone(),
            topics,
            global,
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
    /// When the hook received the prompt, or the transcript timestamp for
    /// a prompt recovered after its hook was missed.
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

impl Turn {
    /// The cite of `#scribe` line `line` of this turn; its message uuid is
    /// `None` until the next `Stop` hook finds it.
    #[must_use]
    pub fn cite(&self, line: u32) -> Cite {
        Cite {
            session: self.session.clone(),
            message: None,
            at: self.at,
            prompt_sha256: self.prompt_sha256.clone(),
            line,
            cwd: self.cwd.clone(),
        }
    }
}

/// The entry an instruction records, before the store gives it an id. Its
/// `review_after` comes from the tag's `stale_after_days`.
#[must_use]
pub fn draft(instruction: &Instruction, turn: &Turn, tags: &TagTable) -> NewEntry {
    new_entry(
        Line {
            line: instruction.line,
            said: &instruction.said,
            tag: &instruction.tag,
            topics: &instruction.topics,
            global: instruction.global,
        },
        instruction.content.clone(),
        ContentFrom::Instruction,
        turn,
        tags,
    )
}

/// The parts of a `#scribe` line every entry draft records.
struct Line<'a> {
    line: u32,
    said: &'a str,
    tag: &'a TagName,
    topics: &'a Topics,
    global: bool,
}

fn new_entry(
    line: Line<'_>,
    content: String,
    content_from: ContentFrom,
    turn: &Turn,
    tags: &TagTable,
) -> NewEntry {
    NewEntry {
        tag: line.tag.clone(),
        project: if line.global {
            Project::sanitized(Project::GLOBAL)
        } else {
            turn.project.clone()
        },
        topics: line.topics.clone(),
        related: Vec::new(),
        provider: turn.provider.clone(),
        host: turn.host.clone(),
        created: turn.at,
        said: line.said.to_owned(),
        content,
        content_from,
        cite: turn.cite(line.line),
        review_after: tags
            .get(line.tag)
            .and_then(|def| def.review_after(turn.at.date())),
        window: None,
    }
}

/// The window an action entry keeps (§4.6): the transcript's turns since
/// the last user message before the prompt that carried a `#scribe` line,
/// oldest first, then the prompt itself (`message: None`, at `at`). The
/// prompt and everything after it are left out of the transcript part when
/// a retried hook finds it there, as [`window`] does. Each turn is cut to
/// [`WINDOW_TURN_CHARS`]; then the oldest turns are dropped until at most
/// [`WINDOW_TURNS`] turns and [`WINDOW_CHARS`] characters remain, and the
/// window records that it was truncated and how many turns it dropped. The
/// prompt itself is always kept. Model-free.
#[must_use]
pub fn action_window(
    messages: &[Message],
    prompt_sha256: &str,
    prompt: &str,
    at: OffsetDateTime,
) -> Window {
    let end = prompt_index(messages, prompt_sha256).unwrap_or(messages.len());
    let before = &messages[..end];
    let start = before
        .iter()
        .rposition(|m| m.role == Speaker::User && !lines(&m.text()).is_empty())
        .map_or(0, |i| i + 1);
    let mut turns: Vec<WindowTurn> = before[start..]
        .iter()
        .filter(|m| !m.text().trim().is_empty())
        .map(|m| window_turn(m.role, Some(m.uuid.clone()), m.at, &m.text()))
        .collect();
    turns.push(window_turn(Speaker::User, None, at, prompt));
    let mut kept: Vec<WindowTurn> = Vec::new();
    let mut chars = 0usize;
    for turn in turns.iter().rev() {
        let len = turn.text.chars().count();
        if !kept.is_empty() && (kept.len() >= WINDOW_TURNS || chars + len > WINDOW_CHARS) {
            break;
        }
        chars += len;
        kept.push(turn.clone());
    }
    kept.reverse();
    let omitted = turns.len() - kept.len();
    Window {
        turns: kept,
        truncated: omitted > 0,
        omitted: u32::try_from(omitted).unwrap_or(u32::MAX),
    }
}

fn window_turn(
    role: Speaker,
    message: Option<String>,
    at: OffsetDateTime,
    text: &str,
) -> WindowTurn {
    let text = text.trim();
    let cut = text.chars().count() > WINDOW_TURN_CHARS;
    WindowTurn {
        role,
        message,
        at,
        text: if cut {
            text.chars().take(WINDOW_TURN_CHARS).collect()
        } else {
            text.to_owned()
        },
        cut,
    }
}

/// The index of the user message that carries the prompt hashed to
/// `prompt_sha256`, the last one when there are several.
fn prompt_index(messages: &[Message], prompt_sha256: &str) -> Option<usize> {
    messages.iter().rposition(|m| {
        m.role == Speaker::User
            && (self::prompt_sha256(&m.text()) == prompt_sha256
                || m.blocks
                    .iter()
                    .any(|b| self::prompt_sha256(b) == prompt_sha256))
    })
}

/// The messages a referent may point at: the last [`WINDOW`] messages with
/// text before the prompt, oldest first. The prompt is usually not in the
/// transcript yet when the hook runs; when it is (a retried hook), it and
/// everything after it are left out, found by `prompt_sha256` as the `Stop`
/// hook finds it.
#[must_use]
pub fn window<'a>(messages: &'a [Message], prompt_sha256: &str) -> Vec<&'a Message> {
    let end = prompt_index(messages, prompt_sha256).unwrap_or(messages.len());
    let with_text: Vec<&Message> = messages[..end]
        .iter()
        .filter(|m| !m.text().trim().is_empty())
        .collect();
    with_text[with_text.len().saturating_sub(WINDOW)..].to_vec()
}

/// What to ask the referent model for `referent`, given the [`window`].
#[must_use]
pub fn referent_request(referent: &Referent, window: &[&Message], model: &str) -> ReferentRequest {
    ReferentRequest {
        model: model.to_owned(),
        budget: REFERENT_BUDGET,
        instruction: referent.said.clone(),
        candidates: window
            .iter()
            .map(|m| Candidate {
                uuid: m.uuid.clone(),
                role: m.role,
                text: m.text(),
            })
            .collect(),
    }
}

/// Why a referent was guessed rather than taken from the model's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Guess {
    /// The model call failed or timed out.
    Model(ModelError),
    /// The model named a message that is not in the window.
    UnknownMessage,
    /// The model's passage is not a verbatim span of the message it named.
    NotVerbatim,
}

impl fmt::Display for Guess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(err) => err.fmt(f),
            Self::UnknownMessage => {
                f.write_str("the model named a message that is not one of the recent ones")
            }
            Self::NotVerbatim => {
                f.write_str("the model's passage is not word for word in the message it named")
            }
        }
    }
}

/// The entry a form-3 line records, and why it was guessed when it was.
///
/// The model's answer is accepted only when its message is in `window` and
/// its span passes [`verbatim::contains`] against that message; the content
/// is then the span and `content_from` cites the message. Otherwise the
/// entry records the first [`FALLBACK_CHARS`] characters of the previous
/// assistant message (the latest message when there is none), and the
/// [`Guess`] says why. Exactly one draft either way.
///
/// `window` must not be empty; a line with nothing to point at is refused
/// with [`Refusal::NoReferent`] before any model call.
#[must_use]
pub fn resolve(
    referent: &Referent,
    window: &[&Message],
    answer: Result<ReferentAnswer, ModelError>,
    turn: &Turn,
    tags: &TagTable,
) -> Option<(NewEntry, Option<Guess>)> {
    let line = || Line {
        line: referent.line,
        said: &referent.said,
        tag: &referent.tag,
        topics: &referent.topics,
        global: referent.global,
    };
    let cite = |m: &Message| ContentFrom::Message {
        role: m.role,
        message: m.uuid.clone(),
        at: m.at,
    };
    let guess = match answer {
        Err(err) => Guess::Model(err),
        Ok(answer) => match window.iter().find(|m| m.uuid == answer.message) {
            None => Guess::UnknownMessage,
            Some(message) if verbatim::contains(&message.text(), &answer.span) => {
                let content = answer.span.trim().to_owned();
                return Some((new_entry(line(), content, cite(message), turn, tags), None));
            }
            Some(_) => Guess::NotVerbatim,
        },
    };
    let fallback = window
        .iter()
        .rev()
        .find(|m| m.role == Speaker::Assistant)
        .or_else(|| window.last())?;
    let content: String = fallback
        .text()
        .trim()
        .chars()
        .take(FALLBACK_CHARS)
        .collect();
    let content = content.trim_end().to_owned();
    Some((
        new_entry(line(), content, cite(fallback), turn, tags),
        Some(guess),
    ))
}
