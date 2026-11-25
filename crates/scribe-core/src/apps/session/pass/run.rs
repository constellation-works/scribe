//! Shared session window reads and writes for Orbit pass steps and the
//! interim catalog flow.

use crate::models::PassMessage;

mod workflow;

pub use workflow::{
    ActivityAnswer, ApplyInput, ApplyOutput, ApplyStatus, FailInput, PrepareInput, Prepared,
    SessionPassInput,
};
pub(crate) use workflow::{apply, fail, prepare, settle_only};

use crate::apps::tag::ConfigReads;
use crate::control;
use crate::models::SessionRequest;
use crate::runtime::audit::PersistedAudit;
use crate::runtime::resolve::refuse_forgotten;
use crate::runtime::{Ctx, PortError, TranscriptState};
use crate::{
    Config, Label, Models, Project, Provider, Redactor, SessionControl, SessionFilter, SessionId,
    SessionRecord, memory, pass, transcript,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

/// `session pass`'s audit command name.
pub const PASS_COMMAND: &str = "session pass";

/// `session catalog`'s audit command name.
pub const CATALOG_COMMAND: &str = "session catalog";

const MAX_LISTED: usize = 200;

/// Why a session pass did not run.
#[derive(Debug)]
pub enum PassRunError<E> {
    /// A store error.
    Core(crate::Error),
    /// A named session is not switched on.
    SessionNotOn(SessionId),
    /// A port refused (the redaction port's unset `[redact] env`).
    Port(E),
    /// Invalid continuation or answer input.
    Input(&'static str),
    /// A deterministic step failed.
    Step(PassError),
}

impl<E> From<crate::Error> for PassRunError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

/// Why a catalog's windows did not run: a store error or a port's refusal.
#[derive(Debug)]
pub(crate) enum RunFailure<E> {
    Core(crate::Error),
    Port(E),
}

/// One cataloged session's run, after `session catalog` began it.
pub(crate) struct CatalogJob<'a> {
    pub(crate) provider: Provider,
    pub(crate) host: &'a Label,
    pub(crate) id: &'a SessionId,
    pub(crate) switch: &'a SessionControl,
    pub(crate) project: Project,
    pub(crate) ended: bool,
    pub(crate) author: &'a str,
}

/// What a session pass did for one session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassPayload {
    /// The session id.
    pub session: String,
    /// `written` (a record was written), `unchanged` (nothing new to
    /// record), `waiting` (no transcript or messages yet; still due) or
    /// `failed` (the previous record is kept).
    pub result: &'static str,
    /// The stable code of a failure; `null` otherwise.
    pub error_code: Option<&'static str>,
    /// The record's pass count after this pass; `null` when there is no
    /// record.
    pub passes: Option<u32>,
    /// Terms the model offered that were not verbatim from the user's turns.
    pub dropped_terms: usize,
    /// Successfully persisted model windows for this session in this run.
    pub windows: usize,
    /// Segments newly sealed in this invocation.
    pub segments_sealed: usize,
    /// Segment count in the latest persisted record/checkpoint.
    pub segments: Option<usize>,
    /// The requesting agent for a rebuild; null for routine passes.
    pub author: Option<crate::Author>,
    /// Always `false`: a failed pass stays due and is never given up. Kept
    /// because the `--json` shape is a contract.
    pub gave_up: bool,
    /// A redacted, bounded explanation for a failure; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Deterministic repairs in successfully persisted windows, in order.
    pub warnings: Vec<String>,
    /// `now` and milestone files written in persisted windows (2_design.md
    /// §15.5).
    pub memory_written: usize,
    /// Always 0 since the fixed memory shape (2_design.md §15.5): the pass
    /// promotes nothing, and the weekly refresh writes rule (a)'s
    /// milestones. Kept because `--json` field names are a contract.
    pub memory_promoted: usize,
    /// Always 0: the `preferences` candidates once opened for recurring
    /// `working` items (2_design.md §13.7) are no longer opened, since the
    /// pass writes no `working` item (§15.9 phase 1). Kept because `--json`
    /// field names are a contract.
    pub memory_proposed: usize,
    /// Undated `now` candidates kept pending in persisted windows.
    pub memory_pending: usize,
    /// Memory elements refused, by code (`memory_sensitive`, ...).
    pub memory_refused: std::collections::BTreeMap<&'static str, usize>,
}

