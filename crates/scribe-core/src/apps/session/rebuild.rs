//! Rebuild verbs: a session record rebuilt in windows beside the visible
//! one, as the `scribe-session-rebuild` Orbit job (2_design.md §15.7,
//! §19.3.4).
//!
//! `prepare` refuses what `session rebuild` always refused (a reserved
//! author, a forgotten session, a missing `--confirm`, a session a pass
//! owns, a missing or unproven transcript) and emits the next window from
//! the private checkpoint, or that the checkpoint is ready to finish.
//! `apply` re-runs those checks, recomputes the window from the transcript
//! and the checkpoint the basis names, verifies the answer, and writes only
//! the checkpoint, under the store lock and only while that checkpoint is
//! unchanged. `finish` promotes a complete checkpoint when the visible
//! record is still the revision the rebuild started from; when it moved,
//! the checkpoint is rebased onto it and the next prepare folds in what is
//! new. No lock is held across steps: the job's `max_active_runs: 1` and
//! these optimistic checks replace the old `rebuild-<id>.lock` (§19.6).

use super::pass::{
    ActivityAnswer, PassError, PassPayload, PassReport, mask_value, masked_input, model_code, seal,
    short_reason, window_warnings,
};
use super::rules::refuse_busy;
use crate::apps::tag::ConfigReads;
use crate::models::{SessionAnswer, SessionHeader, SessionRequest};
use crate::runtime::resolve::refuse_forgotten;
use crate::runtime::{Ctx, PortError, TranscriptState, Unreadable};
use crate::store::forgotten_refusal;
use crate::{
    Author, AuthorName, Config, Provider, SegmentOrigin, SegmentState, SessionId, SessionStatus,
    check_agent_author, pass, transcript,
};
use crate::{Error, Redactor, Result, SessionRecord, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use time::OffsetDateTime;

/// The topics and terms offered to the model, at most.
const MAX_LISTED: usize = 200;

/// The rebuild verbs, implemented for [`Store`].
pub trait RebuildVerbs {
    /// The partial rebuilt record, if present. This read never locks or writes.
    fn rebuild_checkpoint(&self, original: &SessionRecord) -> Result<Option<SessionRecord>>;

    /// Persist one rebuild window without touching the visible record or
    /// control, only while the stored checkpoint is still `expected`
    /// (`None`: no checkpoint). `Ok(None)` means it changed and nothing was
    /// written.
    fn checkpoint_rebuild(
        &self,
        record: SessionRecord,
        expected: Option<&Mark>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>>;

    /// Compare and replace under the store lock, while the stored checkpoint
    /// is still `mark`. [`Finished::Rebased`] means the visible record
    /// advanced: the checkpoint now expects its new revision, and the next
    /// window folds in any newer messages. Rebuild is the sole sealed-segment
    /// replacement route.
    fn finish_rebuild(
        &self,
        record: SessionRecord,
        mark: &Mark,
        redactor: &Redactor,
    ) -> Result<Finished>;
}

/// What [`RebuildVerbs::finish_rebuild`] did.
#[derive(Debug)]
pub enum Finished {
    /// The record was replaced.
    Written(Box<SessionRecord>),
    /// The visible record moved on; the checkpoint now expects this revision.
    Rebased(OffsetDateTime),
    /// The checkpoint is not the one the finish was prepared from.
    Stale,
}

/// A stored checkpoint's identity: where it is scribed through and the
/// visible revision it expects.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Mark {
    through: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

impl Mark {
    fn of(record: &SessionRecord) -> Self {
        Self {
            through: record.scribed_through.clone(),
            updated: record.updated,
        }
    }
}

/// The stored checkpoint of `id`, read in the write transaction.
fn stored_mark(tx: &crate::Tx<'_>, id: &SessionId) -> Result<(SessionRecord, Option<Mark>)> {
    let loaded = tx.sessions().loaded()?;
    if loaded.forgotten.contains_key(id) {
        return Err(forgotten_refusal(id, "nothing is written to it"));
    }
    let current = tx.sessions().get(id)?.record;
    let mark = tx.sessions().checkpoint(&current)?.as_ref().map(Mark::of);
    Ok((current, mark))
}

impl RebuildVerbs for Store {
    fn rebuild_checkpoint(&self, original: &SessionRecord) -> Result<Option<SessionRecord>> {
        self.read().sessions().get(&original.id)?;
        self.read().sessions().checkpoint(original)
    }

    fn checkpoint_rebuild(
        &self,
        mut record: SessionRecord,
        expected: Option<&Mark>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        self.write(|tx| {
            refuse_busy(&tx.sessions().control()?, &record.id)?;
            let (_, mark) = stored_mark(tx, &record.id)?;
            if mark.as_ref() != expected {
                return Ok(None);
            }
            redactor.session(&mut record);
            let id = record.id.clone();
            let kept = tx.sessions().keep_checkpoint(&record)?;
            if !kept {
                return Err(Error::SessionNotFound {
                    id,
                    path: tx.sessions().db_path(),
                });
            }
            Ok(Some(record))
        })
    }

    fn finish_rebuild(
        &self,
        mut record: SessionRecord,
        mark: &Mark,
        redactor: &Redactor,
    ) -> Result<Finished> {
        self.write(|tx| {
            refuse_busy(&tx.sessions().control()?, &record.id)?;
            let (current, stored) = stored_mark(tx, &record.id)?;
            if stored.as_ref() != Some(mark) {
                return Ok(Finished::Stale);
            }
            if current.updated != mark.updated {
                let Some(mut checkpoint) = tx.sessions().checkpoint(&current)? else {
                    return Ok(Finished::Stale);
                };
                checkpoint.updated = current.updated;
                tx.sessions().keep_checkpoint(&checkpoint)?;
                return Ok(Finished::Rebased(current.updated));
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
            Ok(Finished::Written(Box::new(record)))
        })
    }
}

/// `session rebuild`'s audit command name.
pub const REBUILD_COMMAND: &str = "session rebuild";

/// What `session rebuild` checks before it names the job.
pub struct RebuildInput<'i> {
    /// The session, or a prefix of its id.
    pub id: &'i SessionId,
    /// The agent rebuilding it.
    pub author: &'i AuthorName,
    /// `--confirm`: without it nothing is rebuilt.
    pub confirm: bool,
}

/// Why a rebuild step did not run.
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
    /// Invalid continuation or answer input.
    Input(&'static str),
    /// The model's step failed: the checkpoint is kept.
    Step(PassError),
}

impl<E> From<crate::Error> for RebuildError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

/// The job's input, as each prepare receives it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RebuildPrepareInput {
    /// The session, or a prefix of its id.
    pub session: SessionId,
    /// The agent rebuilding it.
    pub author: AuthorName,
    /// Without it nothing is rebuilt.
    #[serde(default)]
    pub confirm: bool,
    /// The previous iteration's cursor (a single fan-in result).
    #[serde(default)]
    pub progress: Vec<RebuildProgress>,
}

/// The continuation field of an apply result.
#[derive(Debug, Deserialize)]
pub struct RebuildProgress {
    cursor: RebuildCursor,
}

/// The run's report so far. Prepare, apply and finish return it sealed and
/// refuse one whose fields no longer match the seal.
#[derive(Clone, Debug, Deserialize)]
pub struct RebuildCursor {
    session: SessionId,
    author: AuthorName,
    windows: usize,
    segments_sealed: usize,
    dropped_terms: usize,
    warnings: Vec<String>,
    #[serde(default)]
    seal: String,
}

#[derive(Serialize)]
struct CursorFields<'a> {
    session: &'a SessionId,
    author: &'a AuthorName,
    windows: usize,
    segments_sealed: usize,
    dropped_terms: usize,
    warnings: &'a [String],
}

