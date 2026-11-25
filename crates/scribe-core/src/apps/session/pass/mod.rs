//! Pass verbs: the session pass's and catalog's window writes, and the
//! candidates a window opens (2_design.md §11, §15).

mod run;

pub use run::{
    ActivityAnswer, ApplyInput, ApplyOutput, ApplyStatus, CATALOG_COMMAND, FailInput, PASS_COMMAND,
    PASS_START_BY_SECONDS, PassPayload, PassReport, PassRunError, PrepareInput, Prepared,
    SessionPassInput, attributed_project, short_reason,
};
pub(crate) use run::{
    CatalogJob, RunFailure, apply, distill, fail, prepare, run_catalog, settle_only,
    window_warnings,
};

use crate::apps::memory::check_sensitive_drafts;
use crate::apps::memory::{WindowMemory, plan_window};
use crate::apps::proposal::{
    checked_outcome, has_context_and_subject, open_under, pass_entry, written_from,
};
use crate::apps::tag::ConfigReads;
use crate::pass::{
    MAX_CANDIDATES, MAX_COLLECTED, MAX_OPEN_CANDIDATES, admission, candidate_says, claim_covers,
    covering_claim, entry_says, own_span,
};
use crate::{
    Author, EntryId, Error, MemoryWritten, Proposal, ProposalId, ProposalKind, ProposalState,
    Redactor, Result, SessionId, SessionLink, SessionRecord, Store, Tx, control,
};

/// What the candidate write path changed under the store lock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaimsWritten {
    /// Open candidates closed because their tag is retired, in id order.
    pub closed: Vec<Proposal>,
    /// Entries the pass wrote itself under `write` tags, in write order.
    pub written: Vec<EntryId>,
}