/// Report for CLI settlements, session catalog and session rebuild.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassReport {
    /// One per session passed, in id order.
    pub passes: Vec<PassPayload>,
    /// Due sessions left for the next run.
    pub remaining: usize,
    /// Lines for stderr, printed in every mode.
    #[serde(skip)]
    pub notices: Vec<String>,
    /// This report is `session catalog`. Not part of the JSON contract.
    #[serde(skip)]
    pub catalog: bool,
    /// The catalog switch was written this run: one extra success audit line.
    #[serde(skip)]
    pub switch_written: bool,
    /// Open candidate proposals closed during this pass because their tag is
    /// retired, and ideas the pass wrote itself: one success audit line each.
    #[serde(skip)]
    pub persisted_targets: Vec<String>,
    /// `memory:<id>` once per item created or occurrence added: one success
    /// audit line each, ids only.
    #[serde(skip)]
    pub memory_targets: Vec<String>,
}

/// Run one `session catalog` after its switch is written. No deadline and
/// no five-session cap. `project` is used only when the session has no
/// record yet. `ended` marks the last window final and turns the switch off.
pub(crate) fn run_catalog<E: PortError, M: Models>(
    ctx: &Ctx<'_, E>,
    models: impl FnOnce(&Config) -> M,
    caught: CatalogJob<'_>,
) -> Result<PassReport, RunFailure<E>> {
    let CatalogJob {
        provider,
        host,
        id,
        switch,
        project,
        ended,
        author,
    } = caught;
    let store = ctx.store;
    let config = store
        .read()
        .tags()
        .config()
        .map_err(RunFailure::Core)?
        .for_provider(provider);
    let redactor = ctx.redaction.redactor(&config).map_err(RunFailure::Port)?;
    let found = crate::apps::session::list_sessions(
        &store.read(),
        &SessionFilter {
            include_inactive: true,
            ..Default::default()
        },
        None,
    )
    .map_err(RunFailure::Core)?;
    let known = Known::new(&found.listing.items);
    let corrupt: Vec<(String, String)> = found
        .skipped
        .iter()
        .filter_map(|s| {
            Some((
                s.path.file_stem()?.to_str()?.to_owned(),
                s.error.to_string(),
            ))
        })
        .collect();
    let adapter = models(&config);
    let mut persisted =
        PersistedAudit::new(store, CATALOG_COMMAND, ctx.clock.now()).with_author(author);
    let mut job = Job {
        ctx,
        provider,
        host,
        config: &config,
        redactor: &redactor,
        id,
        switch,
        candidate_slots: pass::MAX_CANDIDATES,
        write_slots: pass::MAX_COLLECTED,
        memory: MemoryTally::default(),
        windows: 0,
        segments_sealed: 0,
        segments: None,
        dropped_terms: 0,
        warnings: Vec::new(),
        check_switch: false,
        catalog: Some(CatalogRun { project, ended }),
        persisted_targets: Vec::new(),
    };
    let result = if let Some((_, reason)) = corrupt.iter().find(|(stem, _)| stem == id.as_str()) {
        Err(PassError::new("store_corrupt", reason.clone()))
    } else {
        let previous = found.listing.items.iter().find(|r| &r.id == id);
        job.run(previous.cloned(), &known, &adapter)
    };
    // Record any persisted closures before `job.failed` can return `Err` and
    // bypass `Output::Pass`.
    persisted.extend(job.persisted_targets.iter().cloned());
    persisted.extend(job.memory.targets.iter().cloned());
    let passes = vec![match result {
        Ok(done) => done,
        Err(failure) => job.failed(failure).map_err(RunFailure::Core)?,
    }];
    persisted.disarm();
    Ok(PassReport {
        passes,
        remaining: 0,
        notices: Vec::new(),
        catalog: true,
        switch_written: false,
        persisted_targets: job.persisted_targets,
        memory_targets: job.memory.targets,
    })
}

/// The store's topics and terms, offered to the model for reuse, and its
/// records, for a linked session's context.
struct Known<'a> {
    topics: Vec<String>,
    terms: Vec<String>,
    records: &'a [SessionRecord],
}

impl<'a> Known<'a> {
    fn new(records: &'a [SessionRecord]) -> Self {
        let mut topics: BTreeSet<String> = BTreeSet::new();
        let mut terms: BTreeSet<String> = BTreeSet::new();
        for record in records {
            topics.extend(record.topics.as_slice().iter().map(ToString::to_string));
            terms.extend(record.terms.iter().map(|t| t.term.clone()));
        }
        Self {
            topics: topics.into_iter().take(MAX_LISTED).collect(),
            terms: terms.into_iter().take(MAX_LISTED).collect(),
            records,
        }
    }
}