impl Serialize for RebuildCursor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Sealed<'a> {
            #[serde(flatten)]
            fields: CursorFields<'a>,
            seal: String,
        }
        Sealed {
            fields: self.fields(),
            seal: self.seal_of(),
        }
        .serialize(serializer)
    }
}

impl RebuildCursor {
    fn new(session: SessionId, author: AuthorName) -> Self {
        Self {
            session,
            author,
            windows: 0,
            segments_sealed: 0,
            dropped_terms: 0,
            warnings: Vec::new(),
            seal: String::new(),
        }
    }

    fn fields(&self) -> CursorFields<'_> {
        CursorFields {
            session: &self.session,
            author: &self.author,
            windows: self.windows,
            segments_sealed: self.segments_sealed,
            dropped_terms: self.dropped_terms,
            warnings: &self.warnings,
        }
    }

    fn seal_of(&self) -> String {
        seal("rebuild-cursor", &json!(self.fields()))
    }

    fn checked<E>(self) -> std::result::Result<Self, RebuildError<E>> {
        if self.seal == self.seal_of() {
            Ok(self)
        } else {
            Err(RebuildError::Input(
                "the rebuild cursor was changed since scribe returned it",
            ))
        }
    }
}

/// Optimistic basis from prepare. Carries no transcript text. `digest`
/// binds the transcript window; `seal` binds every other field and the
/// cursor it was prepared with.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RebuildBasis {
    session_id: SessionId,
    author: AuthorName,
    /// The stored checkpoint prepare read; `None` when there was none.
    checkpoint: Option<Mark>,
    /// The visible revision the rebuild expects.
    #[serde(with = "time::serde::rfc3339")]
    expected_updated: OffsetDateTime,
    /// Every message is in the checkpoint: finish, not a window.
    ready: bool,
    window_end_uuid: Option<String>,
    final_window: bool,
    digest: String,
    #[serde(default)]
    cursor_seal: String,
    #[serde(default)]
    seal: String,
}

