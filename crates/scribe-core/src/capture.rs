//! `#scribe` lines: finding them in a typed turn, and the model-free
//! settling commands (docs/design/scribe/2_design.md §4.1, §4.2, §14).
//!
//! [`lines`] reads a prompt line by line. A line is a `#scribe` line when it
//! starts with `#scribe` (after optional leading whitespace) and is not inside
//! a code fence, a blockquote or a `<pasted_content>` block, so a pasted log
//! or a quoted message that happens to contain `#scribe` never acts. A bare
//! `#decision` or any other hashtag is not one.
//!
//! Since §14 step 2 a `#scribe` line records nothing here: like a marker
//! inside a sentence it marks the turn, and the same turn's `Stop` catalogs
//! it with a statement and his quoted words (`crate::annotation`). The
//! grammar of forms 1 to 3 and the referent call are gone. Only form 4 is
//! parsed here, a whole line, model-free:
//!
//! 4. `#scribe approve|reject <p-id>` · `#scribe retire|forget <entry-id>` ·
//!    `#scribe forget <session-id>` (`s-<n>`, or the provider's UUID; §15.7)
//!
//! It parses to a [`Settling`] ([`crate::consistency::settle`] applies it).
//! A settling verb with the wrong kind of id is refused,
//! never read as a marker, and so is [`GLOBAL_OPENER`] on a settling line.
//!
//! An entry under an action tag also keeps an [`action_window`]: the turns
//! since the previous `#scribe` line of the session, capped by
//! [`WINDOW_TURNS`] and [`WINDOW_CHARS`] and cut from the oldest turn (§4.6).
//! It takes no model.
//!
//! Pure functions over text: no I/O (STD-02 R5).

use crate::entry::{Cite, EntryId, Speaker, TagName, Window, WindowTurn};
use crate::proposal::ProposalId;
use crate::record::{Label, Project, Topic, Topics};
use crate::session::SessionId;
use crate::tag::TagTable;
use crate::transcript::Message;
use std::fmt;
use time::OffsetDateTime;

/// The marker that opens a `#scribe` line, or marks a turn anywhere in a
/// sentence (§14.1).
pub const MARKER: &str = "#scribe";

/// The opener that records the entry with `project: global` instead of the
/// cwd's project, in place of [`MARKER`]. It is not a topic hashtag and not
/// a tag.
pub const GLOBAL_OPENER: &str = "#scribe/global";

/// Verbs that settle a proposal or hide an entry (form 4).
pub(crate) const SETTLING: [&str; 4] = ["approve", "reject", "retire", "forget"];

/// The most turns an action entry's window keeps (§4.6).
pub const WINDOW_TURNS: usize = 40;
/// The most characters of turn text an action entry's window keeps, all
/// turns together (§4.6).
pub const WINDOW_CHARS: usize = 12_000;
/// The most characters one turn of a window keeps; a longer turn keeps its
/// first this many and is marked `cut`.
pub const WINDOW_TURN_CHARS: usize = 4_000;

/// Why a settling line was not applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A settling verb whose id is not the kind it takes.
    BadId {
        /// The verb as typed, lowercased.
        verb: String,
        /// What it takes, for the reply.
        expected: &'static str,
    },
    /// [`GLOBAL_OPENER`] on a settling line, which records no entry.
    MarkerNotOnEntry,
}

impl Refusal {
    /// The stable code the audit log and the reply carry (STD-01 R19).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadId { .. } | Self::MarkerNotOnEntry => "unparsed_instruction",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadId { verb, expected } => write!(f, "#scribe {verb} takes {expected}"),
            Self::MarkerNotOnEntry => write!(
                f,
                "{GLOBAL_OPENER} records an entry for every repository; a settling line takes #scribe"
            ),
        }
    }
}

/// A settling line that was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// The line's number within the prompt, from 1.
    pub line: u32,
    /// The whole line exactly as typed.
    pub said: String,
    /// Why.
    pub reason: Refusal,
}

impl Refused {
    /// The text the hook echoes.
    #[must_use]
    pub fn message(&self) -> String {
        self.reason.to_string()
    }
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

/// The result for one settling line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
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
            Self::Settle(s) => s.line,
            Self::Refused(r) => r.line,
        }
    }
}

