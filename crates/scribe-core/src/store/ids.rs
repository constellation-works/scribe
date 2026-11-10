//! Id resolution for every command that takes an id
//! (docs/design/scribe/2_design.md §1.1): an exact id, or a unique prefix of
//! at least [`MIN_PREFIX`] characters. An exact id wins over a prefix; an
//! ambiguous prefix is refused `id_ambiguous` naming up to
//! [`MAX_CANDIDATES`] candidates; no match is the noun's not-found.
//! Resolution reads only: no lock, no write.

use super::Store;
use super::agent::refusal;
use crate::{Error, Result, SessionId, WriteRefusal};

/// The shortest prefix accepted, in characters.
pub(crate) const MIN_PREFIX: usize = 4;
/// The most candidates an `id_ambiguous` refusal names.
pub(crate) const MAX_CANDIDATES: usize = 10;

/// The index in `ids` of `input`: the id equal to it, else the one id it is
/// a prefix of (at least [`MIN_PREFIX`] characters).
///
/// # Errors
///
/// `missing` when nothing matches; `id_ambiguous` listing the candidates
/// when several do.
pub(super) fn resolve(input: &str, ids: &[String], missing: WriteRefusal) -> Result<usize> {
    if let Some(i) = ids.iter().position(|id| id == input) {
        return Ok(i);
    }
    let matches: Vec<usize> = ids
        .iter()
        .enumerate()
        .filter(|(_, id)| input.chars().count() >= MIN_PREFIX && id.starts_with(input))
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => Err(refusal(
            missing,
            "no matching id; list records with the same --root",
        )),
        _ => Err(ambiguous(input, matches.iter().map(|i| ids[*i].as_str()))),
    }
}

/// The id in `ids` that `input` names: exact, else a unique prefix of at
/// least [`MIN_PREFIX`] characters.
///
/// # Errors
///
/// `id_ambiguous` when several ids match; an entry-not-found refusal when
/// none do (the caller maps that to its own noun).
pub fn resolve_prefix(input: &str, ids: &[String]) -> Result<String> {
    let index = resolve(input, ids, WriteRefusal::EntryNotFound)?;
    Ok(ids[index].clone())
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
    /// The session `input` names: a session with that exact id (a record
    /// or a switch), else the one session whose id starts with it (at least
    /// four characters). Reads only.
    ///
    /// # Errors
    ///
    /// [`Error::SessionNotFound`] when nothing matches; an `id_ambiguous`
    /// [`Error::AgentWrite`] naming the candidates when several do; a store
    /// error otherwise.
    pub fn resolve_session(&self, input: &SessionId) -> Result<SessionId> {
        let mut ids: Vec<String> = self.session_files()?.into_iter().map(|f| f.stem).collect();
        ids.extend(
            self.control()?
                .sessions
                .keys()
                .map(|id| id.as_str().to_owned()),
        );
        ids.sort();
        ids.dedup();
        match resolve(input.as_str(), &ids, WriteRefusal::EntryNotFound) {
            Ok(i) => ids[i]
                .parse::<SessionId>()
                .map_err(|e| refusal(WriteRefusal::InvalidId, e.to_string())),
            Err(Error::AgentWrite {
                kind: WriteRefusal::EntryNotFound,
                ..
            }) => Err(Error::SessionNotFound {
                id: input.clone(),
                path: self.root.join(super::SESSIONS_DIR),
            }),
            Err(other) => Err(other),
        }
    }
}