impl RebuildBasis {
    fn seal_of(&self) -> String {
        let updated = self
            .expected_updated
            .format(&time::format_description::well_known::Rfc3339)
            .ok();
        seal(
            "rebuild-basis",
            &json!([
                self.session_id,
                self.author,
                self.checkpoint,
                updated,
                self.ready,
                self.window_end_uuid,
                self.final_window,
                self.digest,
                self.cursor_seal,
            ]),
        )
    }

    fn sealed(mut self) -> Self {
        self.seal = self.seal_of();
        self
    }

    fn checked<E>(self, cursor: &RebuildCursor) -> std::result::Result<Self, RebuildError<E>> {
        if self.seal != self.seal_of() {
            return Err(RebuildError::Input(
                "the prepared rebuild basis was changed since scribe returned it",
            ));
        }
        if self.cursor_seal != cursor.seal_of()
            || self.session_id != cursor.session
            || self.author != cursor.author
        {
            return Err(RebuildError::Input(
                "the rebuild cursor is not the one the basis was prepared with",
            ));
        }
        Ok(self)
    }

    /// The session this basis names.
    #[must_use]
    pub fn session(&self) -> &SessionId {
        &self.session_id
    }

    /// The agent rebuilding it.
    #[must_use]
    pub fn author(&self) -> &AuthorName {
        &self.author
    }
}

/// The next window, or that the checkpoint is ready to finish.
#[derive(Debug, Serialize)]
pub struct RebuildPrepared {
    /// Exactly one window for distillation when true.
    pub needs_model: bool,
    /// Every message is in the checkpoint: finish runs.
    pub ready_to_finish: bool,
    /// The visible revision the rebuild expects.
    #[serde(with = "time::serde::rfc3339")]
    pub expected_updated: OffsetDateTime,
    /// Required for apply and finish.
    pub basis: RebuildBasis,
    /// The run's report so far.
    pub cursor: RebuildCursor,
    /// Bounded, redacted input for the distill activity.
    pub window_request: Option<Value>,
}

/// Apply a first answer, retry one refusal, or carry the cursor.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RebuildApplyInput {
    /// Basis from prepare.
    pub basis: RebuildBasis,
    /// Cursor from prepare.
    pub cursor: RebuildCursor,
    /// First distillation result.
    #[serde(default)]
    pub answer: Option<ActivityAnswer<SessionAnswer>>,
    /// The first distillation's position-aligned worker errors.
    #[serde(default)]
    pub errors: Vec<Option<String>>,
    /// Header compression result.
    #[serde(default)]
    pub header: Option<ActivityAnswer<SessionHeader>>,
    /// Whole-window retry result.
    #[serde(default)]
    pub retry_answer: Option<ActivityAnswer<SessionAnswer>>,
    /// The retry's position-aligned worker errors.
    #[serde(default)]
    pub retry_errors: Vec<Option<String>>,
    /// Only the final attempt may use deterministic repair.
    #[serde(default)]
    pub repair: bool,
    /// The first apply's status, when this is the final attempt.
    pub previous_status: Option<RebuildStatus>,
    /// The first refusal, included in the final diagnostic.
    pub correction: Option<String>,
}