/// Every settling line of `prompt` (form 4), in order, parsed. A `#scribe`
/// line that does not open with a settling verb is a marker, not a command,
/// and is not here (`crate::annotation::marks` has it).
#[must_use]
pub fn scan(prompt: &str) -> Vec<Parsed> {
    lines(prompt)
        .into_iter()
        .filter_map(|(line, said)| parse(line, said))
        .collect()
}

/// Whether `said`, a `#scribe` line, is a settling command (form 4) rather
/// than a marker.
#[must_use]
pub fn settles(said: &str) -> bool {
    parse(0, said).is_some()
}

/// The instruction lines of `prompt`, unparsed: `(number from 1, the line
/// as typed)`. Lines in code fences, blockquotes and `<pasted_content>`
/// blocks are skipped. Empty for a prompt with no `#scribe` line, which
/// is most of them.
#[must_use]
pub fn lines(prompt: &str) -> Vec<(u32, &str)> {
    plain_lines(prompt)
        .into_iter()
        .filter(|(_, line)| is_instruction(line.trim_start()))
        .collect()
}

/// The lines of `prompt` that are typed text: `(number from 1, the line
/// without its `\r`)`, every line inside a code fence (and the fence lines
/// themselves), a blockquote line and a `<pasted_content>` block left out.
#[must_use]
pub fn plain_lines(prompt: &str) -> Vec<(u32, &str)> {
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
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        found.push((number, line));
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
    // A `#scribe/` line is an attempted global opener; a malformed one is
    // refused as a marker (`crate::annotation::marks`), never ignored.
    if trimmed.starts_with("#scribe/") {
        return true;
    }
    trimmed
        .strip_prefix(MARKER)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

fn is(word: &str, set: &[&str]) -> bool {
    let lower = word.to_lowercase();
    set.contains(&lower.as_str())
}

/// Parse one `#scribe` line as a settling command; `None` when its first
/// word after the opener is not a settling verb (it is a marker).
fn parse(line: u32, said: &str) -> Option<Parsed> {
    let all = words(said.trim_start());
    let opener = *all.first()?;
    let global = opener == GLOBAL_OPENER;
    if opener != MARKER && !global {
        return None;
    }
    let mut body: Vec<&str> = all.iter().skip(1).copied().collect();
    let first = *body.first()?;
    if !is(first, &SETTLING) {
        return None;
    }
    // Trailing hashtags are not part of the command.
    while body.len() > 1 && body.last().is_some_and(|w| w.starts_with('#')) {
        body.pop();
    }
    let refuse = |reason| {
        Some(Parsed::Refused(Refused {
            line,
            said: said.to_owned(),
            reason,
        }))
    };
    if global {
        return refuse(Refusal::MarkerNotOnEntry);
    }
    let verb = first.to_lowercase();
    let id = match body.as_slice() {
        [_, id] => id.trim_end_matches(['.', '!', ',', ';']),
        _ => "",
    };
    let action = match verb.as_str() {
        "approve" | "reject" => id.parse::<ProposalId>().ok().map(|p| {
            if verb == "approve" {
                Settle::Approve(p)
            } else {
                Settle::Reject(p)
            }
        }),
        "forget" if session_id(id).is_some() => session_id(id).map(Settle::ForgetSession),
        _ => id.parse::<EntryId>().ok().map(|e| {
            if verb == "retire" {
                Settle::Retire(e)
            } else {
                Settle::Forget(e)
            }
        }),
    };
    match action {
        Some(action) => Some(Parsed::Settle(Settling {
            line,
            said: said.to_owned(),
            action,
        })),
        None => {
            let expected = match verb.as_str() {
                "approve" | "reject" => "a proposal id, such as p-4",
                "forget" => {
                    "an entry id, such as preferences-12, or a session id, such as s-4 or 7c2b9d14"
                }
                _ => "an entry id, such as preferences-12",
            };
            refuse(Refusal::BadId { verb, expected })
        }
    }
}

/// The group lengths of a UUID, which a Claude Code or Codex session id is.
const UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// The session a forget names: `s-<n>` (no leading zero), a provider
/// session id in UUID form, or a prefix of one (at least
/// [`crate::session::MIN_ID_PREFIX`] characters, as a table prints it). A UUID or
/// its prefix is read as a session before an entry id, which it could
/// otherwise swallow when its last group is all digits. The store resolves a
/// prefix, refusing an ambiguous one. Anything else is an entry id or
/// nothing.
fn session_id(id: &str) -> Option<SessionId> {
    let numbered = id.strip_prefix("s-").is_some_and(|n| {
        !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
    });
    (numbered || is_uuid_prefix(id))
        .then(|| id.parse().ok())
        .flatten()
}

/// Whether `id` is a UUID, whole or cut anywhere after its first
/// [`crate::session::MIN_ID_PREFIX`] characters: hex groups of 8-4-4-4-12, every
/// group but the last complete and the last neither empty nor too long.
fn is_uuid_prefix(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    let Some((last, whole)) = groups.split_last() else {
        return false;
    };
    let hex = |g: &str| g.bytes().all(|b| b.is_ascii_hexdigit());
    id.len() >= crate::session::MIN_ID_PREFIX
        && groups.len() <= UUID_GROUPS.len()
        && whole
            .iter()
            .zip(UUID_GROUPS)
            .all(|(g, len)| g.len() == len && hex(g))
        && !last.is_empty()
        && last.len() <= UUID_GROUPS[whole.len()]
        && hex(last)
}

/// The hashes a transcript turn's text can match a typed prompt by: of the
/// whole text and of each text block, as the `Stop` hook matches cites.
#[must_use]
pub fn prompt_hashes(message: &crate::transcript::Message) -> Vec<String> {
    let mut hashes = vec![prompt_sha256(&message.text())];
    for block in &message.blocks {
        let hash = prompt_sha256(block);
        if !hashes.contains(&hash) {
            hashes.push(hash);
        }
    }
    hashes
}

pub use crate::origin::{OriginClaims, agent_originated, present_value};
pub use crate::text::{bytes_sha256, prompt_sha256};

// --- markers (§14.1) ---------------------------------------------------------

/// A settling instruction (form 4): `#scribe approve|reject <p-id>` or
/// `#scribe retire|forget <entry-id>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settle {
    /// Apply a proposal: its older entry becomes `superseded` (or `retired`
    /// for a stale proposal).
    Approve(ProposalId),
    /// Close a proposal for good; its pair is never proposed again.
    Reject(ProposalId),
    /// Hide an entry from answers; it stays on disk.
    Retire(EntryId),
    /// Blank an entry's text for good; its id, cite and status stay.
    Forget(EntryId),
    /// Erase a session's stored record and sources (`s-<n>`, its provider
    /// id or a unique prefix of it, which the store resolves), leaving an id-only tombstone. Open claims sourced from it are
    /// withdrawn, citing entries are detached, and handoffs naming it are
    /// deleted (2_design.md §15.7). The store applies it
    /// ([`crate::ProposalVerbs::settle`]).
    ForgetSession(SessionId),
}

