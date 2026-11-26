//! The `hook` app's rules over a typed turn: where and when it was typed
//! ([`Turn`], an entry's cite) and the hashes the `Stop` hook matches a
//! transcript turn to the typed prompt by ([`prompt_hashes`]).

use crate::entry::Cite;
use crate::record::{Label, Project};
use crate::session::SessionId;
use crate::text::prompt_sha256;
use time::OffsetDateTime;

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