/// Result of checking one rebuild window.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RebuildStatus {
    /// Nothing to apply: the basis is ready to finish, or this is a final
    /// attempt after a first one that needed none.
    Idle,
    /// The window was written to the checkpoint.
    Written,
    /// The checkpoint or transcript changed since prepare; nothing was
    /// written, and the next prepare reads it again.
    Stale,
    /// Compress only the header.
    RetryHeader,
    /// Retry the full window with a correction.
    RetryWindow,
}

/// Apply status, cursor and at most one targeted retry request.
#[derive(Debug, Serialize)]
pub struct RebuildApplied {
    /// Outcome of the check.
    pub status: RebuildStatus,
    /// Continuation for the next prepare and the finish.
    pub cursor: RebuildCursor,
    /// Stable refusal detail for the retry.
    pub correction: Option<String>,
    /// Header compression request on a header-only refusal.
    pub header_request: Option<Value>,
    /// Corrected request on a full-window refusal.
    pub window_request: Option<Value>,
}

impl RebuildApplied {
    fn new(status: RebuildStatus, cursor: RebuildCursor) -> Self {
        Self {
            status,
            cursor,
            correction: None,
            header_request: None,
            window_request: None,
        }
    }
}

/// Finish a ready checkpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RebuildFinishInput {
    /// Basis from prepare.
    pub basis: RebuildBasis,
    /// The latest cursor (a single fan-in result).
    pub progress: Vec<RebuildProgress>,
}

/// Whether the rebuild is done, and its report when it is.
#[derive(Debug, Serialize)]
pub struct RebuildFinished {
    /// End the job's loop.
    pub done: bool,
    /// The record was replaced.
    pub finished: bool,
    /// The visible revision the rebased checkpoint now expects.
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_expected: Option<OffsetDateTime>,
    /// The rebuild's report, once written.
    pub pass: Option<PassPayload>,
}

/// A session admitted for a rebuild step: every refusal `session rebuild`
/// made before its first window.
struct Admitted {
    id: SessionId,
    original: SessionRecord,
    provider: Provider,
    path: PathBuf,
    config: Config,
    redactor: Redactor,
}

fn admit<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: &SessionId,
    author: &AuthorName,
    confirm: bool,
) -> std::result::Result<Admitted, RebuildError<E>> {
    check_agent_author(author)?;
    let store = ctx.store;
    let id = store.read().sessions().resolve(input)?;
    // Refused before the confirm check, the transcript and any model call.
    refuse_forgotten(store, &id)?;
    let original = store.read().sessions().get(&id)?.record;
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
    refuse_busy(&store.read().sessions().control()?, &id)?;
    let path = ctx
        .transcripts
        .files(provider)
        .map_err(RebuildError::Port)?
        .into_iter()
        .find(|(found, _)| *found == id)
        .map(|(_, path)| path)
        .ok_or_else(|| RebuildError::TranscriptMissing(id.clone()))?;
    let config = store.read().tags().config()?.for_provider(provider);
    let redactor = ctx
        .redaction
        .redactor(&config)
        .map_err(RebuildError::Port)?;
    Ok(Admitted {
        id,
        original,
        provider,
        path,
        config,
        redactor,
    })
}