impl Settle {
    /// The id the instruction names, as typed.
    #[must_use]
    pub fn target(&self) -> String {
        match self {
            Self::Approve(id) | Self::Reject(id) => id.to_string(),
            Self::Retire(id) | Self::Forget(id) => id.to_string(),
            Self::ForgetSession(id) => id.to_string(),
        }
    }
}

/// The most markers one turn may hold; a fourth is refused
/// [`MarkProblem::TooMany`] before any model call (§14.5).
pub const MAX_MARKS: usize = 3;

/// Why a marker is refused before any model call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkProblem {
    /// A settling verb after a marker inside a sentence: settling needs
    /// its own line (§14.1).
    Settling,
    /// `#scribe/<word>` with a word other than `global`.
    BadOpener,
    /// A marker past [`MAX_MARKS`] in one turn.
    TooMany,
}

impl MarkProblem {
    /// The refusal code it is recorded and audited with.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Settling | Self::BadOpener => "unparsed_instruction",
            Self::TooMany => "too_many_marks",
        }
    }

    /// Why, for Daniel's echo.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Settling => String::from(
                "settling needs its own line: #scribe approve|reject <p-id> or #scribe retire|forget <entry-id>",
            ),
            Self::BadOpener => format!("only {MARKER} and {GLOBAL_OPENER} mark a message"),
            Self::TooMany => format!("at most {MAX_MARKS} #scribe markers in one message"),
        }
    }
}

/// One `#scribe` marker of a typed turn (§14.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Its number in the turn, from 1, in text order.
    pub number: u32,
    /// The line it is on, from 1, as `capture::lines` numbers lines.
    pub line: u32,
    /// That line as typed.
    pub said: String,
    /// It was `#scribe/global`: the entry's project is `global`.
    pub global: bool,
    /// The word right after it, lowercased and without a trailing `:`,
    /// when there is one; a hint only when it names a tag ([`Mark::hint`]).
    pub word: Option<String>,
    /// For a marker that opens its line, the last word after `to`, `in` or
    /// `under` (and an optional `my` or `the`) at the end of the line: the
    /// tag of a folded form 2 or 3 (`#scribe add this to preferences`).
    pub tail: Option<String>,
    /// Hashtags after it on its line: the entry's topics.
    pub topics: Topics,
    /// Why it is refused before any call; `None` for a marker the model
    /// reads.
    pub problem: Option<MarkProblem>,
}

