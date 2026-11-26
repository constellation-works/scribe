//! Id resolution for every command that takes an id
//! (docs/design/scribe/2_design.md §1.1): an exact id, or a unique prefix of
//! at least [`crate::MIN_PREFIX`] characters. An exact id wins over a prefix;
//! an ambiguous prefix is refused `id_ambiguous` naming up to
//! [`crate::MAX_CANDIDATES`] candidates; an input that is not an id and is
//! shorter than [`crate::MIN_PREFIX`] is a usage error; no match is the noun's
//! not-found, which names the id and the place searched.
//! Resolution reads only: no lock, no write.

pub(crate) use crate::resolve_id_index as resolve;