impl Admitted {
    /// The transcript's messages, its provenance re-checked on the handle
    /// the bytes come from.
    fn read<E: PortError>(&self, ctx: &Ctx<'_, E>) -> std::result::Result<Transcript, RebuildError<E>> {
        let bytes = match ctx
            .transcripts
            .read_checked(self.provider, &self.path, &self.id)
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(RebuildError::TranscriptMissing(self.id.clone())),
            Err(Unreadable::Refused(reason)) => {
                return Err(RebuildError::TranscriptRefused {
                    id: self.id.clone(),
                    reason,
                });
            }
            Err(Unreadable::Io(source)) => {
                return Err(RebuildError::Read {
                    what: self.path.display().to_string(),
                    source,
                });
            }
        };
        let messages = transcript::parse(&bytes);
        let conversation = pass::conversation(&messages);
        if conversation.is_empty() {
            return Err(RebuildError::TranscriptMissing(self.id.clone()));
        }
        Ok(Transcript {
            cwd: transcript::cwd(&bytes).unwrap_or_default(),
            messages,
            conversation,
        })
    }

    /// The stored checkpoint, and the record the next window continues:
    /// none when the checkpoint's cursor is no longer in the transcript.
    fn checkpoint<E>(
        &self,
        ctx: &Ctx<'_, E>,
        data: &Transcript,
    ) -> std::result::Result<(Option<Mark>, Option<SessionRecord>, usize), RebuildError<E>> {
        let stored = ctx.store.rebuild_checkpoint(&self.original)?;
        let mark = stored.as_ref().map(Mark::of);
        let offset = pass::start_after(
            &data.conversation,
            stored.as_ref().and_then(|p| p.scribed_through.as_deref()),
        );
        let previous = stored.filter(|_| offset > 0);
        Ok((mark, previous, offset))
    }

    fn final_pass<E>(&self, ctx: &Ctx<'_, E>) -> Result<bool> {
        Ok(self.original.status == SessionStatus::Final
            || ctx
                .store
                .read()
                .sessions()
                .control()?
                .get(&self.id)
                .is_some_and(|s| s.final_pass))
    }

    /// The window after `offset`, ending at `end_uuid` when replaying a
    /// prepared one.
    fn window<E>(
        &self,
        ctx: &Ctx<'_, E>,
        data: &Transcript,
        previous: Option<&SessionRecord>,
        offset: usize,
        end_uuid: Option<&str>,
    ) -> std::result::Result<Option<Window>, RebuildError<E>> {
        let limit = match end_uuid {
            Some(uuid) => match data.conversation[offset..]
                .iter()
                .position(|m| m.uuid == uuid)
            {
                Some(n) => offset + n + 1,
                None => return Ok(None),
            },
            None => data.conversation.len(),
        };
        let window = pass::window_with_limits(
            &data.conversation[offset..limit],
            None,
            self.config.catalog_window,
        );
        let Some(through) = window.messages.last().map(|m| m.uuid.clone()) else {
            return Ok(None);
        };
        let end = offset + window.messages.len();
        let final_window = end == data.conversation.len() && self.final_pass(ctx)?;
        let records = super::list_sessions(
            &ctx.store.read(),
            &crate::SessionFilter {
                include_inactive: true,
                ..Default::default()
            },
            None,
        )?
        .listing
        .items;
        let linked = pass::linked_context(&self.original.links, &records);
        let mut topics: BTreeSet<String> = BTreeSet::new();
        let mut terms: BTreeSet<String> = BTreeSet::new();
        for record in records.iter().chain(previous) {
            topics.extend(record.topics.as_slice().iter().map(ToString::to_string));
            terms.extend(record.terms.iter().map(|t| t.term.clone()));
        }
        let request = pass::request(
            &self.config.models.session,
            previous,
            linked,
            window,
            topics.into_iter().take(MAX_LISTED).collect(),
            terms.into_iter().take(MAX_LISTED).collect(),
            final_window,
            self.config.voice(crate::voice::ModelUse::Session),
        );
        let digest = digest(data, &request, &through);
        Ok(Some(Window {
            request,
            offset,
            end,
            through,
            final_window,
            digest,
        }))
    }
}

struct Transcript {
    messages: Vec<transcript::Message>,
    conversation: Vec<crate::models::PassMessage>,
    cwd: String,
}

struct Window {
    request: SessionRequest,
    offset: usize,
    end: usize,
    through: String,
    final_window: bool,
    digest: String,
}

/// Bind the window's messages, the cwd and the transcript through its end.
fn digest(data: &Transcript, request: &SessionRequest, through: &str) -> String {
    let upto = data
        .messages
        .iter()
        .position(|m| m.uuid == through)
        .map_or(data.messages.as_slice(), |n| &data.messages[..=n]);
    crate::text::bytes_sha256(
        json!([request.messages, data.cwd, upto])
            .to_string()
            .as_bytes(),
    )
}

