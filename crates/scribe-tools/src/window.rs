//! A session pass's window: the transcript's messages as a pass reads them
//! ([`conversation`]) and the next consecutive messages after the record's
//! `scribed_through` within the configured limits ([`window_with_limits`]).
//! They take messages and text and decide nothing about a record
//! (docs/design/scribe/specs/architecture.md); the session app's pass builds
//! its request from them.

use scribe_domain::config::{WINDOW_MESSAGES, WindowLimits};
use scribe_domain::entry::Speaker;
use scribe_domain::models::{CUT_MARK, PassMessage};
use scribe_domain::transcript::{Message, user_turn};

/// The session's messages as a pass reads them, in transcript order.
/// Daniel's turns are [`user_turn`].
#[must_use]
pub fn conversation(messages: &[Message]) -> Vec<PassMessage> {
    messages
        .iter()
        .filter_map(|m| match m.role {
            Speaker::Assistant => {
                let text = m.text().trim().to_owned();
                (!text.is_empty()).then(|| PassMessage {
                    uuid: m.uuid.clone(),
                    role: Speaker::Assistant,
                    text,
                    pasted: Vec::new(),
                })
            }
            Speaker::User => {
                let turn = user_turn(m)?;
                Some(PassMessage {
                    uuid: m.uuid.clone(),
                    role: Speaker::User,
                    text: turn.own,
                    pasted: turn.pasted,
                })
            }
        })
        .collect()
}

/// How many of Daniel's turns the conversation holds after the message
/// `through` (all of them when `through` is `None` or not found).
#[must_use]
pub fn user_turns_after(conversation: &[PassMessage], through: Option<&str>) -> u32 {
    let new = &conversation[start_after(conversation, through)..];
    u32::try_from(new.iter().filter(|m| m.role == Speaker::User).count()).unwrap_or(u32::MAX)
}

/// Index after the persisted cursor, or the start when that cursor is absent.
#[must_use]
pub fn start_after(conversation: &[PassMessage], through: Option<&str>) -> usize {
    through
        .and_then(|uuid| conversation.iter().position(|m| m.uuid == uuid))
        .map_or(0, |i| i + 1)
}

/// The messages one pass sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// The messages after the last pass, oldest first.
    pub messages: Vec<PassMessage>,
    /// Kept for request compatibility; consecutive windows never drop messages.
    pub dropped: usize,
}

/// The messages after `through` (the record's `scribed_through`; all of
/// them when it is `None` or no longer in the transcript), oldest first,
/// with the default message count and at most `max_chars` characters.
#[must_use]
pub fn window(conversation: &[PassMessage], through: Option<&str>, max_chars: usize) -> Window {
    next_window(conversation, through, WINDOW_MESSAGES, max_chars)
}

/// The next oldest window. No message is skipped; an individually oversized
/// message is cut with a marker, including that marker in the character bound.
#[must_use]
pub fn window_with_limits(
    conversation: &[PassMessage],
    through: Option<&str>,
    limits: WindowLimits,
) -> Window {
    next_window(
        conversation,
        through,
        limits.messages.get(),
        limits.chars.get(),
    )
}

fn next_window(
    conversation: &[PassMessage],
    through: Option<&str>,
    max_messages: usize,
    max_chars: usize,
) -> Window {
    let mut messages = Vec::new();
    let mut total = 0usize;
    for message in conversation[start_after(conversation, through)..]
        .iter()
        .take(max_messages)
    {
        let chars = message.chars();
        if !messages.is_empty() && chars > max_chars.saturating_sub(total) {
            break;
        }
        let mut message = message.clone();
        if chars > max_chars {
            // Keep the speaker's own text and pasted blocks distinct, even at a cut.
            let marker: String = CUT_MARK.chars().take(max_chars).collect();
            let mut left = max_chars.saturating_sub(marker.chars().count());
            message.text = message.text.chars().take(left).collect();
            left = left.saturating_sub(message.text.chars().count());
            message.pasted = message
                .pasted
                .iter()
                .filter_map(|p| {
                    if left == 0 {
                        return None;
                    }
                    let text: String = p.chars().take(left).collect();
                    left = left.saturating_sub(text.chars().count());
                    Some(text)
                })
                .collect();
            match message.pasted.last_mut() {
                Some(pasted) => pasted.push_str(&marker),
                None => message.text.push_str(&marker),
            }
        }
        total = total.saturating_add(message.chars());
        messages.push(message);
    }
    Window {
        messages,
        dropped: 0,
    }
}