struct Conversation {
    messages: Vec<transcript::Message>,
    conversation: Vec<PassMessage>,
    cwd: String,
}

struct Window {
    request: SessionRequest,
    project: Project,
    offset: usize,
    end: usize,
    last: bool,
    through: String,
}

/// One session's pass.
struct Job<'a, E> {
    ctx: &'a Ctx<'a, E>,
    /// The session's provider, whose transcripts it reads.
    provider: Provider,
    /// This host, as records name it.
    host: &'a Label,
    config: &'a Config,
    redactor: &'a Redactor,
    id: &'a SessionId,
    switch: &'a SessionControl,
    candidate_slots: usize,
    /// Ideas the pass may still write itself for this session in this
    /// invocation (`write` tags, 2_design.md §11.6).
    write_slots: usize,
    /// Memory written and refused across this invocation's windows.
    memory: MemoryTally,
    windows: usize,
    segments_sealed: usize,
    segments: Option<usize>,
    dropped_terms: usize,
    warnings: Vec<String>,
    check_switch: bool,
    /// Set for `session catalog`: the project of the first record, and
    /// whether the transcript's last message is at least a day old.
    catalog: Option<CatalogRun>,
    /// Open candidate proposals closed because their tag is no longer in the
    /// tag table, and entries the pass wrote itself under `write` tags, for
    /// audit logging.
    persisted_targets: Vec<String>,
}

/// One session's memory across its windows (2_design.md §15.5): the slots
/// left (refused elements use theirs), what was written and refused, and
/// the audit targets.
struct MemoryTally {
    slots: usize,
    written: usize,
    pending: usize,
    refused: std::collections::BTreeMap<&'static str, usize>,
    targets: Vec<String>,
}

impl Default for MemoryTally {
    fn default() -> Self {
        Self {
            slots: memory::MAX_PER_PASS,
            written: 0,
            pending: 0,
            refused: std::collections::BTreeMap::new(),
            targets: Vec::new(),
        }
    }
}

impl MemoryTally {
    fn refuse(&mut self, refusals: impl IntoIterator<Item = memory::Refusal>) {
        for refusal in refusals {
            *self.refused.entry(refusal.code()).or_default() += 1;
        }
    }
}

/// One cataloged session's extra facts. Absent for an ordinary pass.
struct CatalogRun {
    project: Project,
    ended: bool,
}

/// A session pass failure, with the stable code kept separate from its
/// redacted, bounded diagnostic.
#[derive(Debug)]
pub struct PassError {
    pub(crate) code: &'static str,
    write_error: bool,
    pub(crate) message: String,
}

impl PassError {
    fn write(err: crate::Error) -> Self {
        Self {
            write_error: true,
            ..Self::new(err.code(), err.to_string())
        }
    }

    fn new(code: &'static str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            code,
            write_error: false,
            message: if message.trim().is_empty() {
                code.to_owned()
            } else {
                message
            },
        }
    }
}

/// The maximum characters of a reason exposed in a pass report.
const MAX_ERROR_CHARS: usize = 300;

/// Redact, flatten and bound a failure reason before it reaches a report or
/// the aggregate command error.
pub fn short_reason(redactor: &Redactor, message: &str) -> String {
    let flattened = redactor
        .mask(message)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = flattened.chars();
    let mut bounded: String = chars.by_ref().take(MAX_ERROR_CHARS).collect();
    if chars.next().is_some() {
        bounded = bounded.chars().take(MAX_ERROR_CHARS - 1).collect();
        bounded.push('…');
    }
    bounded
}

