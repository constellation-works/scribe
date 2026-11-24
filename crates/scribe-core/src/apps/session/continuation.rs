//! A continuation's predecessor: the session a `/clear` or a `--resume`
//! continues, as `SessionStart`, the prompt hook and `group join` find it.

use crate::runtime::{Ctx, PortError};
use crate::{Provider, SessionId};
use std::path::Path;

/// What a Claude Code transcript's opening `derived-from` metadata says about
/// which session(s) a new transcript was cleared from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClearedPredecessor {
    /// Not Claude Code, path missing, file absent/unreadable, or no `via: clear` entry.
    None,
    /// One distinct `via: clear` predecessor.
    One(SessionId),
    /// Multiple distinct `via: clear` predecessors.
    Multiple(Vec<SessionId>),
}

/// Inspect `path` for opening `derived-from` (`via: clear`) ancestors of `session`.
/// Returns `None` when the transcript does not yet exist or cannot be read.
#[must_use]
pub fn try_cleared_predecessor<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    path: &Path,
    session: &SessionId,
) -> Option<ClearedPredecessor> {
    if provider != Provider::ClaudeCode {
        return Some(ClearedPredecessor::None);
    }
    let header = match ctx.transcripts.claude_header(provider, path, session) {
        Ok(Some(header)) => header,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(?err, "cannot read session ancestry");
            return None;
        }
    };
    let mut parents = crate::transcript::cleared_sessions(&header);
    Some(match parents.len() {
        0 => ClearedPredecessor::None,
        1 => ClearedPredecessor::One(parents.swap_remove(0)),
        _ => ClearedPredecessor::Multiple(parents),
    })
}

/// Inspect `path` for opening `derived-from` (`via: clear`) ancestors of `session`.
#[must_use]
pub fn cleared_predecessor<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    path: Option<&Path>,
    session: &SessionId,
) -> ClearedPredecessor {
    let Some(path) = path else {
        return ClearedPredecessor::None;
    };
    try_cleared_predecessor(ctx, provider, path, session).unwrap_or(ClearedPredecessor::None)
}
