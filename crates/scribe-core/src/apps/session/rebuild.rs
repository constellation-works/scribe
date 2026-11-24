//! Rebuild verbs: a session record rebuilt in windows beside the visible
//! one, under its own lock (2_design.md §15.7).

use super::pass::{PassPayload, PassReport, distill, short_reason, window_warnings};
use crate::checks::{forgotten_refusal, refuse_busy};
use crate::runtime::resolve::refuse_forgotten;
use crate::runtime::{Ctx, PortError, TranscriptState, Unreadable};
use crate::{
    Author, AuthorName, Config, Models, Provider, SegmentOrigin, SegmentState, SessionId,
    SessionStatus, check_agent_author, pass, transcript,
};
use crate::{Error, Redactor, Result, SessionLock, SessionRecord, Store};
use std::collections::BTreeSet;
use time::OffsetDateTime;

/// The rebuild verbs, implemented for [`Store`].
pub trait RebuildVerbs {
    /// Acquire the session's rebuild lock, with the store's bounded wait.
    fn lock_rebuild(&self, record: &SessionRecord) -> Result<SessionLock>;

    /// The partial rebuilt record, if present. This read never locks or writes.
    fn rebuild_checkpoint(&self, original: &SessionRecord) -> Result<Option<SessionRecord>>;

    /// Persist one rebuild window without touching the visible record or control.
    /// Callers hold `lock_rebuild` for the whole rebuild.
    fn checkpoint_rebuild(
        &self,
        record: SessionRecord,
        redactor: &Redactor,
    ) -> Result<SessionRecord>;

    /// Compare and replace under the store lock. `None` means the visible
    /// record advanced: read its new revision and fold in any newer messages
    /// before trying again. Rebuild is the sole sealed-segment replacement route.
    fn finish_rebuild(
        &self,
        record: SessionRecord,
        expected: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>>;
}

impl RebuildVerbs for Store {
    fn lock_rebuild(&self, record: &SessionRecord) -> Result<SessionLock> {
        let view = self.read().sessions().get(&record.id)?;
        if view.forgotten.is_some() {
            return Err(forgotten_refusal(&record.id, "nothing is written to it"));
        }
        let guard = self.session_lock(&format!("rebuild-{}.lock", record.id), "session rebuild")?;
        // Forget may have completed while acquisition was waiting.
        refuse_forgotten(self, &record.id)?;
        Ok(guard)
    }

    fn rebuild_checkpoint(&self, original: &SessionRecord) -> Result<Option<SessionRecord>> {
        self.read().sessions().get(&original.id)?;
        self.read().sessions().checkpoint(original)
    }

    fn checkpoint_rebuild(
        &self,
        mut record: SessionRecord,
        redactor: &Redactor,
    ) -> Result<SessionRecord> {
        self.write(|tx| {
            refuse_busy(&tx.sessions().control()?, &record.id)?;
            tx.sessions().get(&record.id)?;
            redactor.session(&mut record);
            let id = record.id.clone();
            let kept = tx.sessions().keep_checkpoint(&record)?;
            if !kept {
                return Err(Error::SessionNotFound {
                    id,
                    path: tx.sessions().db_path(),
                });
            }
            Ok(record)
        })
    }

    fn finish_rebuild(
        &self,
        mut record: SessionRecord,
        expected: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        self.write(|tx| {
            refuse_busy(&tx.sessions().control()?, &record.id)?;
            let current = tx.sessions().get(&record.id)?.record;
            if current.updated != expected {
                return Ok(None);
            }
            record.started = current.started;
            record.created = current.created;
            record.links = current.links;
            record.active = current.active;
            record.keep_active = current.keep_active;
            record.active_change = None;
            record.cataloged_by = current.cataloged_by;
            redactor.session(&mut record);
            // Activity attribution belongs to the latest visible record, not this rebuild.
            record.active_change = current.active_change;
            // The record and the end of its checkpoint commit together.
            tx.sessions().put_from_checkpoint(&record)?;
            Ok(Some(record))
        })
    }
}

/// `session rebuild`'s audit command name.
pub const REBUILD_COMMAND: &str = "session rebuild";

/// What `session rebuild` runs.
pub struct RebuildInput<'i, F> {
    /// The session, or a prefix of its id.
    pub id: &'i SessionId,
    /// The agent rebuilding it.
    pub author: &'i AuthorName,
    /// `--confirm`: without it nothing is rebuilt.
    pub confirm: bool,
    /// The `Models` adapter for the session's config.
    pub models: F,
}