impl<E: PortError> Job<'_, E> {
    fn run<M: Models>(
        &mut self,
        mut previous: Option<SessionRecord>,
        known: &Known<'_>,
        adapter: &M,
    ) -> Result<PassPayload, PassError> {
        self.segments = previous.as_ref().map(|p| p.log.len());
        let Some(data) = self.read_transcript()? else {
            return self.nothing_new(previous.as_ref(), true);
        };
        loop {
            let Some(window) = self.next_window(previous.as_ref(), known, &data, None)? else {
                return self.nothing_new(previous.as_ref(), false);
            };
            let distilled = distill(
                self.id,
                window.request.clone(),
                &data.conversation[..window.end],
                adapter,
                self.config.voice(crate::voice::ModelUse::SessionHeader),
            )?;
            previous = self.commit(previous.as_ref(), &data, &window, distilled)?;
            if window.last {
                return Ok(self.payload("written", previous.as_ref()));
            }
        }
    }

    fn read_transcript(&mut self) -> Result<Option<Conversation>, PassError> {
        let Some(path) = self
            .ctx
            .transcripts
            .files(self.provider)
            .map_err(|e| PassError::new(e.code(), e.to_string()))?
            .into_iter()
            .find(|(found, _)| found == self.id)
            .map(|(_, path)| path)
        else {
            return Ok(None);
        };
        match self.ctx.transcripts.check(self.provider, &path, self.id) {
            Ok(TranscriptState::Present) => {}
            Ok(TranscriptState::Missing) => return Ok(None),
            Err(reason) => {
                tracing::warn!(session = %self.id, %reason, "not passing: the transcript check failed");
                return Err(PassError::new("provenance_failed", reason));
            }
        }
        let bytes = self.ctx.transcripts.read(self.provider, &path).map_err(|err| {
            tracing::warn!(session = %self.id, path = %path.display(), %err, "cannot read the transcript");
            PassError::new("io_error", format!("cannot read transcript: {err}"))
        })?;
        // A catalog switch that `session catalog` did not run itself (one
        // `/scribe catalog <id>` wrote, or a failed catalog left due) is
        // passed as a catalog: its windows are `origin: catalog`, it keeps
        // `cataloged_by`, and an ended transcript ends final and off.
        if self.catalog.is_none() && self.switch.cataloged_by.is_some() {
            self.catalog = Some(CatalogRun {
                project: attributed_project(&bytes, self.config, |cwd| {
                    self.ctx.transcripts.project_of(cwd)
                }),
                ended: transcript::survey(&bytes)
                    .ended
                    .is_some_and(|last| transcript::is_day_old(last, self.ctx.clock.now())),
            });
        }
        let messages = transcript::parse(&bytes);
        let conversation = pass::conversation(&messages);
        Ok((!conversation.is_empty()).then(|| Conversation {
            messages,
            conversation,
            cwd: transcript::cwd(&bytes).unwrap_or_default(),
        }))
    }

    fn is_final(&self) -> bool {
        self.catalog
            .as_ref()
            .map_or(self.switch.final_pass, |catalog| catalog.ended)
    }

    fn next_window(
        &self,
        previous: Option<&SessionRecord>,
        known: &Known<'_>,
        data: &Conversation,
        // Replaying a prepared window uses its original endpoint/final flag
        // even when the host transcript has grown in the meantime.
        prepared_end: Option<(&str, bool)>,
    ) -> Result<Option<Window>, PassError> {
        let conversation = &data.conversation;
        let offset = pass::start_after(
            conversation,
            previous.and_then(|p| p.scribed_through.as_deref()),
        );
        let limit = match prepared_end {
            Some((uuid, _)) => conversation[offset..]
                .iter()
                .position(|m| m.uuid == uuid)
                .map(|n| offset + n + 1)
                .ok_or_else(|| {
                    PassError::new("record_changed", "prepared transcript window disappeared")
                })?,
            None => conversation.len(),
        };
        let window = pass::window_with_limits(
            &conversation[offset..limit],
            None,
            self.config.catalog_window,
        );
        let Some(through) = window.messages.last().map(|m| m.uuid.clone()) else {
            return Ok(None);
        };
        let end = offset + window.messages.len();
        let last = end == conversation.len();
        let final_pass = self.is_final() && prepared_end.map_or(last, |(_, was_last)| was_last);
        // The linked sessions' latest records, read once: context only.
        let mut links = previous.map(|p| p.links.clone()).unwrap_or_default();
        for link in &self.switch.pending_links {
            if !links.contains(link) {
                links.push(link.clone());
            }
        }
        let linked = pass::linked_context(&links, known.records);
        let mut topics: BTreeSet<_> = known.topics.iter().cloned().collect();
        let mut terms: BTreeSet<_> = known.terms.iter().cloned().collect();
        if let Some(record) = previous {
            topics.extend(record.topics.as_slice().iter().map(ToString::to_string));
            terms.extend(record.terms.iter().map(|t| t.term.clone()));
        }
        let mut request = pass::request(
            &self.config.models.session,
            previous,
            linked,
            window,
            topics.into_iter().take(MAX_LISTED).collect(),
            terms.into_iter().take(MAX_LISTED).collect(),
            final_pass,
            self.config.voice(crate::voice::ModelUse::Session),
        );
        // The trigger gate (2_design.md §11.2): no Daniel turn in the
        // window fires a capture tag's trigger, no claim is requested.
        let (turns, rules) = pass::capture_request(&self.config.tags, &request.messages);
        request.candidate_limit = if turns.is_empty() {
            0
        } else {
            self.candidate_slots
        };
        request.write_limit = if turns.is_empty() {
            0
        } else {
            self.write_slots
        };
        request.candidate_turns = turns;
        request.candidate_rules = rules;
        let mut project = if data.cwd.is_empty() {
            Project::sanitized("")
        } else {
            self.ctx.transcripts.project_of(Path::new(&data.cwd))
        };
        // `--project`, or the transcript-list attribution, only for the
        // first record. A resumed catalog keeps the record's project, as
        // does every later window (the write exclusions need it now).
        if let Some(record) = previous {
            project = record.project.clone();
        } else if let Some(catalog) = &self.catalog {
            project = catalog.project.clone();
        }
        let entries = self
            .ctx
            .store
            .read()
            .entries()
            .list(&Default::default())
            .map_err(|err| PassError::new(err.code(), err.to_string()))?;
        let proposals = self
            .ctx
            .store
            .read()
            .proposals()
            .all()
            .map_err(|err| PassError::new(err.code(), err.to_string()))?;
        // Retired-tag open candidates will be closed under the lock before
        // the cap is counted, so count only open candidates whose tag is
        // still in the tag table here.
        if proposals
            .iter()
            .filter(|p| {
                p.kind == crate::ProposalKind::Candidate
                    && p.state == crate::ProposalState::Open
                    && p.candidate
                        .as_ref()
                        .is_some_and(|c| self.config.tags.get(&c.tag).is_some())
            })
            .count()
            >= pass::MAX_OPEN_CANDIDATES
        {
            request.candidate_limit = 0;
        }
        request.candidate_exclusions =
            pass::candidate_exclusions(&entries.listing.items, &proposals, &request);
        request.write_exclusions =
            pass::write_exclusions(&entries.listing.items, &project, &request);
        // Memory (2_design.md §15.5): only from a session switched on,
        // as every pass's is, within the invocation's slots; the signal
        // (§15.6) is asked of every window.
        let (memory_now, memory_pending) = self
            .ctx
            .store
            .read()
            .memory()
            .context(self.id)
            .map_err(|err| PassError::new(err.code(), err.to_string()))?;
        request.memory = true;
        request.memory_limit = self.memory.slots;
        request.memory_now = memory_now;
        request.memory_pending = memory_pending;
        Ok(Some(Window {
            request,
            project,
            offset,
            end,
            last,
            through,
        }))
    }

    fn commit(
        &mut self,
        previous: Option<&SessionRecord>,
        data: &Conversation,
        window: &Window,
        distilled: pass::Distilled,
    ) -> Result<Option<SessionRecord>, PassError> {
        let Window {
            offset, end, last, ..
        } = *window;
        let messages = data.messages.as_slice();
        let conversation = data.conversation.as_slice();
        let through = window.through.clone();
        let first_message = window.request.messages.first().map(|m| m.uuid.clone());
        let expected = previous.map(|p| p.updated);
        let memory_limit = window.request.memory_limit;
        let now = self.ctx.clock.now();
        let window_meta = window
            .request
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
        let masks = |text: &str| self.redactor.mask(text) != text;
        let checked = memory::check(
            &distilled.memory,
            &conversation[offset..end],
            memory_limit,
            &masks,
        );
        self.memory.slots = self.memory.slots.saturating_sub(checked.used);
        let (memory_drafts, unverified) = memory::drafts(checked.checked, messages);
        // Candidate slots are spent by what the model returned; write
        // slots only by what the store wrote, below, since a drafted idea
        // may still be dropped (a task outcome, a duplicate) and must not
        // shrink a later window's limit.
        let write_answers = distilled
            .candidates
            .iter()
            .filter(|c| {
                c.tag
                    .parse()
                    .ok()
                    .and_then(|tag| self.config.tags.get(&tag))
                    .is_some_and(|tag| tag.capture.writes())
            })
            .count();
        self.candidate_slots = self
            .candidate_slots
            .saturating_sub(distilled.candidates.len() - write_answers);
        let candidate_answers = distilled.candidates.clone();
        let signal = distilled.signal;
        let dropped = distilled.dropped_terms;
        let warnings = window_warnings(self.windows + 1, &distilled);
        // The window's first and last messages' times bound the segment.
        let sent = |uuid: Option<&str>| {
            uuid.and_then(|u| messages.iter().find(|m| m.uuid == u))
                .map_or(now, |m| m.at)
        };
        let from = sent(first_message.as_deref());
        let to = sent(Some(&through));
        let meta = pass::PassMeta {
            id: self.id.clone(),
            project: window.project.clone(),
            provider: Label::sanitized(self.provider.as_str(), self.provider.as_str()),
            host: self.host.clone(),
            cwd: data.cwd.clone(),
            started: messages.first().map_or(now, |m| m.at),
            on_since: self.switch.since.unwrap_or(now),
            through: Some(through),
            first_message,
            from,
            to,
            now,
            final_pass: self.is_final() && last,
            window: window_meta,
            segment_windows: self.config.segment_windows,
            origin: if self.catalog.is_some() {
                crate::SegmentOrigin::Catalog
            } else {
                crate::SegmentOrigin::Pass
            },
        };
        let sealed_before = previous.map_or(0, |p| {
            p.log
                .iter()
                .filter(|s| s.state == crate::SegmentState::Sealed)
                .count()
        });
        let record = self.stamp_catalog(pass::record(previous, distilled, meta));
        let sealed_after = record
            .log
            .iter()
            .filter(|s| s.state == crate::SegmentState::Sealed)
            .count();
        let candidates =
            pass::candidate_drafts(&candidate_answers, messages, &record, &self.config.tags);
        let mode = if !last {
            super::PassWrite::Checkpoint
        } else if self.catalog.as_ref().is_some_and(|c| c.ended) {
            super::PassWrite::CatalogFinal
        } else if self.catalog.is_some() {
            super::PassWrite::CatalogLive
        } else {
            super::PassWrite::Settle(self.switch.final_pass)
        };
        let pass_memory = crate::apps::memory::WindowMemory {
            drafts: memory_drafts,
            signal: signal
                .zip(record.scribed_through.clone())
                .map(|(s, t)| (t, s)),
            final_pass: record.status == crate::SessionStatus::Final,
        };
        // The window's messages commit in the transaction that
        // advances the record's cursor (2_design.md §15.7, phase 6).
        let kept = crate::message::window(
            &record.id,
            &conversation[offset..end],
            messages,
            &candidates,
        );
        let written = match super::write_window(
            self.ctx.store,
            record,
            expected,
            mode,
            &candidates,
            Some(&pass_memory),
            &kept,
            self.redactor,
            self.check_switch.then_some(self.switch),
        ) {
            Ok(written) => written,
            // The window's closures and entries commit with it, so a
            // failure leaves none of them to audit.
            Err(err) => return Err(PassError::write(err)),
        };
        self.write_slots = self
            .write_slots
            .saturating_sub(written.written_entries.len());
        let record = written.record;
        self.memory.refuse(checked.refused);
        self.memory.refuse(unverified);
        self.memory.refuse(written.memory.refused);
        self.memory.written += written.memory.written.len();
        self.memory.pending += written.memory.pending;
        for target in written.memory.written {
            if !self.memory.targets.contains(&target) {
                self.memory.targets.push(target);
            }
        }
        self.persisted_targets.extend(
            written
                .closed_candidates
                .into_iter()
                .map(|p| p.id.to_string())
                .chain(written.written_entries.iter().map(ToString::to_string)),
        );
        self.segments_sealed += sealed_after.saturating_sub(sealed_before);
        self.segments = record.as_ref().map(|r| r.log.len());
        self.windows = self.windows.saturating_add(1);
        self.dropped_terms = self.dropped_terms.saturating_add(dropped);
        self.warnings.extend(warnings);
        Ok(record)
    }

    /// Nothing to send the model. At session end the record (if any) is
    /// marked final and the pass settled. Otherwise, when `wait` (no
    /// transcript or message yet) the pass stays due for the next run;
    /// else (nothing new since the last pass) it is settled unchanged.
    fn nothing_new(
        &mut self,
        previous: Option<&SessionRecord>,
        wait: bool,
    ) -> Result<PassPayload, PassError> {
        if self.catalog.is_some() {
            let ended = self.catalog.as_ref().is_some_and(|c| c.ended);
            // An ended transcript with nothing left to send is still final,
            // and the switch turns off with that write. A live one, or a
            // transcript with no message yet, stays on and due.
            if ended && !wait {
                let record = previous.map(|p| {
                    let mut finalized = pass::finalized(p, self.ctx.clock.now());
                    if finalized.cataloged_by.is_none() {
                        finalized.cataloged_by.clone_from(&self.switch.cataloged_by);
                    }
                    finalized
                });
                let newly_sealed = previous.is_some_and(|p| p.open_segment().is_some());
                let written = self.settle_off(record, previous.map(|p| p.updated))?;
                self.segments_sealed += usize::from(newly_sealed);
                let result = if written.is_some() {
                    "written"
                } else {
                    "unchanged"
                };
                return Ok(self.payload(result, written.as_ref()));
            }
            let result = if wait { "waiting" } else { "unchanged" };
            return Ok(self.payload(result, previous));
        }
        if self.switch.final_pass {
            let record = previous.map(|p| pass::finalized(p, self.ctx.clock.now()));
            let newly_sealed = previous.is_some_and(|p| p.open_segment().is_some());
            let written = self.settle(record, previous.map(|p| p.updated))?;
            self.segments_sealed += usize::from(newly_sealed);
            let result = if written.is_some() {
                "written"
            } else {
                "unchanged"
            };
            return Ok(self.payload(result, written.as_ref()));
        }
        if wait {
            return Ok(self.payload("waiting", previous));
        }
        self.settle(None, previous.map(|p| p.updated))?;
        Ok(self.payload("unchanged", previous))
    }

    /// Copy `cataloged_by` from the switch onto the first record. Later
    /// windows keep it through [`pass::record`].
    fn stamp_catalog(&self, mut record: SessionRecord) -> SessionRecord {
        if self.catalog.is_some() && record.cataloged_by.is_none() {
            record.cataloged_by.clone_from(&self.switch.cataloged_by);
        }
        record
    }

    /// An ended catalog's last window: final record and the switch off,
    /// in one store call.
    fn settle_off(
        &self,
        record: Option<SessionRecord>,
        expected: Option<time::OffsetDateTime>,
    ) -> Result<Option<SessionRecord>, PassError> {
        super::write_pass(
            self.ctx.store,
            self.id,
            record,
            super::PassWrite::CatalogFinal,
            expected,
            self.redactor,
            self.check_switch.then_some(self.switch),
        )
        .map_err(|err| {
            tracing::warn!(session = %self.id, %err, "cannot finish the catalog");
            PassError::write(err)
        })
    }

    /// Write `record` (if any) and settle the switch, in one store call.
    /// `expected` is the loaded record's `updated` revision, checked by the
    /// store under its lock; a refusal keeps the session due through
    /// [`Job::failed`].
    fn settle(
        &self,
        record: Option<SessionRecord>,
        expected: Option<time::OffsetDateTime>,
    ) -> Result<Option<SessionRecord>, PassError> {
        super::write_pass(
            self.ctx.store,
            self.id,
            record,
            super::PassWrite::Settle(self.switch.final_pass),
            expected,
            self.redactor,
            self.check_switch.then_some(self.switch),
        )
        .map_err(|err| {
            tracing::warn!(session = %self.id, %err, "cannot write the session pass");
            PassError::write(err)
        })
    }

    /// The previous record is kept and the pass stays due, so the next run
    /// retries it.
    fn failed(&self, failure: PassError) -> Result<PassPayload, crate::Error> {
        self.ctx.store.write(|tx| {
            tx.sessions()
                .change_control(|state| control::fail(state, self.id, self.ctx.clock.now()))
        })?;
        Ok(self.failure_payload(failure))
    }

    fn failure_payload(&self, failure: PassError) -> PassPayload {
        PassPayload {
            session: self.id.to_string(),
            result: "failed",
            error_code: Some(failure.code),
            passes: None,
            dropped_terms: self.dropped_terms,
            windows: self.windows,
            segments_sealed: self.segments_sealed,
            segments: self.segments,
            author: None,
            gave_up: false,
            error: Some(short_reason(self.redactor, &failure.message)),
            warnings: self.warnings.clone(),
            memory_written: self.memory.written,
            memory_promoted: 0,
            memory_proposed: 0,
            memory_pending: self.memory.pending,
            memory_refused: self.memory.refused.clone(),
        }
    }

    fn payload(&self, result: &'static str, record: Option<&SessionRecord>) -> PassPayload {
        PassPayload {
            session: self.id.to_string(),
            result,
            error_code: None,
            passes: record.map(|r| r.passes),
            dropped_terms: self.dropped_terms,
            windows: self.windows,
            segments_sealed: self.segments_sealed,
            segments: record.map(|r| r.log.len()),
            author: None,
            gave_up: false,
            error: None,
            warnings: self.warnings.clone(),
            memory_written: self.memory.written,
            memory_promoted: 0,
            memory_proposed: 0,
            memory_pending: self.memory.pending,
            memory_refused: self.memory.refused.clone(),
        }
    }
}

