//! Id resolution for every command that takes an id
//! (docs/design/scribe/2_design.md §1.1): an exact id, or a unique prefix of
//! at least [`MIN_PREFIX`] characters. An exact id wins over a prefix; an
//! ambiguous prefix is refused `id_ambiguous` naming up to
//! [`MAX_CANDIDATES`] candidates; an input that is not an id and is shorter
//! than [`MIN_PREFIX`] is a usage error; no match is the noun's not-found,
//! which names the id and the place searched.
//! Resolution reads only: no lock, no write.

use super::Store;
use crate::checks::refusal;
use crate::{Entry, EntryId, Error, Proposal, ProposalId, Result, SessionId, WriteRefusal};

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
        self.resolve_session_with(input, false, &[])
    }

    /// [`Store::resolve_session`], also accepting sessions known only through
    /// an active `handoff` entry (a session that was never switched on and
    /// has no session record; 2_design.md §16). Reads only.
    pub fn resolve_session_or_handoff(&self, input: &SessionId) -> Result<SessionId> {
        self.resolve_session_with(input, true, &[])
    }

    /// Every session id the store knows and `resolve_session_or_handoff`
    /// accepts: records (readable or not), switches, Codex cursors and
    /// sessions named only by an active handoff, and group members. The
    /// universe display prefixes must be unique in. Reads only.
    pub fn session_ids(&self) -> Result<Vec<String>> {
        let loaded = self.load_sessions()?;
        let handoffs: Vec<SessionId> = self
            .load_entries()?
            .0
            .iter()
            .filter(|e| {
                e.status == crate::EntryStatus::Active
                    && e.tag.as_str() == crate::handoff::HANDOFF_TAG
            })
            .flat_map(crate::handoff::session_ids)
            .collect();
        let mut ids = Self::session_id_universe(&loaded, &handoffs);
        ids.extend(self.group_session_ids()?);
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// `related` with a `session:` target written as a unique id prefix
    /// replaced by the full id, so what is stored or compared is always the
    /// full id. A target that matches no known session stays as typed: the
    /// session may be one the store has never cataloged, and the command
    /// that reads the reference reports it. Any other kind is returned as
    /// given. Reads only.
    ///
    /// # Errors
    ///
    /// A usage error for a target shorter than four characters that is not a
    /// known id; `id_ambiguous` naming the candidates when several ids
    /// match.
    pub fn resolve_related(&self, related: &crate::Related) -> Result<crate::Related> {
        self.resolve_related_with(related, &[])
    }

    /// [`Store::resolve_related`], also counting `extra_ids` (such as
    /// uncataloged transcripts on disk) in the session id universe. Reads only.
    ///
    /// # Errors
    ///
    /// As [`Store::resolve_related`].
    pub fn resolve_related_with(
        &self,
        related: &crate::Related,
        extra_ids: &[String],
    ) -> Result<crate::Related> {
        if related.kind() != crate::RelatedKind::Session {
            return Ok(related.clone());
        }
        let Ok(input) = related.target().parse::<SessionId>() else {
            return Ok(related.clone());
        };
        match self.resolve_session_with(&input, true, extra_ids) {
            Ok(id) if id == input => Ok(related.clone()),
            Ok(id) => format!("session:{id}")
                .parse::<crate::Related>()
                .map_err(|e| refusal(WriteRefusal::InvalidId, e.to_string())),
            Err(Error::SessionNotFound { .. }) => Ok(related.clone()),
            Err(other) => Err(other),
        }
    }

    /// [`Store::resolve_session_or_handoff`], except that an id matching no
    /// known session is returned as typed: for a caller naming the session
    /// it runs in, which the store may never have cataloged.
    ///
    /// # Errors
    ///
    /// As [`Store::resolve_session_or_handoff`], except not-found.
    pub fn resolve_session_or_typed(&self, input: &SessionId) -> Result<SessionId> {
        self.resolve_session_or_typed_with(input, &[])
    }

    /// [`Store::resolve_session_or_typed`], also counting `extra_ids` (such as
    /// uncataloged transcripts on disk) in the session id universe. Reads only.
    ///
    /// # Errors
    ///
    /// As [`Store::resolve_session_or_typed`].
    pub fn resolve_session_or_typed_with(
        &self,
        input: &SessionId,
        extra_ids: &[String],
    ) -> Result<SessionId> {
        match self.resolve_session_with(input, true, extra_ids) {
            Err(Error::SessionNotFound { .. }) => Ok(input.clone()),
            other => other,
        }
    }

    /// [`Store::resolve_related`] over a list.
    ///
    /// # Errors
    ///
    /// The first error of [`Store::resolve_related`].
    pub fn resolve_relateds(&self, related: &[crate::Related]) -> Result<Vec<crate::Related>> {
        self.resolve_relateds_with(related, &[])
    }

    /// [`Store::resolve_related_with`] over a list.
    ///
    /// # Errors
    ///
    /// The first error of [`Store::resolve_related_with`].
    pub fn resolve_relateds_with(
        &self,
        related: &[crate::Related],
        extra_ids: &[String],
    ) -> Result<Vec<crate::Related>> {
        related
            .iter()
            .map(|r| self.resolve_related_with(r, extra_ids))
            .collect()
    }

    fn group_session_ids(&self) -> Result<Vec<String>> {
        Ok(self
            .groups()?
            .into_iter()
            .flat_map(|g| {
                let mut ids: Vec<String> =
                    g.members().into_iter().map(|id| id.to_string()).collect();
                ids.push(g.opener);
                ids
            })
            .collect())
    }

    fn session_id_universe(
        loaded: &super::sessions::Loaded,
        handoffs: &[SessionId],
    ) -> Vec<String> {
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
            .chain(
                loaded
                    .control
                    .codex_cursors
                    .keys()
                    .map(|id| id.as_str().to_owned()),
            )
            .chain(handoffs.iter().map(|id| id.as_str().to_owned()))
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    fn resolve_session_with(
        &self,
        input: &SessionId,
        include_handoffs: bool,
        extra_ids: &[String],
    ) -> Result<SessionId> {
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
        let mut ids = Self::session_id_universe(&loaded, &handoff_sessions);
        ids.extend(self.group_session_ids()?);
        ids.extend(extra_ids.iter().cloned());
        ids.sort();
        ids.dedup();
        // An exact provider id first; then `s-<n>`, the row id (2_design.md
        // §15.7), which a provider's UUID never looks like.
        if !ids.iter().any(|id| id == input.as_str())
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
        match resolve(input.as_str(), &ids)? {
            Some(i) => ids[i]
                .parse::<SessionId>()
                .map_err(|e| refusal(WriteRefusal::InvalidId, e.to_string())),
            None => not_found(),
        }
    }
}

/// The entry and proposal lookups behind [`super::repo::Entries::resolve`]
/// and [`super::repo::Proposals::resolve`].
impl Store {
    /// The index in `ids` of the proposal `input` names; a miss is
    /// `proposal_not_found`, naming the id and the file searched.
    pub(super) fn proposal_index(&self, input: &str, ids: &[String]) -> Result<usize> {
        // A well-formed id is an exact lookup, as for entries; only other
        // input is a prefix, which needs four characters.
        let found = if input.parse::<ProposalId>().is_ok() {
            ids.iter().position(|id| id == input)
        } else {
            resolve(input, ids)?
        };
        found.ok_or_else(|| {
            refusal(
                WriteRefusal::ProposalNotFound,
                format!(
                    "no proposal {input} in {}; run 'scribe proposal list' with the same --root to see existing ids",
                    self.root.join(super::PROPOSALS_FILE).display()
                ),
            )
        })
    }

    /// Resolve an exact proposal id or a unique prefix of at least four
    /// characters, as the write verbs do, without locking or writing.
    pub(super) fn find_proposal(&self, input: &str) -> Result<Proposal> {
        let proposals = self.proposals()?;
        let ids: Vec<String> = proposals.iter().map(|p| p.id.to_string()).collect();
        let i = self.proposal_index(input, &ids)?;
        Ok(proposals[i].clone())
    }

    /// Resolve an exact entry id or a unique prefix of at least four characters.
    pub(super) fn find_entry(&self, input: &str) -> Result<Entry> {
        let exact = input.parse::<EntryId>().ok();
        if let Some(id) = &exact {
            match self.entry(id) {
                Ok(entry) => return Ok(entry),
                Err(Error::EntryNotFound { .. }) => {}
                Err(e) => return Err(e),
            }
        } else if input.len() < 4
            || !input
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || input
                .rsplit_once('-')
                .is_some_and(|(_, n)| n.starts_with('0'))
        {
            return Err(refusal(
                WriteRefusal::InvalidId,
                "use an entry id or a lowercase unique prefix of at least four characters",
            ));
        }
        let (entries, skipped) = self.load_entries()?;
        let mut ids: Vec<String> = entries.iter().map(|e| e.id.to_string()).collect();
        ids.extend(skipped.iter().filter_map(|s| {
            s.path
                .file_stem()?
                .to_str()?
                .parse::<EntryId>()
                .ok()
                .map(|id| id.to_string())
        }));
        ids.sort();
        ids.dedup();
        let Some(i) = resolve(input, &ids)? else {
            return Err(match exact {
                Some(id) => Error::EntryNotFound {
                    path: self.entry_path(&id),
                    id,
                },
                None => refusal(
                    WriteRefusal::EntryNotFound,
                    format!(
                        "no entry matches '{input}' under {}; run 'scribe entry list' with the same --root to see existing ids",
                        self.root.join(super::ENTRIES_DIR).display()
                    ),
                ),
            });
        };
        let id = ids[i]
            .parse::<EntryId>()
            .map_err(|e| refusal(WriteRefusal::InvalidId, e.to_string()))?;
        self.entry(&id)
    }
}
