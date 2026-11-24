//! Id resolution for every command that takes an id
//! (docs/design/scribe/2_design.md §1.1): an exact id, or a unique prefix of
//! at least [`MIN_PREFIX`] characters. An exact id wins over a prefix; an
//! ambiguous prefix is refused `id_ambiguous` naming up to
//! [`MAX_CANDIDATES`] candidates; an input that is not an id and is shorter
//! than [`MIN_PREFIX`] is a usage error; no match is the noun's not-found,
//! which names the id and the place searched.
//! Resolution reads only: no lock, no write.

use crate::error::refusal;
use crate::{Error, Result, WriteRefusal};

/// The shortest prefix accepted, in characters.
pub(crate) const MIN_PREFIX: usize = crate::session::MIN_ID_PREFIX;

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
pub(crate) fn resolve(input: &str, ids: &[String]) -> Result<Option<usize>> {
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

/// The number of a group id `g-<n>`, when it is one exactly.
pub fn number(id: &str) -> Option<i64> {
    let n = id.strip_prefix("g-")?.parse::<i64>().ok()?;
    (n > 0 && format!("g-{n}") == id).then_some(n)
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