/// The pass verbs, implemented for [`Store`].
#[cfg(any(test, feature = "test-support"))]
pub trait PassVerbs {
    /// Finish a session pass under the store lock: write `record` (when the
    /// pass produced one) whole, its free text
    /// masked by `redactor`, then settle the session's switch with
    /// [`control::settle`]. Returns the record as written.
    ///
    /// `expected` is the `updated` revision the pass loaded (`None` when it
    /// loaded no record). Under the lock it is compared with the record on
    /// disk before anything is written; a mismatch refuses the write with
    /// [`Error::RecordChanged`], leaving the record on disk unchanged and
    /// the session due, so the next run starts from that record. This
    /// mirrors [`crate::RebuildVerbs::finish_rebuild`].
    fn settle_pass(
        &self,
        session: &SessionId,
        record: Option<SessionRecord>,
        ran_final: bool,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>>;

    #[cfg(any(test, feature = "test-support"))]
    /// Persist one intermediate window under the store lock, redacted and
    /// atomically replaced. Keep the pass due until its last window settles.
    /// `expected` is checked as in [`PassVerbs::settle_pass`].
    fn checkpoint_pass(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>>;

    #[cfg(any(test, feature = "test-support"))]
    /// The last window of a live `session catalog`: write the record and
    /// leave the switch on and due, clearing retry backoff, so the hooks
    /// carry on. `expected` is checked as in [`PassVerbs::settle_pass`].
    fn checkpoint_catalog(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>>;

    /// The last window of an ended `session catalog`, under one lock: write
    /// the record, settle the pass as final, and switch the session off so
    /// no hook passes it again. A crash cannot leave it on but not due.
    fn settle_catalog_final(
        &self,
        session: &SessionId,
        record: Option<SessionRecord>,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>>;

    /// Write a checked pass window and its candidates under the same lock.
    /// Candidates are persisted first, so an interruption retries safely.
    /// Then one `scribe.db` transaction commits the record (its cursor
    /// advanced), the window's `messages`, the memory rows and journal and
    /// the switch; the memory files follow the journal (2_design.md §15.6,
    /// §15.7). Retired-tag open candidates are closed before caps are
    /// counted, duplicates and each claim's trigger (a trigger of its tag
    /// fires in its turn, under the config read under the lock) are checked
    /// here.
    #[allow(clippy::too_many_arguments)]
    fn write_pass_window(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
        kind: PassWrite,
        candidates: &[crate::pass::PassClaim],
        memory: Option<&WindowMemory>,
        messages: &[crate::message::NewMessage],
        redactor: &Redactor,
    ) -> Result<PassWritten>;

    #[cfg(any(test, feature = "test-support"))]
    /// Open the window's candidates in the write under way, or in one of its
    /// own (`open_candidates`).
    ///
    /// # Errors
    ///
    /// The config's and the store's errors.
    fn open_pass_candidates(
        &self,
        record: &SessionRecord,
        drafts: &[crate::pass::PassClaim],
        redactor: &Redactor,
    ) -> Result<ClaimsWritten>;
}

#[cfg(any(test, feature = "test-support"))]
impl PassVerbs for Store {
    fn settle_pass(
        &self,
        session: &SessionId,
        record: Option<SessionRecord>,
        ran_final: bool,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        write_pass(
            self,
            session,
            record,
            PassWrite::Settle(ran_final),
            expected,
            redactor,
            None,
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    fn checkpoint_pass(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        let session = record.id.clone();
        write_pass(
            self,
            &session,
            Some(record),
            PassWrite::Checkpoint,
            expected,
            redactor,
            None,
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    fn checkpoint_catalog(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        let session = record.id.clone();
        write_pass(
            self,
            &session,
            Some(record),
            PassWrite::CatalogLive,
            expected,
            redactor,
            None,
        )
    }

    fn settle_catalog_final(
        &self,
        session: &SessionId,
        record: Option<SessionRecord>,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        write_pass(
            self,
            session,
            record,
            PassWrite::CatalogFinal,
            expected,
            redactor,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn write_pass_window(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
        kind: PassWrite,
        candidates: &[crate::pass::PassClaim],
        memory: Option<&WindowMemory>,
        messages: &[crate::message::NewMessage],
        redactor: &Redactor,
    ) -> Result<PassWritten> {
        write_window(
            self, record, expected, kind, candidates, memory, messages, redactor, None,
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    fn open_pass_candidates(
        &self,
        record: &SessionRecord,
        drafts: &[crate::pass::PassClaim],
        redactor: &Redactor,
    ) -> Result<ClaimsWritten> {
        self.write(|tx| open_candidates(tx, record, drafts, redactor))
    }
}

#[allow(clippy::too_many_arguments)]
fn write_window(
    store: &Store,
    record: SessionRecord,
    expected: Option<time::OffsetDateTime>,
    kind: PassWrite,
    candidates: &[crate::pass::PassClaim],
    memory: Option<&WindowMemory>,
    messages: &[crate::message::NewMessage],
    redactor: &Redactor,
    expected_switch: Option<&crate::SessionControl>,
) -> Result<PassWritten> {
    let session = record.id.clone();
    write_pass_candidates(
        store,
        &session,
        Some(record),
        kind,
        expected,
        PassParts {
            candidates,
            memory,
            messages,
            run_candidates: true,
            expected_switch,
        },
        redactor,
    )
}

/// The parts of a pass window besides its record.
struct PassParts<'a> {
    /// Claims to open.
    candidates: &'a [crate::pass::PassClaim],
    /// The window's memory, planned and committed with the record.
    memory: Option<&'a WindowMemory>,
    /// The window's messages, upserted with the record.
    messages: &'a [crate::message::NewMessage],
    /// Whether to open `candidates` (and close retired-tag ones).
    run_candidates: bool,
    expected_switch: Option<&'a crate::SessionControl>,
}

fn write_pass(
    store: &Store,
    session: &SessionId,
    record: Option<SessionRecord>,
    kind: PassWrite,
    expected: Option<time::OffsetDateTime>,
    redactor: &Redactor,
    expected_switch: Option<&crate::SessionControl>,
) -> Result<Option<SessionRecord>> {
    Ok(write_pass_candidates(
        store,
        session,
        record,
        kind,
        expected,
        PassParts {
            candidates: &[],
            memory: None,
            messages: &[],
            run_candidates: false,
            expected_switch,
        },
        redactor,
    )?
    .record)
}

#[allow(
    clippy::too_many_arguments,
    reason = "the pass window's parts at the use-case boundary"
)]
fn write_pass_candidates(
    store: &Store,
    session: &SessionId,
    record: Option<SessionRecord>,
    kind: PassWrite,
    expected: Option<time::OffsetDateTime>,
    parts: PassParts<'_>,
    redactor: &Redactor,
) -> Result<PassWritten> {
    store.write(|tx| {
        // Compare the revision the pass loaded with the stored one before
        // any write, so a rebuild that finished while the model ran is
        // never overwritten by this stale pass. A link keeps `updated`, so
        // a linked record still matches and its links are merged below.
        let mut loaded = tx.sessions().loaded()?;
        let current = loaded.take(session).transpose()?;
        if current.as_ref().map(|c| c.updated) != expected
            || parts
                .expected_switch
                .is_some_and(|switch| loaded.control.get(session) != Some(switch))
        {
            return Err(Error::RecordChanged {
                id: session.clone(),
            });
        }
        let pending = loaded
            .control
            .get(session)
            .map(|c| c.pending_links.clone())
            .unwrap_or_default();
        let mut record = record;
        let mut claims = ClaimsWritten::default();
        if let Some(record) = record.as_mut() {
            // A link written while the model ran, or one waiting on the
            // switch for the first record, is kept: never lost to a pass.
            if let Some(current) = &current {
                for sealed in current
                    .log
                    .iter()
                    .filter(|s| s.state == crate::SegmentState::Sealed)
                {
                    if record.segment(sealed.n.get()) != Some(sealed) {
                        return Err(Error::SealedSegmentChanged {
                            id: session.clone(),
                            n: sealed.n.get(),
                        });
                    }
                }
                merge_links(&mut record.links, &current.links);
            }
            merge_links(&mut record.links, &pending);
            redactor.session(record);
            // A changed redaction configuration cannot rewrite sealed history.
            if let Some(current) = current {
                for sealed in current
                    .log
                    .into_iter()
                    .filter(|s| s.state == crate::SegmentState::Sealed)
                {
                    if let Some(segment) = record.log.iter_mut().find(|s| s.n == sealed.n) {
                        *segment = sealed;
                    }
                }
            }
            if parts.run_candidates {
                claims = open_candidates(tx, record, parts.candidates, redactor)?;
            }
        }
        // The closures, written entries, record, messages, memory rows and
        // journal and the switch all commit in this one transaction, so a
        // later failure leaves none of them.
        let wrote_record = record.is_some();
        // A draft holding text redaction would change is refused before the
        // store plans the rest; its refusals come first in what it reports.
        let checked = parts.memory.map(|memory| {
            let (drafts, sensitive) = check_sensitive_drafts(&memory.drafts, redactor);
            (
                WindowMemory {
                    drafts,
                    ..memory.clone()
                },
                sensitive,
            )
        });
        let memory = tx.sessions().commit_window(
            record.as_ref(),
            parts.messages,
            plan_window(
                checked.as_ref().map(|(memory, _)| memory),
                checked.as_ref().map_or(&[][..], |(_, sensitive)| sensitive),
                record.as_ref(),
            ),
            redactor,
            |state| {
                // An explicit pass can start when not due. Publish the
                // obligation with the checkpoint, so interruption cannot
                // strand its remainder. A live catalog also clears retry
                // backoff: the hooks take it from here.
                if matches!(kind, PassWrite::Checkpoint | PassWrite::CatalogLive)
                    && let Some(control) = state.sessions.get_mut(session).filter(|c| c.on)
                {
                    control.due = true;
                    if matches!(kind, PassWrite::CatalogLive) {
                        control.consecutive_failures = 0;
                        control.retry_after = None;
                    }
                }
                // Only once the record holds them: the links are stored once.
                if wrote_record
                    && !pending.is_empty()
                    && let Some(control) = state.sessions.get_mut(session)
                {
                    control.pending_links.retain(|l| !pending.contains(l));
                }
                match kind {
                    PassWrite::Settle(ran_final) => {
                        control::settle(state, session, ran_final);
                    }
                    PassWrite::CatalogFinal => {
                        // Settle and switch off together. Off does not record
                        // a time.
                        control::settle(state, session, true);
                        let _ = control::switch(
                            state,
                            session,
                            &control::Switch::Off,
                            time::OffsetDateTime::UNIX_EPOCH,
                        );
                    }
                    PassWrite::Checkpoint | PassWrite::CatalogLive => {}
                }
            },
        )?;
        Ok(PassWritten {
            record,
            closed_candidates: claims.closed,
            written_entries: claims.written,
            memory,
        })
    })
}

/// Caller holds the pass lock and has checked the record revision and seals.
/// Closes any open candidates whose tag is no longer in the tag table
/// before counting the open-candidate cap, and returns the closed
/// proposals in id order.
///
/// A claim under a `write` tag (2_design.md §11.6) is written here as an
/// active entry the pass authors, with no proposal, at most
/// [`MAX_COLLECTED`] per call and outside the open-candidate cap, once the
/// same checks as a candidate pass and no active entry of its tag and
/// project, no prior candidate and no entry the pass wrote from the same
/// turn already says it.
fn open_candidates(
    tx: &Tx<'_>,
    record: &SessionRecord,
    drafts: &[crate::pass::PassClaim],
    redactor: &Redactor,
) -> Result<ClaimsWritten> {
    if !tx
        .sessions()
        .control()?
        .get(&record.id)
        .is_some_and(|c| c.on)
    {
        return Ok(ClaimsWritten::default());
    }
    let config = tx.tags().config()?;
    let mut proposals = tx.proposals().all()?;
    let closed =
        crate::proposal::close_retired_tag_candidates(&mut proposals, &config.tags, record.updated);
    if drafts.is_empty() {
        if !closed.is_empty() {
            tx.proposals().put_all(&proposals)?;
        }
        return Ok(ClaimsWritten {
            closed,
            written: Vec::new(),
        });
    }
    let (mut entries, skipped) = tx.entries().all()?;
    if !skipped.is_empty() {
        if !closed.is_empty() {
            tx.proposals().put_all(&proposals)?;
        }
        // Without every entry, absence of an existing statement is unknown.
        tracing::warn!(
            skipped = skipped.len(),
            "not opening candidates: some entries are unreadable"
        );
        return Ok(ClaimsWritten {
            closed,
            written: Vec::new(),
        });
    }
    let mut open = proposals
        .iter()
        .filter(|p| p.kind == ProposalKind::Candidate && p.state == ProposalState::Open)
        .count();
    let mut ids: Vec<_> = entries
        .iter()
        .map(|e| e.id.clone())
        .chain(
            proposals
                .iter()
                .filter(|p| p.kind == ProposalKind::Candidate)
                .map(|p| p.older.clone()),
        )
        .collect();
    let mut next = proposals.iter().map(|p| p.id.number()).max().unwrap_or(0);
    let mut changed = false;
    let (mut proposed, mut written) = (0_usize, Vec::<EntryId>::new());
    for claim in drafts {
        let tag = config.tags.get(&claim.candidate.tag);
        let writes = tag.is_some_and(|tag| tag.capture.writes());
        if writes {
            // The bound on collected entries applies per tag (§15.2).
            let collected = written
                .iter()
                .filter(|id| id.tag() == &claim.candidate.tag)
                .count();
            if collected >= MAX_COLLECTED {
                continue;
            }
        } else if proposed >= MAX_CANDIDATES
            || open >= MAX_OPEN_CANDIDATES
            || open_under(&proposals, &claim.candidate.tag)
                >= tag.map_or(0, |tag| {
                    usize::try_from(tag.capture.max_open()).unwrap_or(usize::MAX)
                })
        {
            continue;
        }
        // The capture gate, repeated under the lock against the config
        // read now (2_design.md §11.2, §15.2): a claim is the own words of
        // a turn by a speaker its tag reads, and a trigger of the tag
        // fires in that turn. The admission persisted is the one
        // computed here.
        let Some(admitted) = own_span(&claim.turn, &claim.candidate.quote)
            .then(|| {
                admission(
                    &config.tags,
                    &claim.candidate.tag,
                    claim.candidate.role,
                    &claim.turn,
                )
            })
            .flatten()
        else {
            continue;
        };
        // The bounds hold the model's words as given; masking a secret
        // may lengthen them, and the masked value is kept.
        if !has_context_and_subject(&claim.candidate) {
            continue;
        }
        let mut draft = claim.candidate.clone();
        draft.admitted = Some(admitted);
        // The context and subject are the model's words: kept
        // redacted when redaction changes them. Only the quote must be
        // the speaker's words unchanged (2_design.md §15.3, §5.3).
        draft.context = draft.context.as_deref().map(|text| redactor.mask(text));
        draft.subject = draft.subject.as_deref().map(|text| redactor.mask(text));
        let draft = &draft;
        let says = candidate_says(draft);
        if !draft.has_valid_provenance()
            || !(8..=400).contains(&draft.quote.chars().count())
            || draft.cite.session != record.id
            || draft.project != record.project
            || draft.quote != redactor.mask(&draft.quote)
            || draft.evidence.is_some()
            || proposals
                .iter()
                .filter_map(|p| p.candidate.as_ref())
                .any(|prior| claim_covers(&candidate_says(prior), &says))
        {
            continue;
        }
        if writes {
            // The shared duplicate check (`covering_claim`: active
            // entries of the tag and project), and any entry the pass
            // already wrote from this turn that is forgotten or holds
            // the same words, so a retried window cannot write it twice.
            if covering_claim(&entries, draft).is_some()
                || entries.iter().any(|entry| written_from(entry, draft))
            {
                continue;
            }
            let Some(outcome) = checked_outcome(claim, &|s| redactor.mask(s)) else {
                continue;
            };
            let id = EntryId::next_in(&draft.tag, &ids);
            ids.push(id.clone());
            let review_after = config
                .tags
                .get(&draft.tag)
                .and_then(|tag| tag.review_after(record.updated.date()));
            let mut entry = pass_entry(id, draft, outcome, record.updated, review_after);
            redactor.entry(&mut entry);
            tx.entries().put(&entry, true)?;
            written.push(entry.id.clone());
            entries.push(entry);
            continue;
        }
        if entries
            .iter()
            .filter(|entry| entry.tag.as_str() != crate::handoff::HANDOFF_TAG)
            .any(|entry| claim_covers(&entry_says(entry), &says))
        {
            continue;
        }
        let Some(number) = next.checked_add(1) else {
            break;
        };
        next = number;
        let id: ProposalId = format!("p-{next}")
            .parse()
            .map_err(|e| Error::corrupt(tx.proposals().path(), e))?;
        let older = EntryId::next_in(&draft.tag, &ids);
        ids.push(older.clone());
        let mut proposal = Proposal {
            id,
            kind: ProposalKind::Candidate,
            older,
            newer: None,
            candidate: Some(draft.clone()),
            reason: format!("Unmarked {} statement: {}", draft.tag, draft.quote),
            quotes: Vec::new(),
            state: ProposalState::Open,
            opened_at: record.updated,
            settled_at: None,
            settled_by: None,
            author: Author::session_pass(),
            updated_by: None,
            settled_author: None,
            closed_reason: None,
            instruction: None,
        };
        proposal.reason = proposal.reason.chars().take(300).collect();
        redactor.proposal(&mut proposal);
        proposals.push(proposal);
        open += 1;
        proposed += 1;
        changed = true;
    }
    if !closed.is_empty() || changed {
        tx.proposals().put_all(&proposals)?;
    }
    Ok(ClaimsWritten { closed, written })
}

fn merge_links(links: &mut Vec<SessionLink>, more: &[SessionLink]) {
    for link in more {
        if !links.contains(link) {
            links.push(link.clone());
        }
    }
}

/// How `PassVerbs::write_pass_window` (scribe-core) leaves the switch.
#[derive(Clone, Copy, Debug)]
pub enum PassWrite {
    /// An intermediate window. Force the session due.
    Checkpoint,
    /// The last window of an ordinary pass. `bool` is `ran_final`.
    Settle(bool),
    /// The last window of a live catalog: stay on and due, clear backoff.
    CatalogLive,
    /// The last window of an ended catalog: settle as final and switch off.
    CatalogFinal,
}

/// What `PassVerbs::write_pass_window` (scribe-core) wrote under the store lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassWritten {
    /// The updated session record, when one was written.
    pub record: Option<SessionRecord>,
    /// Open candidate proposals closed because their tag is absent from the
    /// tag table, in id order.
    pub closed_candidates: Vec<Proposal>,
    /// Entries the pass wrote itself under `write` tags (2_design.md
    /// §11.6), in write order.
    pub written_entries: Vec<EntryId>,
    /// The window's memory write (2_design.md §13.3).
    pub memory: MemoryWritten,
}
