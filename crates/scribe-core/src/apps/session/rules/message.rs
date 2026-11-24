//! The messages a session pass keeps from its window (2_design.md §15.7):
//! each typed user turn, and each assistant turn a claim cites. Which turns
//! are kept at all is the entry app's (`apps/entry/rules/message.rs`).

use crate::apps::entry::{keeps, typed_turn};
use crate::claim::PassClaim;
use crate::entry::Speaker;
use crate::models::PassMessage;
use crate::session::SessionId;
use crate::store::NewMessage;
use crate::transcript;

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