/// `session rebuild`: every refusal a rebuild makes before its first
/// window, without reading the transcript. The windows run as the
/// `scribe-session-rebuild` job.
///
/// # Errors
///
/// [`RebuildError`].
pub fn check<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: &RebuildInput<'_>,
) -> std::result::Result<PassReport, RebuildError<E>> {
    let admitted = admit(ctx, input.id, input.author, input.confirm)?;
    match ctx
        .transcripts
        .check(admitted.provider, &admitted.path, &admitted.id)
    {
        Ok(TranscriptState::Present) => {}
        Ok(TranscriptState::Missing) => return Err(RebuildError::TranscriptMissing(admitted.id)),
        Err(reason) => {
            return Err(RebuildError::TranscriptRefused {
                id: admitted.id,
                reason,
            });
        }
    }
    let job_input = json!({
        "session": admitted.id,
        "author": input.author,
        "confirm": true,
    });
    Ok(PassReport {
        passes: Vec::new(),
        remaining: 1,
        notices: vec![format!(
            "the rebuild runs as an Orbit job; run orbit job run scribe-session-rebuild --input '{job_input}'"
        )],
        catalog: false,
        switch_written: false,
        persisted_targets: Vec::new(),
        memory_targets: Vec::new(),
    })
}

/// Prepare the next rebuild window, or report the checkpoint ready.
///
/// # Errors
///
/// [`RebuildError`]: a refusal, the transcript, or the store.
pub(crate) fn prepare<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: RebuildPrepareInput,
) -> std::result::Result<RebuildPrepared, RebuildError<E>> {
    let admitted = admit(ctx, &input.session, &input.author, input.confirm)?;
    let cursor = match input.progress.into_iter().next() {
        Some(progress) => {
            let cursor = progress.cursor.checked()?;
            if cursor.session != admitted.id || cursor.author != input.author {
                return Err(RebuildError::Input(
                    "the rebuild cursor does not name this session and author",
                ));
            }
            cursor
        }
        None => RebuildCursor::new(admitted.id.clone(), input.author.clone()),
    };
    let data = admitted.read(ctx)?;
    let (mark, previous, offset) = admitted.checkpoint(ctx, &data)?;
    let expected = mark.as_ref().map_or(admitted.original.updated, |m| m.updated);
    let mut basis = RebuildBasis {
        session_id: admitted.id.clone(),
        author: input.author,
        checkpoint: mark,
        expected_updated: expected,
        ready: true,
        window_end_uuid: None,
        final_window: false,
        digest: String::new(),
        cursor_seal: cursor.seal_of(),
        seal: String::new(),
    };
    let mut window_request = None;
    if offset < data.conversation.len() {
        let window = admitted
            .window(ctx, &data, previous.as_ref(), offset, None)?
            .ok_or_else(|| RebuildError::TranscriptMissing(admitted.id.clone()))?;
        basis.ready = false;
        basis.window_end_uuid = Some(window.through.clone());
        basis.final_window = window.final_window;
        basis.digest.clone_from(&window.digest);
        window_request = Some(masked_input(&window.request, &admitted.redactor));
    }
    Ok(RebuildPrepared {
        needs_model: !basis.ready,
        ready_to_finish: basis.ready,
        expected_updated: expected,
        basis: basis.sealed(),
        cursor,
        window_request,
    })
}