/// Why `session rebuild` did not run.
#[derive(Debug)]
pub enum RebuildError<E> {
    /// A store error.
    Core(crate::Error),
    /// A port refused or failed.
    Port(E),
    /// A usage error (the record names an unknown provider).
    Usage(String),
    /// `--confirm` was not given.
    ConfirmRequired {
        /// The session.
        id: SessionId,
        /// Its record's segments.
        segments: usize,
        /// Its record's passes.
        passes: u32,
    },
    /// The session's transcript is not on this host.
    TranscriptMissing(SessionId),
    /// The transcript failed its provenance check.
    TranscriptRefused {
        /// The session.
        id: SessionId,
        /// Why.
        reason: String,
    },
    /// The transcript could not be read.
    Read {
        /// The file.
        what: String,
        /// The read error.
        source: std::io::Error,
    },
}

impl<E> From<crate::Error> for RebuildError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

/// Rebuild a session's record from the first transcript message. Each
/// window is written to an isolated checkpoint; only the complete rebuild
/// replaces the record.
///
/// # Errors
///
/// [`RebuildError`]. A failed model window is a `failed` payload with the
/// checkpoint kept, not an error.
pub fn run<E: PortError, M: Models>(
    ctx: &Ctx<'_, E>,
    input: RebuildInput<'_, impl FnOnce(&Config) -> M>,
) -> Result<PassReport, RebuildError<E>> {
    let RebuildInput {
        id: input,
        author,
        confirm,
        models,
    } = input;
    check_agent_author(author)?;
    let store = ctx.store;
    let id = store.read().sessions().resolve(input)?;
    // Refused before the confirm check, the transcript and any model call.
    refuse_forgotten(store, &id)?;
    let mut original = store.read().sessions().get(&id)?.record;
    let provider: Provider = original
        .provider
        .as_str()
        .parse()
        .map_err(RebuildError::Usage)?;
    if !confirm {
        return Err(RebuildError::ConfirmRequired {
            id: id.clone(),
            segments: original.log.len(),
            passes: original.passes,
        });
    }
    if store
        .read()
        .sessions()
        .control()?
        .get(&id)
        .is_some_and(|c| c.on && c.due)
    {
        return Err(crate::Error::SessionBusy(id.clone()).into());
    }
    let path = ctx
        .transcripts
        .files(provider)
        .map_err(RebuildError::Port)?
        .into_iter()
        .find(|(found, _)| *found == id)
        .map(|(_, path)| path)
        .ok_or_else(|| RebuildError::TranscriptMissing(id.clone()))?;
    // An early refusal before the lock and the model are set up; it reads
    // nothing. The loop below checks and reads each pass through one handle.
    match ctx.transcripts.check(provider, &path, &id) {
        Ok(TranscriptState::Present) => {}
        Ok(TranscriptState::Missing) => return Err(RebuildError::TranscriptMissing(id.clone())),
        Err(reason) => {
            return Err(RebuildError::TranscriptRefused {
                id: id.clone(),
                reason,
            });
        }
    }
    let config = store.read().tags().config()?.for_provider(provider);
    let redactor = ctx
        .redaction
        .redactor(&config)
        .map_err(RebuildError::Port)?;
    let adapter = models(&config);
    let _rebuild = store.lock_rebuild(&original)?;
    // Re-read after taking the per-session lock: another rebuilder may have finished.
    original = store.read().sessions().get(&id)?.record;
    let mut previous = store.rebuild_checkpoint(&original)?;
    let mut expected = previous.as_ref().map_or(original.updated, |r| r.updated);
    let mut report = PassPayload {
        session: id.to_string(),
        result: "written",
        error_code: None,
        passes: None,
        dropped_terms: 0,
        warnings: Vec::new(),
        windows: 0,
        gave_up: false,
        error: None,
        segments_sealed: 0,
        segments: previous.as_ref().map(|r| r.log.len()),
        author: Some(Author {
            kind: crate::AuthorKind::Agent,
            name: author.clone(),
        }),
        memory_written: 0,
        memory_promoted: 0,
        memory_proposed: 0,
        memory_pending: 0,
        memory_refused: std::collections::BTreeMap::new(),
    };
    let records = super::list_sessions(
        &store.read(),
        &crate::SessionFilter {
            include_inactive: true,
            ..Default::default()
        },
        None,
    )?
    .listing
    .items;
    let linked = pass::linked_context(&original.links, &records);
    let known_topics: BTreeSet<String> = records
        .iter()
        .flat_map(|r| r.topics.as_slice().iter().map(ToString::to_string))
        .collect();
    let known_terms: BTreeSet<String> = records
        .iter()
        .flat_map(|r| r.terms.iter().map(|t| t.term.clone()))
        .collect();
    loop {
        // Re-check provenance when re-reading a growing transcript, on the
        // handle the bytes come from.
        let bytes = match ctx.transcripts.read_checked(provider, &path, &id) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(RebuildError::TranscriptMissing(id.clone())),
            Err(Unreadable::Refused(reason)) => {
                return Err(RebuildError::TranscriptRefused {
                    id: id.clone(),
                    reason,
                });
            }
            Err(Unreadable::Io(source)) => {
                return Err(RebuildError::Read {
                    what: path.display().to_string(),
                    source,
                });
            }
        };
        let messages = transcript::parse(&bytes);
        let conversation = pass::conversation(&messages);
        if conversation.is_empty() {
            return Err(RebuildError::TranscriptMissing(id.clone()));
        }
        let mut offset = pass::start_after(
            &conversation,
            previous.as_ref().and_then(|p| p.scribed_through.as_deref()),
        );
        if previous.is_some() && offset == 0 {
            previous = None;
        }
        while offset < conversation.len() {
            let window =
                pass::window_with_limits(&conversation[offset..], None, config.catalog_window);
            let end = offset + window.messages.len();
            let now = ctx.clock.now();
            let window_meta: Vec<_> = window
                .messages
                .iter()
                .map(|m| {
                    (
                        m.uuid.clone(),
                        messages
                            .iter()
                            .find(|sent| sent.uuid == m.uuid)
                            .map_or(now, |sent| sent.at),
                    )
                })
                .collect();
            let final_window = end == conversation.len()
                && (original.status == SessionStatus::Final
                    || store
                        .read()
                        .sessions()
                        .control()?
                        .get(&id)
                        .is_some_and(|s| s.final_pass));
            let mut topics = known_topics.clone();
            let mut terms = known_terms.clone();
            if let Some(record) = &previous {
                topics.extend(record.topics.as_slice().iter().map(ToString::to_string));
                terms.extend(record.terms.iter().map(|t| t.term.clone()));
            }
            let request = pass::request(
                &config.models.session,
                previous.as_ref(),
                linked.clone(),
                window,
                topics.into_iter().take(200).collect(),
                terms.into_iter().take(200).collect(),
                final_window,
                config.voice(crate::voice::ModelUse::Session),
            );
            let answer = match distill(
                &id,
                request,
                &conversation[..end],
                &adapter,
                config.voice(crate::voice::ModelUse::SessionHeader),
            ) {
                Ok(answer) => answer,
                Err(error) => {
                    report.result = "failed";
                    report.error_code = Some(error.code);
                    report.error = Some(short_reason(&redactor, &error.message));
                    return Ok(PassReport {
                        passes: vec![report],
                        remaining: 0,
                        notices: vec![String::from(
                            "rebuild checkpoint kept; repeat session rebuild with --confirm to resume",
                        )],
                        catalog: false,
                        switch_written: false,
                        persisted_targets: Vec::new(),
                        memory_targets: Vec::new(),
                    });
                }
            };
            let dropped = answer.dropped_terms;
            let warnings = window_warnings(report.windows + 1, &answer);
            let meta = pass::PassMeta {
                id: id.clone(),
                project: original.project.clone(),
                provider: original.provider.clone(),
                host: original.host.clone(),
                cwd: original.cwd.clone(),
                started: original.started,
                on_since: original.on_since,
                through: window_meta.last().map(|(id, _)| id.clone()),
                first_message: window_meta.first().map(|(id, _)| id.clone()),
                from: window_meta.first().map_or(now, |(_, at)| *at),
                to: window_meta.last().map_or(now, |(_, at)| *at),
                now,
                final_pass: final_window,
                window: window_meta,
                segment_windows: config.segment_windows,
                origin: SegmentOrigin::Rebuild,
            };
            let sealed_before = previous.as_ref().map_or(0, |p| {
                p.log
                    .iter()
                    .filter(|s| s.state == SegmentState::Sealed)
                    .count()
            });
            let mut record = pass::record(previous.as_ref(), answer, meta);
            record.created = original.created;
            record.updated = expected;
            let record = store.checkpoint_rebuild(record, &redactor)?;
            report.segments_sealed += record
                .log
                .iter()
                .filter(|s| s.state == SegmentState::Sealed)
                .count()
                .saturating_sub(sealed_before);
            report.segments = Some(record.log.len());
            report.passes = Some(record.passes);
            report.windows += 1;
            report.dropped_terms += dropped;
            report.warnings.extend(warnings);
            previous = Some(record);
            offset = end;
        }
        let Some(mut finished) = previous.clone() else {
            return Err(RebuildError::TranscriptMissing(id.clone()));
        };
        let switch = store.read().sessions().control()?;
        let final_pass = original.status == SessionStatus::Final
            || switch.get(&id).is_some_and(|s| s.final_pass);
        let sealed_at_finish = final_pass && finished.open_segment().is_some();
        if final_pass {
            finished = pass::finalized(&finished, ctx.clock.now());
        }
        finished.updated = ctx.clock.now();
        finished.rebuilt_by = Some(Author::agent(author.clone()));
        match store.finish_rebuild(finished, expected, &redactor)? {
            Some(written) => {
                report.segments_sealed += usize::from(sealed_at_finish);
                report.passes = Some(written.passes);
                report.segments = Some(written.log.len());
                return Ok(PassReport {
                    passes: vec![report],
                    remaining: 0,
                    notices: Vec::new(),
                    catalog: false,
                    switch_written: false,
                    persisted_targets: Vec::new(),
                    memory_targets: Vec::new(),
                });
            }
            None => {
                original = store.read().sessions().get(&id)?.record;
                expected = original.updated;
                // The next iteration re-reads the transcript and folds in any newer messages.
            }
        }
    }
}

/// `session rebuild`'s errors, each the variant it was before the flow moved.
impl From<RebuildError<crate::runtime::errors::CommandError>>
    for crate::runtime::errors::CommandError
{
    fn from(err: RebuildError<crate::runtime::errors::CommandError>) -> Self {
        match err {
            RebuildError::Core(e) => Self::Core(e),
            RebuildError::Port(e) => e,
            RebuildError::Usage(message) => Self::Usage(message),
            RebuildError::ConfirmRequired {
                id,
                segments,
                passes,
            } => Self::ConfirmRequired {
                id,
                segments,
                passes,
            },
            RebuildError::TranscriptMissing(id) => Self::TranscriptMissing(id),
            RebuildError::TranscriptRefused { id, reason } => {
                Self::TranscriptRefused { id, reason }
            }
            RebuildError::Read { what, source } => Self::Read { what, source },
        }
    }
}
