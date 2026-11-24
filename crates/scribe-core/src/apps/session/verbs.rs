//! Session verbs: links between sessions, the activity switch, a
//! continuation's handoff, and a session's forget (2_design.md §6, §12.4).

use crate::activity::{ActiveOutcome, SetActive};
use crate::checks::{check_active_author, check_session_field, forgotten_refusal, refusal};
use crate::{
    Entry, EntryStatus, Error, LinkType, Provider, Redactor, Result, SessionId, SessionLink,
    SessionRecord, Store, Tx, WriteRefusal, activity, control, handoff,
};
use std::collections::BTreeMap;
use time::OffsetDateTime;

/// How a continuation identifies its handoff.
#[derive(Clone, Copy)]
pub enum HandoffSelection<'a> {
    /// The newest active handoff for the session the transcript names.
    Predecessor(&'a SessionId),
    /// The handoff id or unique prefix the user typed.
    Named(&'a str),
}

/// The session verbs, implemented for [`Store`].
pub trait SessionVerbs {
    #[cfg(any(test, feature = "test-support"))]
    /// Link `session` to another session, under the store lock
    /// ([`control::link`]): when `on`, switch it on too. A new link is
    /// added to the session's record when it has one (its other fields,
    /// `updated` included, unchanged), else kept on its switch until the
    /// first pass writes the record. Refused links write nothing.
    ///
    /// # Errors
    ///
    /// [`Error::Link`] for a refused link; a store error otherwise. The
    /// session's own record failing to parse is [`Error::Corrupt`].
    fn link_session(
        &self,
        session: &SessionId,
        link: &SessionLink,
        on: bool,
        now: time::OffsetDateTime,
    ) -> Result<control::Linked>;

    /// Link and stamp the origin provider under the same store lock.
    fn link_session_for_provider(
        &self,
        session: &SessionId,
        link: &SessionLink,
        on: bool,
        now: time::OffsetDateTime,
        provider: crate::Provider,
    ) -> Result<control::Linked>;

    /// Change one session's `active` or `keep_active` under the store lock
    /// ([`activity::set_active`]), its reason masked by `redactor`. Only
    /// `active`, `keep_active` and `active_change` change (`updated`
    /// included stays), and the record is written only when something
    /// changed. Returns the record as it now stands and what changed.
    ///
    /// # Errors
    ///
    /// [`Error::SessionNotFound`] when no record has the id; a store error
    /// otherwise.
    fn set_active(
        &self,
        id: &SessionId,
        request: &SetActive,
        now: time::OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(SessionRecord, ActiveOutcome)>;

    /// Apply the routine's deactivations under one store lock, each
    /// checked again against the record on disk (a session pinned or
    /// already deactivated since the scan read it is left alone). A
    /// missing or unreadable record fails only its own request; the other
    /// requests continue. Returns each outcome in request order.
    fn set_active_many(
        &self,
        requests: &[(SessionId, SetActive)],
        now: time::OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Vec<Result<ActiveOutcome>>>;

    /// Return one checked handoff body and whether the child's link was written.
    /// An ordinary start takes no write lock. Recheck selection inside the write
    /// so retirement, a newer handoff, or a switch change cannot race injection.
    fn continue_handoff(
        &self,
        child: &SessionId,
        selection: HandoffSelection<'_>,
        now: OffsetDateTime,
    ) -> Result<Option<(String, bool)>>;

    /// Set one user-specified session record field, `title` or `project`,
    /// in a store write, its value masked by `redactor`.
    ///
    /// # Errors
    ///
    /// `invalid_text` for a field or value refused, `session_not_found`
    /// for a session with no record, and the store's errors.
    fn update_session_metadata(
        &self,
        id: &SessionId,
        field: &str,
        value: &str,
        redactor: &Redactor,
    ) -> Result<()>;
}

impl SessionVerbs for Store {
    #[cfg(any(test, feature = "test-support"))]
    fn link_session(
        &self,
        session: &SessionId,
        link: &SessionLink,
        on: bool,
        now: time::OffsetDateTime,
    ) -> Result<control::Linked> {
        self.link_session_for_provider(session, link, on, now, crate::Provider::ClaudeCode)
    }

    fn link_session_for_provider(
        &self,
        session: &SessionId,
        link: &SessionLink,
        on: bool,
        now: time::OffsetDateTime,
        provider: crate::Provider,
    ) -> Result<control::Linked> {
        self.write(|tx| {
            apply_session_link(
                tx,
                session,
                link,
                control::LinkSwitch::Manual { on },
                now,
                provider,
            )
        })
    }

    fn set_active(
        &self,
        id: &SessionId,
        request: &SetActive,
        now: time::OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(SessionRecord, ActiveOutcome)> {
        check_active_author(request)?;
        self.write(|tx| {
            let mut loaded = tx.sessions().loaded()?;
            let mut record = match loaded.take(id) {
                Some(record) => record?,
                None => {
                    return Err(Error::SessionNotFound {
                        id: id.clone(),
                        path: tx.sessions().place()?,
                    });
                }
            };
            let outcome = apply_active(tx, &mut record, request, now, redactor)?;
            Ok((record, outcome))
        })
    }

    fn set_active_many(
        &self,
        requests: &[(SessionId, SetActive)],
        now: time::OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Vec<Result<ActiveOutcome>>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        self.write(|tx| {
            let loaded = tx.sessions().loaded()?;
            let mut out = Vec::with_capacity(requests.len());
            for (id, request) in requests {
                let mut record = match loaded.records.iter().find(|r| &r.id == id) {
                    Some(record) => record.clone(),
                    None => {
                        out.push(Err(
                            match loaded.skipped.iter().find(|(s, _)| s.as_ref() == Some(id)) {
                                Some((_, bad)) => {
                                    Error::corrupt(bad.path.clone(), bad.error.to_string())
                                }
                                None => Error::SessionNotFound {
                                    id: id.clone(),
                                    path: tx.sessions().place()?,
                                },
                            },
                        ));
                        continue;
                    }
                };
                out.push(apply_active(tx, &mut record, request, now, redactor));
            }
            Ok(out)
        })
    }

    fn continue_handoff(
        &self,
        child: &SessionId,
        selection: HandoffSelection<'_>,
        now: OffsetDateTime,
    ) -> Result<Option<(String, bool)>> {
        if continuation_entry(self, selection)?.is_none() {
            return Ok(None);
        }
        self.write(|tx| {
            let Some(entry) = continuation_entry(self, selection)? else {
                return Ok(None);
            };
            crate::checks::entry::check_handoff(&entry.tag, &entry.content)?;
            let parent = handoff::check_related(&entry.related)
                .map_err(|err| refusal(WriteRefusal::HandoffSession, err.to_string()))?;
            let link = SessionLink {
                kind: LinkType::Continues,
                session: parent,
            };
            let linked = apply_session_link(
                tx,
                child,
                &link,
                control::LinkSwitch::Inherit,
                now,
                Provider::ClaudeCode,
            )?;
            Ok(Some((entry.content, linked.added)))
        })
    }

    fn update_session_metadata(
        &self,
        id: &SessionId,
        field: &str,
        value: &str,
        redactor: &Redactor,
    ) -> Result<()> {
        let value = value.trim();
        let field = check_session_field(field, value)?;
        let value = redactor.mask(value);
        self.write(|tx| {
            if tx.sessions().put_field(id, field, &value)? == 0 {
                return Err(Error::SessionNotFound {
                    id: id.clone(),
                    path: self.root().to_path_buf(),
                });
            }
            Ok(())
        })
    }
}

fn apply_session_link(
    tx: &Tx<'_>,
    session: &SessionId,
    link: &SessionLink,
    mode: control::LinkSwitch,
    now: time::OffsetDateTime,
    provider: crate::Provider,
) -> Result<control::Linked> {
    let loaded = tx.sessions().loaded()?;
    for id in [session, &link.session] {
        if loaded.forgotten.contains_key(id) {
            return Err(forgotten_refusal(id, "it cannot be linked"));
        }
    }
    if matches!(mode, control::LinkSwitch::Inherit) {
        for id in [session, &link.session] {
            if loaded
                .control
                .get(id)
                .is_some_and(|c| c.provider != provider)
            {
                return Err(refusal(
                    crate::WriteRefusal::HandoffSession,
                    "session belongs to another transcript provider",
                ));
            }
        }
    }
    let mut own: Option<SessionRecord> = None;
    let mut graph: BTreeMap<SessionId, Vec<SessionLink>> = BTreeMap::new();
    for record in loaded.records {
        graph.insert(record.id.clone(), record.links.clone());
        if &record.id == session {
            own = Some(record);
        }
    }
    for (id, bad) in loaded.skipped {
        match id {
            Some(id) if &id == session => return Err(bad.error),
            // Known by id, its links unreadable.
            Some(id) => {
                graph.entry(id).or_default();
            }
            None => {}
        }
    }
    // A refused link changes nothing, so nothing is written.
    tx.sessions().change(|state| {
        let linked = control::apply_link(state, &graph, session, link, mode, now)?;
        if let Some(control) = state.sessions.get_mut(session) {
            control.provider = provider;
        }
        let record = own.filter(|_| linked.write_record).map(|mut record| {
            record.links.push(link.clone());
            record
        });
        Ok((linked, record))
    })
}

/// Apply `request` to `record` and write it back when it changed. The
/// caller holds the lock.
fn apply_active(
    tx: &Tx<'_>,
    record: &mut SessionRecord,
    request: &SetActive,
    now: time::OffsetDateTime,
    redactor: &Redactor,
) -> Result<ActiveOutcome> {
    check_active_author(request)?;
    let outcome = activity::set_active(record, request, now);
    if outcome.changed() {
        if let Some(change) = record.active_change.as_mut() {
            change.reason = redactor.mask(&change.reason);
            if let Some(cite) = change.cite.as_mut() {
                *cite = redactor.mask(cite);
            }
        }
        tx.sessions().put(record)?;
    }
    Ok(outcome)
}

fn continuation_entry(store: &Store, selection: HandoffSelection<'_>) -> Result<Option<Entry>> {
    match selection {
        HandoffSelection::Predecessor(parent) => Ok(store
            .read()
            .entries()
            .handoffs_for_session(parent)?
            .into_iter()
            .next()
            .filter(|e| e.handoff_mode == Some(handoff::Mode::Continue))),
        HandoffSelection::Named(id) => {
            let entry = store.read().entries().resolve(id)?;
            if entry.status != EntryStatus::Active {
                return Err(refusal(
                    WriteRefusal::EntryNotActive,
                    "the handoff is no longer active",
                ));
            }
            if entry.tag.as_str() != handoff::HANDOFF_TAG
                || entry.handoff_mode != Some(handoff::Mode::Continue)
            {
                return Err(refusal(
                    WriteRefusal::HandoffSession,
                    "choose an active handoff with mode continue",
                ));
            }
            Ok(Some(entry))
        }
    }
}
