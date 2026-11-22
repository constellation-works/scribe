//! Id resolution for every command that takes an id
//! (docs/design/scribe/2_design.md §1.1): an exact id, or a unique prefix of
//! at least [`MIN_PREFIX`] characters. An exact id wins over a prefix; an
//! ambiguous prefix is refused `id_ambiguous` naming up to
//! [`MAX_CANDIDATES`] candidates; an input that is not an id and is shorter
//! than [`MIN_PREFIX`] is a usage error; no match is the noun's not-found,
//! which names the id and the place searched.
//! Resolution reads only: no lock, no write.

use super::Store;
use super::agent::refusal;
use crate::{Error, Result, SessionId, WriteRefusal};

/// The shortest prefix accepted, in characters.
pub(crate) const MIN_PREFIX: usize = 4;
/// The most candidates an `id_ambiguous` refusal names.
pub(crate) const MAX_CANDIDATES: usize = 10;

/// The index in `ids` of `input`: the id equal to it, else the one id it is
/// a prefix of (at least [`MIN_PREFIX`] characters). `None` means nothing
/// matched; the caller builds its noun's not-found error, which names the id
/// and the place searched (STD-02 R26).
///
/// # Errors
///
/// `usage_error` when `input` is not an id and is shorter than
/// [`MIN_PREFIX`] characters; `id_ambiguous` listing the candidates when
/// several ids match.
pub(super) fn resolve(input: &str, ids: &[String]) -> Result<Option<usize>> {
    if let Some(i) = ids.iter().position(|id| id == input) {
        return Ok(Some(i));
    }
    if input.chars().count() < MIN_PREFIX {
        return Err(refusal(
            WriteRefusal::InvalidId,
            format!(
                "'{input}' is not an id and too short to be a prefix; give an id or a unique prefix of at least four characters"
            ),
        ));
    }
    let matches: Vec<usize> = ids
        .iter()
        .enumerate()
        .filter(|(_, id)| id.starts_with(input))
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [i] => Ok(Some(*i)),
        [] => Ok(None),
        _ => Err(ambiguous(input, matches.iter().map(|i| ids[*i].as_str()))),
    }
}

/// The id in `ids` that `input` names: exact, else a unique prefix of at
/// least [`MIN_PREFIX`] characters. `None` when none does, so the caller can
/// report its own noun's not-found with the place it searched.
///
/// # Errors
///
/// `usage_error` for an input shorter than [`MIN_PREFIX`] that is not an id;
/// `id_ambiguous` when several ids match.
pub fn resolve_prefix(input: &str, ids: &[String]) -> Result<Option<String>> {
    Ok(resolve(input, ids)?.map(|i| ids[i].clone()))
}

fn ambiguous<'a>(input: &str, candidates: impl Iterator<Item = &'a str>) -> Error {
    let mut candidates: Vec<&str> = candidates.collect();
    candidates.sort_unstable();
    let more = candidates.len().saturating_sub(MAX_CANDIDATES);
    candidates.truncate(MAX_CANDIDATES);
    let mut message = format!(
        "id prefix '{input}' is ambiguous; candidates: {}",
        candidates.join(", ")
    );
    if more > 0 {
        message.push_str(&format!(" and {more} more"));
    }
    message.push_str("; give more characters of the id");
    refusal(WriteRefusal::IdAmbiguous, message)
}

impl Store {
    /// The session `input` names: `s-<n>`, a session with that exact
    /// provider id (a record or a switch), else the one session whose id
    /// starts with it (at least four characters). Reads only.
    ///
    /// # Errors
    ///
    /// [`Error::SessionNotFound`] when nothing matches; a usage error for a
    /// short input; an `id_ambiguous`
    /// [`Error::AgentWrite`] naming the candidates when several do; a store
    /// error otherwise.
    pub fn resolve_session(&self, input: &SessionId) -> Result<SessionId> {
        self.resolve_session_with(input, false)
    }

    /// [`Store::resolve_session`], also accepting sessions known only through
    /// an active `handoff` entry (a session that was never switched on and
    /// has no session record; 2_design.md §16). Reads only.
    pub fn resolve_session_or_handoff(&self, input: &SessionId) -> Result<SessionId> {
        self.resolve_session_with(input, true)
    }

    fn resolve_session_with(&self, input: &SessionId, include_handoffs: bool) -> Result<SessionId> {
        let loaded = self.load_sessions()?;
        let handoff_sessions: Vec<SessionId> = if include_handoffs {
            self.load_entries()?
                .0
                .iter()
                .filter(|e| {
                    e.status == crate::EntryStatus::Active
                        && e.tag.as_str() == crate::handoff::HANDOFF_TAG
                })
                .flat_map(crate::handoff::session_ids)
                .collect()
        } else {
            Vec::new()
        };
        let not_found = || -> Result<SessionId> {
            Err(Error::SessionNotFound {
                id: input.clone(),
                path: self.root.join(super::SESSIONS_DIR),
            })
        };
        let exact = |id: &SessionId| id == input;
        // An exact provider id first; then `s-<n>`, the row id (2_design.md
        // §15.7), which a provider's UUID never looks like.
        if !loaded.records.iter().any(|r| exact(&r.id))
            && !loaded.numbers.keys().any(exact)
            && !loaded.control.sessions.keys().any(exact)
            && !handoff_sessions.iter().any(exact)
            && let Some(n) = input
                .as_str()
                .strip_prefix("s-")
                .and_then(|n| n.parse::<u64>().ok())
        {
            return match loaded.numbers.iter().find(|(_, v)| v.get() == n) {
                Some((id, _)) => Ok(id.clone()),
                None => not_found(),
            };
        }
        let mut ids: Vec<String> = loaded
            .records
            .iter()
            .map(|r| r.id.as_str().to_owned())
            .chain(
                loaded
                    .skipped
                    .iter()
                    .filter_map(|(id, _)| id.as_ref().map(|id| id.as_str().to_owned())),
            )
            .chain(loaded.numbers.keys().map(|id| id.as_str().to_owned()))
            .chain(
                loaded
                    .control
                    .sessions
                    .keys()
                    .map(|id| id.as_str().to_owned()),
            )
            .chain(handoff_sessions.iter().map(|id| id.as_str().to_owned()))
            .collect();
        ids.sort();
        ids.dedup();
        match resolve(input.as_str(), &ids)? {
            Some(i) => ids[i]
                .parse::<SessionId>()
                .map_err(|e| refusal(WriteRefusal::InvalidId, e.to_string())),
            None => not_found(),
        }
    }
}