impl Mark {
    /// The tag its next word names (a tag or an alias), else the tag its
    /// line ends with ([`Mark::tail`]), which the model may not change;
    /// `None` when neither names one.
    #[must_use]
    pub fn hint<'a>(&self, tags: &'a TagTable) -> Option<&'a TagName> {
        [self.word.as_deref(), self.tail.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(|word| tags.resolve(word))
            .map(|def| &def.name)
            .next()
    }
}

/// The tag word a folded form 2 or 3 line ends with: `… (to|in|under)
/// [my|the] <tag>`, before any trailing hashtags.
fn tail(rest: &str) -> Option<String> {
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    while words.last().is_some_and(|w| w.starts_with('#')) {
        words.pop();
    }
    let tag = words.pop()?.trim_end_matches(PUNCTUATION).to_lowercase();
    if words
        .last()
        .is_some_and(|w| DETERMINERS.contains(&w.to_lowercase().as_str()))
    {
        words.pop();
    }
    words
        .last()
        .is_some_and(|w| PREPOSITIONS.contains(&w.to_lowercase().as_str()))
        .then_some(tag)
        .filter(|tag| !tag.is_empty())
}

/// What a word after a marker sheds at its end.
const PUNCTUATION: [char; 9] = [':', ',', '.', ';', '!', '?', ')', ']', '}'];

/// The prepositions before the tag that ends a folded form 2 or 3 line.
const PREPOSITIONS: [&str; 3] = ["to", "in", "under"];
/// The optional determiners before that tag.
const DETERMINERS: [&str; 2] = ["my", "the"];

/// The markers of a typed turn, in text order, numbered from 1 (§14.1).
///
/// Lines inside code fences, blockquotes and `<pasted_content>` blocks hold
/// none (as `capture::plain_lines` leaves them out), nor does an indented
/// code line or a whole-line settling command (form 4). Within a line,
/// a token in an inline code span or a double-quoted span (`"…"`, `“…”`) is
/// a mention, not a marker; a backtick or quote with no partner on its line
/// (`a 12" pipe`) opens no span. A marker is [`MARKER`] or `#scribe/<word>`
/// that starts the line or follows whitespace or an opening bracket, and
/// is followed by the end of the line, whitespace, `:`, a closing bracket
/// or sentence punctuation.
#[must_use]
pub fn marks(text: &str) -> Vec<Mark> {
    let mut found = Vec::new();
    for (line, said) in plain_lines(text) {
        // A settling line is a command (form 4); an indented line is code.
        if settles(said) || said.starts_with("    ") || said.starts_with('\t') {
            continue;
        }
        for start in mark_starts(said) {
            let number = u32::try_from(found.len() + 1).unwrap_or(u32::MAX);
            let mut mark = parse_mark(number, line, said, start);
            if found.len() >= MAX_MARKS {
                mark.problem = Some(MarkProblem::TooMany);
            }
            found.push(mark);
        }
    }
    found
}

/// The markers of a typed turn as the transcript holds it (§14.1): [`marks`]
/// of its text, or, for a slash command Daniel typed, of the command's args,
/// which are his words (`transcript::slash_command_args`, §4.1). An args
/// marker keeps the line number of the turn's whole text, so its cite and the
/// lines an older store recorded verbatim agree with the prompt the line is
/// in.
#[must_use]
pub fn turn_marks(text: &str) -> Vec<Mark> {
    let Some(args) = crate::transcript::slash_command_args(text) else {
        return marks(text);
    };
    // `args` is a slice of `text`; its first line is the line it starts on.
    let start = (args.as_ptr() as usize).saturating_sub(text.as_ptr() as usize);
    let before = text
        .get(..start)
        .map_or(0, |head| head.matches('\n').count());
    let shift = u32::try_from(before).unwrap_or(u32::MAX);
    marks(args)
        .into_iter()
        .map(|mut mark| {
            mark.line = mark.line.saturating_add(shift);
            mark
        })
        .collect()
}