/// Check an answer for the prepared window and write it to the checkpoint.
///
/// # Errors
///
/// [`RebuildError`]: a refusal, the transcript, the store, or the model's
/// failure (`Step`), which keeps the checkpoint.
#[allow(clippy::too_many_lines)]
pub(crate) fn apply<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: RebuildApplyInput,
) -> std::result::Result<RebuildApplied, RebuildError<E>> {
    let RebuildApplyInput {
        basis,
        cursor,
        answer,
        errors,
        header,
        retry_answer,
        retry_errors,
        repair,
        previous_status,
        correction,
    } = input;
    let mut cursor = cursor.checked()?;
    let basis = basis.checked(&cursor)?;
    if repair {
        match previous_status {
            Some(RebuildStatus::RetryWindow | RebuildStatus::RetryHeader) => {}
            Some(RebuildStatus::Idle | RebuildStatus::Written | RebuildStatus::Stale) => {
                return Ok(RebuildApplied::new(RebuildStatus::Idle, cursor));
            }
            None => {
                return Err(RebuildError::Input(
                    "repair requires the first apply status",
                ));
            }
        }
    } else if previous_status.is_some() {
        return Err(RebuildError::Input(
            "previous_status requires a final retry",
        ));
    }
    if basis.ready {
        return Ok(RebuildApplied::new(RebuildStatus::Idle, cursor));
    }
    let admitted = admit(ctx, &basis.session_id, &basis.author, true)?;
    let first = answer.and_then(ActivityAnswer::first);
    let header = header.and_then(ActivityAnswer::first);
    let (answer, failed_with) = if previous_status == Some(RebuildStatus::RetryWindow) {
        (retry_answer.and_then(ActivityAnswer::first), retry_errors)
    } else if previous_status == Some(RebuildStatus::RetryHeader) {
        (first.clone().filter(|_| header.is_some()), retry_errors)
    } else {
        (first.clone(), errors)
    };
    let data = admitted.read(ctx)?;
    let (mark, previous, offset) = admitted.checkpoint(ctx, &data)?;
    let stale = |cursor| Ok(RebuildApplied::new(RebuildStatus::Stale, cursor));
    if mark != basis.checkpoint
        || mark.as_ref().map_or(admitted.original.updated, |m| m.updated) != basis.expected_updated
    {
        return stale(cursor);
    }
    let Some(window) = admitted.window(
        ctx,
        &data,
        previous.as_ref(),
        offset,
        basis.window_end_uuid.as_deref(),
    )?
    else {
        return stale(cursor);
    };
    if window.digest != basis.digest || window.final_window != basis.final_window {
        return stale(cursor);
    }
    // The activity failed: the checkpoint is kept and the run ends with
    // the failure's code.
    let Some(mut answer) = answer else {
        let error = failed_with.into_iter().flatten().next();
        let code = model_code(None, error.as_deref());
        let message = error.unwrap_or_else(|| "the session distillation activity returned no answer".into());
        return Err(RebuildError::Step(PassError::new(
            code,
            short_reason(&admitted.redactor, &message),
        )));
    };
    let conversation = &data.conversation[..window.end];
    let voice = || admitted.config.voice(crate::voice::ModelUse::SessionHeader);
    // The final attempt recomputes the first apply's status from the first
    // answer, so a forwarded status cannot skip the check or the retry.
    if let Some(status) = previous_status {
        let Some(first) = first else {
            return Err(RebuildError::Input(
                "a final retry requires the first session answer",
            ));
        };
        let recomputed = match pass::check(first.clone(), conversation, &window.request) {
            Ok(_) => RebuildStatus::Written,
            Err(_)
                if pass::compression_request(&first, conversation, &window.request, voice())
                    .is_some() =>
            {
                RebuildStatus::RetryHeader
            }
            Err(_) => RebuildStatus::RetryWindow,
        };
        if recomputed != status {
            return Err(RebuildError::Input(
                "previous_status is not the status of the first answer",
            ));
        }
        if status == RebuildStatus::RetryHeader
            && let Some(header) = header
        {
            answer.replace_header(header);
        }
    }
    let distilled = match pass::check(answer.clone(), conversation, &window.request) {
        Ok(distilled) => distilled,
        Err(_) if repair => pass::repair(answer, conversation, &window.request).map_err(|err| {
            let reason =
                correction.map_or_else(|| err.to_string(), |first| format!("{first}; retry: {err}"));
            RebuildError::Step(PassError::new(
                "session_answer_invalid",
                short_reason(&admitted.redactor, &reason),
            ))
        })?,
        Err(err) => {
            let refusal = short_reason(&admitted.redactor, &err.to_string());
            if let Some(mut request) =
                pass::compression_request(&answer, conversation, &window.request, voice())
            {
                let mut reply = RebuildApplied::new(RebuildStatus::RetryHeader, cursor);
                reply.correction = Some(refusal);
                request.voice.instructions = request
                    .voice
                    .instructions
                    .take()
                    .map(|text| request.voice.fill(&text));
                reply.header_request = Some(mask_value(json!(request), &admitted.redactor));
                return Ok(reply);
            }
            let mut reply = RebuildApplied::new(RebuildStatus::RetryWindow, cursor);
            reply.correction = Some(refusal.clone());
            let request = SessionRequest {
                correction: Some(refusal),
                ..window.request
            };
            reply.window_request = Some(masked_input(&request, &admitted.redactor));
            return Ok(reply);
        }
    };
    let dropped = distilled.dropped_terms;
    let warnings = window_warnings(cursor.windows + 1, &distilled);
    let now = ctx.clock.now();
    let window_meta: Vec<_> = data.conversation[window.offset..window.end]
        .iter()
        .map(|m| {
            (
                m.uuid.clone(),
                data.messages
                    .iter()
                    .find(|sent| sent.uuid == m.uuid)
                    .map_or(now, |sent| sent.at),
            )
        })
        .collect();
    let original = &admitted.original;
    let meta = pass::PassMeta {
        id: admitted.id.clone(),
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
        final_pass: window.final_window,
        window: window_meta,
        segment_windows: admitted.config.segment_windows,
        origin: SegmentOrigin::Rebuild,
    };
    let sealed_before = sealed(previous.as_ref());
    let mut record = pass::record(previous.as_ref(), distilled, meta);
    record.created = original.created;
    record.updated = basis.expected_updated;
    let Some(record) = ctx
        .store
        .checkpoint_rebuild(record, basis.checkpoint.as_ref(), &admitted.redactor)?
    else {
        return stale(cursor);
    };
    cursor.segments_sealed += sealed(Some(&record)).saturating_sub(sealed_before);
    cursor.windows += 1;
    cursor.dropped_terms += dropped;
    cursor.warnings.extend(warnings);
    Ok(RebuildApplied::new(RebuildStatus::Written, cursor))
}