/// The session's transcript: `<dir>/<project-dir>/<id>.jsonl`, the first
/// project directory (by name) that has one. Claude Code names the project
/// directory after the session's cwd, which the store does not keep.
pub(crate) fn distill<M: Models>(
    id: &SessionId,
    request: SessionRequest,
    conversation: &[crate::models::PassMessage],
    adapter: &M,
    header_voice: crate::voice::Voice,
) -> Result<pass::Distilled, PassError> {
    let mut answer = adapter.session(&request).map_err(|err| {
        tracing::warn!(session = %id, %err, "the session model failed");
        PassError::new(err.code(), err.to_string())
    })?;
    // One more ask in the same pass, naming what was refused: a model
    // can't count characters exactly, so a near miss is common. A refusal
    // the retry or the repair resolves is logged at info; only a pass that
    // still fails warns, naming both refusals. Repairs reach the report.
    let distilled = match pass::check(answer.clone(), conversation, &request) {
        Ok(distilled) => distilled,
        Err(err) => {
            tracing::info!(session = %id, %err, "the session model's answer was refused; asking once more");
            let refusal = err.to_string();
            let retry_failure = |err: crate::models::ModelError| {
                tracing::warn!(session = %id, %refusal, %err, "the session model failed on the retry");
                PassError::new(err.code(), format!("{refusal}; retry: {err}"))
            };
            if let Some(compress) =
                pass::compression_request(&answer, conversation, &request, header_voice)
            {
                let header = adapter.session_header(&compress).map_err(retry_failure)?;
                answer.replace_header(header);
            } else {
                let retry = SessionRequest {
                    correction: Some(refusal.clone()),
                    ..request.clone()
                };
                answer = adapter.session(&retry).map_err(retry_failure)?;
            }
            pass::check(answer.clone(), conversation, &request)
                .or_else(|_| pass::repair(answer, conversation, &request))
                .map_err(|err| {
                    tracing::warn!(session = %id, %refusal, %err, "the session model's answer was refused twice");
                    PassError::new("session_answer_invalid", format!("{refusal}; retry: {err}"))
                })?
        }
    };
    Ok(distilled)
}