/// Byte offsets in `line` where a marker starts, outside code and quotes.
fn mark_starts(line: &str) -> Vec<usize> {
    let lone = unpaired(line);
    let mut out = Vec::new();
    let (mut code, mut quoted, mut curly) = (false, false, false);
    let mut prev: Option<char> = None;
    for (at, c) in line.char_indices() {
        match c {
            _ if lone.contains(&at) => {}
            '`' => code = !code,
            '"' if !code => quoted = !quoted,
            '“' if !code => curly = true,
            '”' if !code => curly = false,
            '#' if !code && !quoted && !curly => {
                let opens = prev.is_none_or(|p| p.is_whitespace() || matches!(p, '(' | '[' | '{'));
                let rest = &line[at..];
                let ends = rest.strip_prefix(MARKER).is_some_and(|after| {
                    after.is_empty()
                        || after.starts_with(char::is_whitespace)
                        || after.starts_with([':', '/', ')', ']', '}', ',', '.', ';', '!', '?'])
                });
                if opens && ends {
                    out.push(at);
                }
            }
            _ => {}
        }
        prev = Some(c);
    }
    out
}

/// Byte offsets of the span openers in `line` that no partner closes: the
/// last backtick of an odd count, the last straight quote of an odd count
/// outside code, and a `“` with no `”` after it. Such an opener is a stray
/// character (`a 12" pipe`), so it hides nothing that follows it.
fn unpaired(line: &str) -> Vec<usize> {
    let mut lone = Vec::new();
    let ticks: Vec<usize> = line.match_indices('`').map(|(at, _)| at).collect();
    if ticks.len() % 2 == 1 {
        lone.extend(ticks.last());
    }
    let mut code = false;
    let mut quotes = Vec::new();
    let mut open_curly = None;
    for (at, c) in line.char_indices() {
        match c {
            '`' if !lone.contains(&at) => code = !code,
            '"' if !code => quotes.push(at),
            '“' if !code && open_curly.is_none() => open_curly = Some(at),
            '”' if !code => open_curly = None,
            _ => {}
        }
    }
    if quotes.len() % 2 == 1 {
        lone.extend(quotes.last());
    }
    lone.extend(open_curly);
    lone
}

/// The marker starting at byte `start` of `said`.
fn parse_mark(number: u32, line: u32, said: &str, start: usize) -> Mark {
    let rest = &said[start + MARKER.len()..];
    let mut words = rest.split_whitespace();
    let opener_tail = if rest.starts_with('/') {
        // Sentence punctuation may follow the opener: `… #scribe/global.`
        words.next().map(|w| w.trim_end_matches(PUNCTUATION))
    } else {
        None
    };
    let (global, bad) = match opener_tail {
        None => (false, false),
        Some(tail) => (tail == "/global", tail != "/global"),
    };
    // `#scribe: idea` and `#scribe idea:` both read `idea` as the word.
    let mut next = words.next().map(|w| w.trim_start_matches(':'));
    if next.is_some_and(str::is_empty) {
        next = words.next();
    }
    let word = next
        .map(|w| w.trim_end_matches(PUNCTUATION).to_lowercase())
        .filter(|w| !w.is_empty() && !w.starts_with('#'));
    let mut topics: Vec<Topic> = Vec::new();
    for hashtag in rest.split_whitespace().filter_map(|w| w.strip_prefix('#')) {
        let tag = hashtag.trim_end_matches(PUNCTUATION).to_lowercase();
        if let Ok(topic) = Topic::try_from(tag)
            && !topics.contains(&topic)
            && topics.len() < Topics::MAX
            && topic.as_str() != "scribe"
        {
            topics.push(topic);
        }
    }
    let problem = if bad {
        Some(MarkProblem::BadOpener)
    } else if word.as_deref().is_some_and(|w| SETTLING.contains(&w)) {
        Some(MarkProblem::Settling)
    } else {
        None
    };
    let opens_line = said[..start].trim().is_empty();
    Mark {
        number,
        line,
        said: said.to_owned(),
        global,
        word,
        tail: if opens_line { tail(rest) } else { None },
        topics: Topics::try_from(topics).unwrap_or_default(),
        problem,
    }
}

/// Where and when a prompt was typed: everything an entry records about its
/// turn besides what it says.
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
        .rposition(|m| {
            m.role == Speaker::User
                && (!lines(&m.text()).is_empty() || !marks(&m.text()).is_empty())
        })
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