fn sealed(record: Option<&SessionRecord>) -> usize {
    record.map_or(0, |r| {
        r.log
            .iter()
            .filter(|s| s.state == SegmentState::Sealed)
            .count()
    })
}

/// Promote a complete checkpoint, or rebase it onto a newer visible record.
///
/// # Errors
///
/// [`RebuildError`]: a refusal, the transcript, or the store.
pub(crate) fn finish<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: RebuildFinishInput,
) -> std::result::Result<RebuildFinished, RebuildError<E>> {
    let Some(progress) = input.progress.into_iter().next() else {
        return Err(RebuildError::Input("finish requires the rebuild cursor"));
    };
    let cursor = progress.cursor.checked()?;
    let basis = input.basis.checked(&cursor)?;
    let not_yet = |next_expected| RebuildFinished {
        done: false,
        finished: false,
        next_expected,
        pass: None,
    };
    if !basis.ready {
        return Ok(not_yet(None));
    }
    let admitted = admit(ctx, &basis.session_id, &basis.author, true)?;
    let data = admitted.read(ctx)?;
    let (mark, previous, offset) = admitted.checkpoint(ctx, &data)?;
    // Recomputed, not trusted: the checkpoint is the one prepare read and
    // holds every message the transcript has now.
    if mark != basis.checkpoint || offset < data.conversation.len() {
        return Ok(not_yet(None));
    }
    let (Some(mark), Some(mut finished)) = (mark, previous) else {
        return Err(RebuildError::TranscriptMissing(admitted.id.clone()));
    };
    let final_pass = admitted.final_pass(ctx)?;
    let sealed_at_finish = final_pass && finished.open_segment().is_some();
    if final_pass {
        finished = pass::finalized(&finished, ctx.clock.now());
    }
    finished.updated = ctx.clock.now();
    finished.rebuilt_by = Some(Author::agent(basis.author.clone()));
    match ctx
        .store
        .finish_rebuild(finished, &mark, &admitted.redactor)?
    {
        Finished::Written(written) => Ok(RebuildFinished {
            done: true,
            finished: true,
            next_expected: None,
            pass: Some(PassPayload {
                session: admitted.id.to_string(),
                result: "written",
                error_code: None,
                passes: Some(written.passes),
                dropped_terms: cursor.dropped_terms,
                windows: cursor.windows,
                segments_sealed: cursor.segments_sealed + usize::from(sealed_at_finish),
                segments: Some(written.log.len()),
                author: Some(Author::agent(basis.author.clone())),
                gave_up: false,
                error: None,
                warnings: cursor.warnings,
                memory_written: 0,
                memory_promoted: 0,
                memory_proposed: 0,
                memory_pending: 0,
                memory_refused: std::collections::BTreeMap::new(),
            }),
        }),
        Finished::Rebased(next) => Ok(not_yet(Some(next))),
        Finished::Stale => Ok(not_yet(None)),
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
            RebuildError::Input(message) => Self::Usage(message.to_owned()),
            RebuildError::Step(error) => Self::SessionPassStep {
                code: error.code,
                message: error.message,
            },
        }
    }
}
