//! The messages a session pass keeps in `scribe.db`
//! (docs/design/scribe/2_design.md §15.7), and which of a window's turns
//! they are.
//!
//! - **What is kept.** The user's typed turns of the window, with §5.3's
//!   exclusions: synthetic and control turns are not turns at all
//!   ([`crate::transcript::user_turn`]), pasted blocks are left out, and so is
//!   fenced code. An assistant turn is kept only when a claim or collected
//!   entry of the window cites it.
//! - **Not kept: sensitive turns.** A turn holding one of the memory gate's
//!   finance words or sensitive-event words ([`crate::memory::finance_word`],
//!   [`crate::memory::sensitive_word`]) is not stored, as no memory item may
//!   hold one outside `now` and `milestones`. A credential is masked by the
//!   store's redactor instead.
//! - **Forget.** Forgetting an entry or a memory item deletes the messages it
//!   cites (the store does it).
//! - **Keyed** on (session, provider message id), so writing a window again
//!   rewrites the same rows.
//!
//! No I/O (STD-02 R5): the store redacts and writes what [`window`] selects.

use crate::claim::PassClaim;
use crate::entry::Speaker;
use crate::models::PassMessage;
use crate::session::SessionId;
use crate::transcript;
use time::OffsetDateTime;

/// One message to keep, before redaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMessage {
    /// The session it belongs to.
    pub session: SessionId,
    /// The provider's message id (the transcript line's `uuid`).
    pub message: String,
    /// Whose words they are.
    pub role: Speaker,
    /// When the provider wrote it.
    pub at: OffsetDateTime,
    /// Its text: a user turn's own typed words, or a cited assistant turn.
    pub text: String,
}

/// One message a full-text search matched, as the store holds it (redacted).
#[derive(Clone, Debug, PartialEq)]
pub struct MessageMatch {
    /// The session it belongs to.
    pub session: SessionId,
    /// The provider's message id.
    pub message: String,
    /// Whose words they are.
    pub role: Speaker,
    /// When the provider wrote it.
    pub at: OffsetDateTime,
    /// Its stored text.
    pub text: String,
    /// FTS5's `bm25()` score, negated so that higher is better.
    pub score: f64,
    /// Project of the group that authorized a turn without a session pass.
    pub group_project: Option<crate::Project>,
}

/// What a message search read: the matches, best first, and whether they
/// came from a read-only copy of the database opened `immutable=1`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageSearch {
    /// Every match, best first.
    pub matches: Vec<MessageMatch>,
    /// The database was opened as a read-only copy (§15.7): a read-only
    /// root without its `-wal` and `-shm` files.
    pub read_only_copy: bool,
}

/// The messages to keep from one pass window: each user turn of `window`
/// with its own typed words left after fenced code is removed, and each
/// assistant turn a claim in `claims` cites. `transcript` gives each
/// message's time and an assistant turn's whole text; a message it does not
/// hold is skipped.
#[must_use]
pub fn window(
    session: &SessionId,
    window: &[PassMessage],
    transcript: &[transcript::Message],
    claims: &[PassClaim],
) -> Vec<NewMessage> {
    let line = |uuid: &str| transcript.iter().find(|m| m.uuid == uuid);
    let mut kept: Vec<NewMessage> = window
        .iter()
        .filter(|m| m.role == Speaker::User)
        .filter_map(|m| typed_turn(session, line(&m.uuid)?, &m.text))
        .collect();
    for claim in claims {
        let candidate = &claim.candidate;
        if candidate.role != Speaker::Assistant {
            continue;
        }
        let Some(uuid) = candidate.cite.message.as_deref() else {
            continue;
        };
        if kept.iter().any(|k| k.message == uuid) {
            continue;
        }
        let Some(m) = line(uuid) else { continue };
        let text = m.text().trim().to_owned();
        if keeps(&text) {
            kept.push(NewMessage {
                session: session.clone(),
                message: uuid.to_owned(),
                role: Speaker::Assistant,
                at: m.at,
                text,
            });
        }
    }
    kept
}

/// Select a verified user's turn for storage, applying the same content
/// exclusions as a session pass. `own` has already had pasted blocks removed.
#[must_use]
pub fn typed_turn(
    session: &SessionId,
    message: &transcript::Message,
    own: &str,
) -> Option<NewMessage> {
    let text = typed_text(own);
    keeps(&text).then(|| NewMessage {
        session: session.clone(),
        message: message.uuid.clone(),
        role: Speaker::User,
        at: message.at,
        text,
    })
}

/// Whether a turn's text is stored: not empty and holding no finance or
/// sensitive-event word.
#[must_use]
pub fn keeps(text: &str) -> bool {
    !text.is_empty()
        && !crate::memory::finance_word(text)
        && crate::memory::sensitive_word(text).is_none()
}

/// `text` without its fenced code blocks (a line opening with three or more
/// backticks or tildes, through the line that closes it, or the end), each
/// run of blank lines left by a removed block as one, trimmed. An unclosed
/// fence drops the rest of the text.
#[must_use]
pub fn typed_text(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let opener = trimmed.chars().next().filter(|c| *c == '`' || *c == '~');
        let run = opener.map_or(0, |c| trimmed.chars().take_while(|x| *x == c).count());
        match fence {
            Some((c, n)) => {
                if opener == Some(c) && run >= n && trimmed.trim_start_matches(c).trim().is_empty()
                {
                    fence = None;
                }
            }
            None => match opener {
                Some(c) if run >= 3 => fence = Some((c, run)),
                Some(_) | None => {
                    if !(line.trim().is_empty() && out.last().is_some_and(|l| l.trim().is_empty()))
                    {
                        out.push(line);
                    }
                }
            },
        }
    }
    out.join("\n").trim().to_owned()
}