/// Repairs describe only fixed rule names and numbers; no model text or ids.
pub(crate) fn window_warnings(window: usize, answer: &pass::Distilled) -> Vec<String> {
    answer
        .repairs
        .iter()
        .map(|repair| format!("window {window}: {repair}"))
        .collect()
}

/// The project a transcript is attributed to: the one its configured
/// project names are mentioned most for, else its cwd's.
#[must_use]
pub fn attributed_project(
    bytes: &[u8],
    config: &Config,
    project_of: impl Fn(&Path) -> Project,
) -> Project {
    let cwd = transcript::cwd(bytes).unwrap_or_default();
    let cwd_project = if cwd.is_empty() {
        Project::sanitized("")
    } else {
        project_of(Path::new(&cwd))
    };
    transcript::attribute(
        &transcript::mention_counts(bytes, &config.projects),
        &cwd_project,
    )
}

/// `session pass`'s errors, each the variant it was before the flow moved to
/// the session app (ORB-27707 step 3).
impl From<PassRunError<crate::runtime::errors::CommandError>>
    for crate::runtime::errors::CommandError
{
    fn from(err: PassRunError<crate::runtime::errors::CommandError>) -> Self {
        match err {
            PassRunError::Core(e) => Self::Core(e),
            PassRunError::SessionNotOn(id) => Self::SessionNotOn(id),
            PassRunError::Port(e) => e,
            PassRunError::Input(message) => Self::Usage(message.to_owned()),
            PassRunError::Step(error) => Self::SessionPassStep {
                code: error.code,
                message: error.message,
            },
        }
    }
}
