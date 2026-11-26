//! The deterministic sides of the session-pass job.

use super::{
    Config, Conversation, Ctx, Job, Known, Label, MemoryTally, PassError, PassPayload, PassReport,
    PassRunError, PortError, Redactor, SessionControl, SessionId, Window, control, memory, pass,
    refuse_forgotten,
};
use crate::apps::tag::ConfigReads;
use crate::models::{SessionAnswer, SessionHeader, SessionWindowInput};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;

/// Select due sessions, or explicitly name switched-on sessions.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SessionPassInput {
    /// Empty selects due sessions. Named sessions bypass due and backoff.
    pub sessions: Vec<SessionId>,
    /// Resolve named session prefixes.
    pub resolve: bool,
    /// Maximum distinct sessions to admit.
    pub max_sessions: Option<usize>,
    /// Stop starting windows at this Unix time.
    pub deadline_epoch_s: Option<i64>,
    /// The `scribe-session-catalog` job's mode: one named session with a
    /// catalog switch, no start cutoff, audited as `session catalog`.
    pub catalog: Option<CatalogSelection>,
}

/// What the `scribe-session-catalog` job passes to each prepare, from its
/// `begin` step.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogSelection {
    /// The agent cataloging it, named on the audit lines.
    pub author: crate::AuthorName,
    /// The project of the first record (`--project`); else attributed.
    #[serde(default)]
    pub project: Option<crate::Project>,
}

/// Prepare one window or settle one session without a model.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrepareInput {
    /// Session selection, used on the first iteration.
    #[serde(flatten)]
    pub selection: SessionPassInput,
    /// The previous iteration's cursor (a single fan-in result).
    pub progress: Vec<Progress>,
}

/// The continuation field of an apply result.
#[derive(Debug, Deserialize)]
pub struct Progress {
    cursor: Cursor,
}

/// How long after a job's first prepare a later session or window may start:
/// a window may take two 120-second activity attempts and the job must end
/// inside the routine's timeout. The first window of a run always starts.
pub const PASS_START_BY_SECONDS: i64 = 50;

/// Job-local continuation and remaining session budget. Prepare and apply
/// return it sealed and refuse one whose fields no longer match the seal.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Cursor {
    sessions: Vec<SessionId>,
    next: usize,
    tally: Tally,
    remaining: usize,
    /// No session or window but the run's first starts at or after this
    /// Unix time.
    #[serde(default)]
    start_by: Option<i64>,
    /// The catalog job's selection, bound for the whole run: a continuation
    /// carries the one `select` validated, never one the caller supplies.
    #[serde(default)]
    catalog: Option<CatalogSelection>,
    #[serde(default)]
    seal: String,
}

/// The sealed fields of a [`Cursor`].
#[derive(Serialize)]
struct CursorFields<'a> {
    sessions: &'a [SessionId],
    next: usize,
    tally: &'a Tally,
    remaining: usize,
    start_by: Option<i64>,
    // Omitted when absent, so a normal pass's seal is the one it always had.
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog: &'a Option<CatalogSelection>,
}

impl Serialize for Cursor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
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

impl Cursor {
    fn advance(&mut self) {
        self.next += 1;
        self.tally = Tally::default();
    }

    fn pending(&self) -> usize {
        self.remaining
            .saturating_add(self.sessions.len().saturating_sub(self.next))
    }

    fn fields(&self) -> CursorFields<'_> {
        CursorFields {
            sessions: &self.sessions,
            next: self.next,
            tally: &self.tally,
            remaining: self.remaining,
            start_by: self.start_by,
            catalog: &self.catalog,
        }
    }

    fn seal_of(&self) -> String {
        seal("cursor", &json!(self.fields()))
    }

    /// Refuse a cursor that is not exactly one prepare or apply returned.
    fn checked<E>(self) -> Result<Self, PassRunError<E>> {
        if self.seal == self.seal_of() {
            Ok(self)
        } else {
            Err(PassRunError::Input(
                "the pass cursor was changed since scribe returned it",
            ))
        }
    }
}

/// Caps across windows of the same session.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct Tally {
    candidates: usize,
    writes: usize,
    memory: usize,
    windows: usize,
}

impl Default for Tally {
    fn default() -> Self {
        Self {
            candidates: pass::MAX_CANDIDATES,
            writes: pass::MAX_COLLECTED,
            memory: memory::MAX_PER_PASS,
            windows: 0,
        }
    }
}

impl Tally {
    fn after<E>(job: &Job<'_, E>) -> Self {
        Self {
            candidates: job.candidate_slots,
            writes: job.write_slots,
            memory: job.memory.slots,
            windows: job.windows,
        }
    }
}

/// Optimistic basis from prepare. Carries no transcript text. `digest`
/// binds the transcript window; `seal` binds every other field and the
/// cursor the window was prepared with, so apply and fail refuse a changed
/// value.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Basis {
    session_id: SessionId,
    #[serde(with = "time::serde::rfc3339::option")]
    expected_updated: Option<OffsetDateTime>,
    expected_through: Option<String>,
    switch: SessionControl,
    window_end_uuid: String,
    last_window: bool,
    digest: String,
    /// The catalog job's selection, so apply and fail catalog with it.
    #[serde(default)]
    catalog: Option<CatalogSelection>,
    #[serde(default)]
    cursor_seal: String,
    #[serde(default)]
    seal: String,
}

impl Basis {
    /// The catalog job's selection this window was prepared under.
    #[must_use]
    pub fn catalog(&self) -> Option<&CatalogSelection> {
        self.catalog.as_ref()
    }

    /// The session this window was prepared for.
    #[must_use]
    pub fn session(&self) -> &SessionId {
        &self.session_id
    }

    fn seal_of(&self) -> String {
        let updated = self.expected_updated.and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        });
        seal(
            "basis",
            &json!([
                self.session_id,
                updated,
                self.expected_through,
                self.switch,
                self.window_end_uuid,
                self.last_window,
                self.digest,
                self.catalog,
                self.cursor_seal,
            ]),
        )
    }

    fn sealed(mut self) -> Self {
        self.seal = self.seal_of();
        self
    }

    /// Refuse a basis that is not exactly one prepare returned.
    fn checked<E>(self) -> Result<Self, PassRunError<E>> {
        if self.seal == self.seal_of() {
            Ok(self)
        } else {
            Err(PassRunError::Input(
                "the prepared basis was changed since scribe returned it",
            ))
        }
    }
}

/// A selected window, model-free settlement, or end of the run.
#[derive(Debug, Serialize)]
pub struct Prepared {
    /// End the job's loop.
    pub done: bool,
    /// Exactly one window for distillation when true.
    pub needs_model: bool,
    /// Passes settled or left waiting without a model.
    pub settled: Vec<PassPayload>,
    /// Remaining work and slot counts.
    pub cursor: Cursor,
    /// Required for apply or failure backoff.
    pub basis: Option<Basis>,
    /// Bounded, redacted input for the distill activity.
    pub window_request: Option<Value>,
}

/// Apply a first answer, retry one refusal, or carry a model-free cursor.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    /// Basis from prepare. Null for a model-free step.
    pub basis: Option<Basis>,
    /// Cursor from prepare.
    pub cursor: Cursor,
    /// First distillation result.
    #[serde(default)]
    pub answer: Option<ActivityAnswer<SessionAnswer>>,
    /// Header compression result.
    #[serde(default)]
    pub header: Option<ActivityAnswer<SessionHeader>>,
    /// Whole-window retry result.
    #[serde(default)]
    pub retry_answer: Option<ActivityAnswer<SessionAnswer>>,
    /// Only the final attempt may use deterministic repair.
    #[serde(default)]
    pub repair: bool,
    /// The first apply's status, when this is the final attempt.
    pub previous_status: Option<ApplyStatus>,
    /// The first refusal, included in the final diagnostic.
    pub correction: Option<String>,
}

/// A CLI answer or the position-aligned results of an Orbit activity.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ActivityAnswer<T> {
    /// A supplied answer.
    One(T),
    /// The result of the first worker, followed by a skipped worker.
    FanIn(Vec<Option<T>>),
}

impl<T> ActivityAnswer<T> {
    pub(crate) fn first(self) -> Option<T> {
        match self {
            Self::One(answer) => Some(answer),
            Self::FanIn(results) => results.into_iter().next().flatten(),
        }
    }
}

/// Apply status, cursor and at most one targeted retry request.
#[derive(Debug, Serialize)]
pub struct ApplyOutput {
    /// Outcome of the check.
    pub status: ApplyStatus,
    /// Continuation used by the next prepare.
    pub cursor: Cursor,
    /// The checked window or failure report.
    pub pass: Option<PassPayload>,
    /// Stable failure code, or null.
    pub error_code: Option<&'static str>,
    /// Stable refusal detail for the retry.
    pub correction: Option<String>,
    /// Header compression request on a header-only refusal.
    pub header_request: Option<Value>,
    /// Corrected request on a full-window refusal.
    pub window_request: Option<Value>,
    #[serde(skip)]
    pub(crate) persisted_targets: Vec<String>,
    #[serde(skip)]
    pub(crate) memory_targets: Vec<String>,
}

/// Result of checking one window.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    /// Model-free step.
    Idle,
    /// The window committed.
    Written,
    /// The pass failed or the optimistic basis expired.
    Failed,
    /// Compress only the header.
    RetryHeader,
    /// Retry the full window with a correction.
    RetryWindow,
}

impl ApplyOutput {
    fn new(status: ApplyStatus, cursor: Cursor) -> Self {
        Self {
            status,
            cursor,
            pass: None,
            error_code: None,
            correction: None,
            header_request: None,
            window_request: None,
            persisted_targets: Vec::new(),
            memory_targets: Vec::new(),
        }
    }

    fn reported(mut self, mut payload: PassPayload, job: &mut Job<'_, impl PortError>) -> Self {
        payload.windows = usize::from(self.status == ApplyStatus::Written);
        self.error_code = payload.error_code;
        self.pass = Some(payload);
        self.persisted_targets = std::mem::take(&mut job.persisted_targets);
        self.memory_targets = std::mem::take(&mut job.memory.targets);
        self
    }
}

/// An Orbit activity failed after prepare selected a window.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailInput {
    /// Current prepared basis, or null when no activity was pending.
    pub basis: Option<Basis>,
    /// Stable, allowlisted model failure code. Without one, the code is
    /// read from `errors`.
    pub error_code: Option<String>,
    /// Optional diagnostic, redacted and bounded before use.
    pub reason: Option<String>,
    /// The failed fan-in's position-aligned worker errors (Orbit's
    /// `fan_in.errors`): null for a worker that succeeded or was skipped.
    #[serde(default)]
    pub errors: Vec<Option<String>>,
}

/// The stable model code for an activity failure: an allowlisted
/// `error_code`, else the class Orbit's failure message names.
pub(crate) fn model_code(error_code: Option<&str>, message: Option<&str>) -> &'static str {
    match error_code {
        Some("model_timeout") => return "model_timeout",
        Some("model_unavailable") => return "model_unavailable",
        Some("model_malformed") => return "model_malformed",
        Some("model_failed") => return "model_failed",
        _ => {}
    }
    let Some(message) = message.map(str::to_ascii_lowercase) else {
        return "model_failed";
    };
    if [
        "[provider_unavailable]",
        "[provider_capacity",
        "[provider_limit]",
    ]
    .iter()
    .any(|marker| message.contains(marker))
    {
        "model_unavailable"
    } else if ["timeout", "timed out", "wall clock", "wall-clock"]
        .iter()
        .any(|marker| message.contains(marker))
    {
        "model_timeout"
    } else {
        "model_failed"
    }
}

pub(crate) fn fail<E: PortError>(
    ctx: &Ctx<'_, E>,
    host: &Label,
    input: FailInput,
) -> Result<Option<PassPayload>, PassRunError<E>> {
    let Some(basis) = input.basis else {
        return Ok(None);
    };
    let basis = basis.checked()?;
    let config = ctx.store.read().tags().config()?;
    let redactor = ctx
        .redaction
        .redactor(&config)
        .map_err(PassRunError::Port)?;
    let worker_error = input.errors.into_iter().flatten().next();
    let code = model_code(
        input.error_code.as_deref(),
        worker_error.as_deref().or(input.reason.as_deref()),
    );
    let failure = PassError::new(
        code,
        input
            .reason
            .or(worker_error)
            .unwrap_or_else(|| "the model failed".into()),
    );
    if !record_failure(
        ctx,
        &basis.session_id,
        &basis.switch,
        basis.expected_updated,
        true,
    )? {
        return Ok(None);
    }
    let job = new_job(
        ctx,
        host,
        &basis.session_id,
        &basis.switch,
        &config,
        &redactor,
        &Tally::default(),
    );
    Ok(Some(job.failure_payload(failure)))
}

/// The named sessions, resolved when asked.
fn named<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: &SessionPassInput,
) -> Result<Vec<SessionId>, PassRunError<E>> {
    let mut named = input.sessions.clone();
    if input.resolve {
        for id in &mut named {
            match ctx.store.read().sessions().resolve(id) {
                Ok(resolved) => *id = resolved,
                Err(crate::Error::SessionNotFound { .. }) => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
    Ok(named)
}

/// How many sessions one run may admit.
fn admit_max(input: &SessionPassInput, named: &[SessionId]) -> usize {
    if named.is_empty() {
        input
            .max_sessions
            .unwrap_or(pass::MAX_PER_RUN)
            .min(pass::MAX_PER_RUN)
    } else {
        input.max_sessions.unwrap_or(named.len())
    }
}

/// Refuse a catalog selection a catalog run could not use: a reserved
/// author, anything but exactly one named session, or a session whose switch
/// is not a catalog switch. Run when the run is selected and again on every
/// continuation.
fn check_catalog<E: PortError>(
    catalog: &CatalogSelection,
    named: &[SessionId],
    state: &crate::ControlState,
) -> Result<(), PassRunError<E>> {
    crate::check_agent_author(&catalog.author)?;
    let [id] = named else {
        return Err(PassRunError::Input(
            "a catalog run names exactly one session",
        ));
    };
    if state
        .get(id)
        .is_some_and(|switch| switch.cataloged_by.is_none())
    {
        return Err(PassRunError::Input(
            "a catalog run needs the session's catalog switch",
        ));
    }
    Ok(())
}

/// Refuse a continuation the selection could not have produced: more
/// sessions than the run admits, a repeated or unnamed session, a position
/// past its end, or a catalog selection other than the one the run began
/// with. The seal covers the rest.
fn check_progress<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: &SessionPassInput,
    cursor: Cursor,
) -> Result<Cursor, PassRunError<E>> {
    let cursor = cursor.checked()?;
    if input.catalog != cursor.catalog {
        return Err(PassRunError::Input(
            "the catalog selection does not match the pass cursor",
        ));
    }
    let named = named(ctx, input)?;
    let unique: std::collections::BTreeSet<&SessionId> = cursor.sessions.iter().collect();
    if cursor.next > cursor.sessions.len()
        || unique.len() != cursor.sessions.len()
        || cursor.sessions.len() > admit_max(input, &named)
        || (!named.is_empty() && cursor.sessions.iter().any(|id| !named.contains(id)))
    {
        return Err(PassRunError::Input(
            "the pass cursor does not match the session selection",
        ));
    }
    if let Some(catalog) = &cursor.catalog {
        check_catalog(catalog, &named, &ctx.store.read().sessions().control()?)?;
    }
    Ok(cursor)
}

fn select<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: &SessionPassInput,
) -> Result<Cursor, PassRunError<E>> {
    let named = named(ctx, input)?;
    for id in &named {
        refuse_forgotten(ctx.store, id)?;
    }
    let state = ctx.store.read().sessions().control()?;
    if let Some(catalog) = &input.catalog {
        check_catalog(catalog, &named, &state)?;
    }
    let selected = if named.is_empty() {
        control::due(&state)
    } else {
        control::chosen(&state, &named).map_err(PassRunError::SessionNotOn)?
    };
    let max = admit_max(input, &named);
    let now = ctx.clock.now();
    let mut cursor = Cursor {
        catalog: input.catalog.clone(),
        ..Cursor::default()
    };
    for (id, switch) in selected {
        let ready = !named.is_empty() || switch.retry_after.is_none_or(|at| now >= at);
        if ready && cursor.sessions.len() < max {
            cursor.sessions.push(id);
        } else {
            cursor.remaining += 1;
        }
    }
    Ok(cursor)
}

pub(crate) fn prepare<E: PortError>(
    ctx: &Ctx<'_, E>,
    host: &Label,
    input: PrepareInput,
) -> Result<Prepared, PassRunError<E>> {
    let cursor = match input.progress.into_iter().next() {
        Some(progress) => check_progress(ctx, &input.selection, progress.cursor)?,
        None => {
            let mut cursor = select(ctx, &input.selection)?;
            // The job's run starts here: later sessions and windows start
            // only before the cutoff. A catalog has no deadline.
            if input.selection.catalog.is_none() {
                cursor.start_by = Some(
                    ctx.clock
                        .now()
                        .unix_timestamp()
                        .saturating_add(PASS_START_BY_SECONDS),
                );
            }
            cursor
        }
    };
    prepare_one(ctx, host, &input.selection, cursor, false)
}

pub(crate) fn settle_only<E: PortError>(
    ctx: &Ctx<'_, E>,
    host: &Label,
    selection: SessionPassInput,
) -> Result<PassReport, PassRunError<E>> {
    let mut cursor = select(ctx, &selection)?;
    let mut passes = Vec::new();
    let mut forced = Vec::new();
    let mut pending = 0;
    loop {
        let prepared = prepare_one(ctx, host, &selection, cursor, true)?;
        cursor = prepared.cursor;
        passes.extend(prepared.settled);
        if prepared.done {
            break;
        }
        if prepared.needs_model {
            pending += 1;
            if !selection.sessions.is_empty()
                && let Some(id) = cursor.sessions.get(cursor.next)
            {
                forced.push(id.clone());
            }
            cursor.advance();
        }
    }
    let remaining = cursor.pending() + pending;
    let notices = if pending > 0 {
        let mut command = "orbit job run scribe-session-pass".to_owned();
        if !forced.is_empty() {
            command.push_str(&format!(" --input 'sessions={}'", json!(forced)));
        }
        vec![format!("session windows are ready; run {command}")]
    } else if passes.is_empty() {
        vec![if remaining == 0 {
            "no session pass is due".into()
        } else {
            format!("{remaining} due session pass(es) waiting for retry backoff")
        }]
    } else {
        Vec::new()
    };
    Ok(PassReport {
        passes,
        remaining,
        notices,
        catalog: false,
        switch_written: false,
        persisted_targets: Vec::new(),
        memory_targets: Vec::new(),
    })
}

fn prepare_one<E: PortError>(
    ctx: &Ctx<'_, E>,
    host: &Label,
    selection: &SessionPassInput,
    cursor: Cursor,
    settle_only: bool,
) -> Result<Prepared, PassRunError<E>> {
    let mut result = Prepared {
        done: false,
        needs_model: false,
        settled: Vec::new(),
        cursor,
        basis: None,
        window_request: None,
    };
    // The earlier of the run's cutoff and the caller's deadline. The run's
    // first window always starts; once a session or window has been
    // handled, nothing new starts after the cutoff.
    let cutoff = match (result.cursor.start_by, selection.deadline_epoch_s) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let started = result.cursor.next > 0 || result.cursor.tally.windows > 0;
    let expired = started && cutoff.is_some_and(|at| ctx.clock.now().unix_timestamp() >= at);
    let id = result
        .cursor
        .sessions
        .get(result.cursor.next)
        .cloned()
        .filter(|_| !expired);
    let Some(id) = id else {
        result.done = true;
        return Ok(result);
    };
    let found = ctx.store.read().sessions().loaded()?;
    let allowed = |switch: &&SessionControl| {
        switch.on
            && (!selection.sessions.is_empty()
                || (switch.due && switch.retry_after.is_none_or(|at| ctx.clock.now() >= at)))
    };
    let Some(switch) = found.control.get(&id).filter(allowed) else {
        result.cursor.advance();
        return Ok(result);
    };
    let config = ctx
        .store
        .read()
        .tags()
        .config()?
        .for_provider(switch.provider);
    let redactor = ctx
        .redaction
        .redactor(&config)
        .map_err(PassRunError::Port)?;
    let previous = found.records.iter().find(|r| r.id == id);
    let corrupt = found
        .skipped
        .iter()
        .find(|(sid, _)| sid.as_ref() == Some(&id));
    let mut job = new_job(
        ctx,
        host,
        &id,
        switch,
        &config,
        &redactor,
        &result.cursor.tally,
    );
    job.segments = previous.map(|p| p.log.len());
    job.catalog_project = selection
        .catalog
        .as_ref()
        .and_then(|catalog| catalog.project.clone());
    let data = match corrupt {
        Some((_, skipped)) => Err(PassError::new("store_corrupt", skipped.error.to_string())),
        None => job.read_transcript(),
    };
    let selected = data.and_then(|data| {
        let Some(data) = data else {
            return job
                .nothing_new(previous, true)
                .map(|payload| Preparation::Settled(Box::new(payload)));
        };
        let offset = pass::start_after(
            &data.conversation,
            previous.and_then(|p| p.scribed_through.as_deref()),
        );
        if offset >= data.conversation.len() {
            return job
                .nothing_new(previous, false)
                .map(|payload| Preparation::Settled(Box::new(payload)));
        }
        if settle_only {
            return Ok(Preparation::NeedsModel);
        }
        let known = Known::new(&found.records);
        let Some(window) = job.next_window(previous, &known, &data, None)? else {
            return job
                .nothing_new(previous, false)
                .map(|payload| Preparation::Settled(Box::new(payload)));
        };
        let basis = Basis {
            session_id: id.clone(),
            expected_updated: previous.map(|p| p.updated),
            expected_through: previous.and_then(|p| p.scribed_through.clone()),
            switch: switch.clone(),
            window_end_uuid: window.through.clone(),
            last_window: window.last,
            digest: digest(&data, &window),
            catalog: selection.catalog.clone(),
            cursor_seal: result.cursor.seal_of(),
            seal: String::new(),
        }
        .sealed();
        Ok(Preparation::Window(Box::new((
            basis,
            masked_input(&window.request, &redactor),
        ))))
    });
    match selected {
        Ok(Preparation::NeedsModel) => result.needs_model = true,
        Ok(Preparation::Window(window)) => {
            let (basis, request) = *window;
            result.needs_model = true;
            result.basis = Some(basis);
            result.window_request = Some(request);
        }
        Ok(Preparation::Settled(mut payload)) => {
            payload.windows = 0;
            result.settled.push(*payload);
            result.cursor.advance();
        }
        Err(err) => {
            // A store or transcript error backs the session off like any
            // failed pass, so the next run moves on to the sessions after it.
            if err.code != "record_changed" {
                record_failure(
                    ctx,
                    &id,
                    switch,
                    previous.map(|p| p.updated),
                    corrupt.is_none(),
                )?;
            }
            result.cursor.remaining += 1;
            let mut payload = job.failure_payload(err);
            payload.windows = 0;
            result.settled.push(payload);
            result.cursor.advance();
        }
    }
    Ok(result)
}

enum Preparation {
    NeedsModel,
    Settled(Box<PassPayload>),
    Window(Box<(Basis, Value)>),
}

pub(crate) fn apply<E: PortError>(
    ctx: &Ctx<'_, E>,
    host: &Label,
    input: ApplyInput,
) -> Result<ApplyOutput, PassRunError<E>> {
    let ApplyInput {
        basis,
        cursor,
        answer,
        header,
        retry_answer,
        repair,
        previous_status,
        correction,
    } = input;
    let mut cursor = cursor.checked()?;
    if repair {
        match previous_status {
            Some(ApplyStatus::RetryWindow | ApplyStatus::RetryHeader) => {}
            Some(ApplyStatus::Idle | ApplyStatus::Written | ApplyStatus::Failed) => {
                return Ok(ApplyOutput::new(ApplyStatus::Idle, cursor));
            }
            None => {
                return Err(PassRunError::Input(
                    "repair requires the first apply status",
                ));
            }
        }
    } else if previous_status.is_some() {
        return Err(PassRunError::Input(
            "previous_status requires a final retry",
        ));
    }
    let Some(basis) = basis else {
        return Ok(ApplyOutput::new(ApplyStatus::Idle, cursor));
    };
    let basis = basis.checked()?;
    let id = &basis.session_id;
    if basis.cursor_seal != cursor.seal_of() {
        return Err(PassRunError::Input(
            "the pass cursor is not the one the basis was prepared with",
        ));
    }
    if cursor.sessions.get(cursor.next) != Some(id) {
        return Err(PassRunError::Input(
            "the pass cursor does not name the prepared session",
        ));
    }
    let first = answer.and_then(ActivityAnswer::first);
    let header = header.and_then(ActivityAnswer::first);
    let answer = if previous_status == Some(ApplyStatus::RetryWindow) {
        retry_answer.and_then(ActivityAnswer::first)
    } else {
        first.clone()
    };
    // A header retry needs both the first answer and the compressed header.
    let answer =
        answer.filter(|_| previous_status != Some(ApplyStatus::RetryHeader) || header.is_some());

    let found = ctx.store.read().sessions().loaded()?;
    let previous = found.records.iter().find(|r| &r.id == id);
    let known = Known::new(&found.records);
    let config = ctx
        .store
        .read()
        .tags()
        .config()?
        .for_provider(basis.switch.provider);
    let redactor = ctx
        .redaction
        .redactor(&config)
        .map_err(PassRunError::Port)?;
    let mut job = new_job(
        ctx,
        host,
        id,
        &basis.switch,
        &config,
        &redactor,
        &cursor.tally,
    );
    job.segments = previous.map(|p| p.log.len());
    job.catalog_project = basis
        .catalog
        .as_ref()
        .and_then(|catalog| catalog.project.clone());

    // The activity failed: its fallback recorded backoff (this records it
    // when nothing did), and the run moves on to the next session.
    let Some(mut answer) = answer else {
        let recorded = record_failure(ctx, id, &basis.switch, basis.expected_updated, true)?;
        cursor.remaining += 1;
        cursor.advance();
        let reply = ApplyOutput::new(ApplyStatus::Failed, cursor);
        return Ok(if recorded {
            let payload = job.failure_payload(PassError::new(
                "model_failed",
                "the session distillation activity returned no answer",
            ));
            reply.reported(payload, &mut job)
        } else {
            reply
        });
    };

    let stale =
        |mut cursor: Cursor, job: &mut Job<'_, E>| -> Result<ApplyOutput, PassRunError<E>> {
            // An explicit selection may start while not due. Keep that
            // obligation if the record changed in place. A different cursor
            // can mean another writer already finished the prepared window.
            if !basis.switch.due
                && previous.and_then(|record| record.scribed_through.as_ref())
                    == basis.expected_through.as_ref()
            {
                record_failure(
                    ctx,
                    id,
                    &basis.switch,
                    previous.map(|record| record.updated),
                    true,
                )?;
            }
            cursor.remaining += 1;
            cursor.advance();
            let payload = job.failure_payload(PassError::new(
                "record_changed",
                "the session record or transcript changed since it was loaded",
            ));
            Ok(ApplyOutput::new(ApplyStatus::Failed, cursor).reported(payload, job))
        };
    // A store or transcript error fails the pass with its code and backs the
    // session off, so the next run moves on to the sessions after it.
    let failed = |mut cursor: Cursor,
                  job: &mut Job<'_, E>,
                  err: PassError|
     -> Result<ApplyOutput, PassRunError<E>> {
        if !record_failure(ctx, id, &basis.switch, basis.expected_updated, true)? {
            return stale(cursor, job);
        }
        cursor.remaining += 1;
        cursor.advance();
        let payload = job.failure_payload(err);
        Ok(ApplyOutput::new(ApplyStatus::Failed, cursor).reported(payload, job))
    };
    if found.control.get(id) != Some(&basis.switch)
        || !basis.switch.on
        || found.forgotten.contains_key(id)
        || previous.map(|p| p.updated) != basis.expected_updated
    {
        return stale(cursor, &mut job);
    }
    let data = match job.read_transcript() {
        Ok(Some(data)) => data,
        Ok(None) => return stale(cursor, &mut job),
        Err(err) => return failed(cursor, &mut job, err),
    };
    let window = match job.next_window(
        previous,
        &known,
        &data,
        Some((&basis.window_end_uuid, basis.last_window)),
    ) {
        Ok(Some(window)) if digest(&data, &window) == basis.digest => window,
        Ok(_)
        | Err(PassError {
            code: "record_changed",
            ..
        }) => return stale(cursor, &mut job),
        Err(err) => return failed(cursor, &mut job, err),
    };

    let conversation = &data.conversation[..window.end];
    let voice = || config.voice(crate::voice::ModelUse::SessionHeader);
    // The final attempt recomputes the first apply's status from the first
    // answer, so a forwarded status cannot skip the check or the retry.
    if let Some(status) = previous_status {
        let Some(first) = first else {
            return Err(PassRunError::Input(
                "a final retry requires the first session answer",
            ));
        };
        let recomputed = match pass::check(first.clone(), conversation, &window.request) {
            Ok(_) => ApplyStatus::Written,
            Err(_)
                if pass::compression_request(&first, conversation, &window.request, voice())
                    .is_some() =>
            {
                ApplyStatus::RetryHeader
            }
            Err(_) => ApplyStatus::RetryWindow,
        };
        if recomputed != status {
            return Err(PassRunError::Input(
                "previous_status is not the status of the first answer",
            ));
        }
        if status == ApplyStatus::RetryHeader
            && let Some(header) = header
        {
            answer.replace_header(header);
        }
    }
    let distilled = match pass::check(answer.clone(), conversation, &window.request) {
        Ok(distilled) => Ok(distilled),
        Err(_) if repair => pass::repair(answer, conversation, &window.request),
        Err(err) => {
            let refusal = super::short_reason(&redactor, &err.to_string());
            if let Some(request) =
                pass::compression_request(&answer, conversation, &window.request, voice())
            {
                let mut reply = ApplyOutput::new(ApplyStatus::RetryHeader, cursor);
                reply.correction = Some(refusal);
                let mut request = request;
                request.voice.instructions = request
                    .voice
                    .instructions
                    .take()
                    .map(|text| request.voice.fill(&text));
                reply.header_request = Some(mask_value(json!(request), &redactor));
                return Ok(reply);
            }
            let mut reply = ApplyOutput::new(ApplyStatus::RetryWindow, cursor);
            reply.correction = Some(refusal.clone());
            let request = SessionWindowInput {
                correction: Some(refusal),
                ..window.request
            };
            reply.window_request = Some(masked_input(&request, &redactor));
            return Ok(reply);
        }
    };
    match distilled {
        Ok(distilled) => {
            let record = match job.commit(previous, &data, &window, distilled) {
                Ok(record) => record,
                Err(err) if err.code == "record_changed" => return stale(cursor, &mut job),
                Err(err) => return failed(cursor, &mut job, err),
            };
            let payload = job.payload("written", record.as_ref());
            if window.last {
                cursor.advance();
            } else {
                cursor.tally = Tally::after(&job);
            }
            Ok(ApplyOutput::new(ApplyStatus::Written, cursor).reported(payload, &mut job))
        }
        Err(err) => {
            if !record_failure(ctx, id, &basis.switch, basis.expected_updated, true)? {
                return stale(cursor, &mut job);
            }
            let reason = correction
                .map_or_else(|| err.to_string(), |first| format!("{first}; retry: {err}"));
            let payload = job.failure_payload(PassError::new("session_answer_invalid", reason));
            cursor.remaining += 1;
            cursor.advance();
            Ok(ApplyOutput::new(ApplyStatus::Failed, cursor).reported(payload, &mut job))
        }
    }
}

// A fallback retried after a commit or another failure cannot penalize it.
fn record_failure<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &SessionId,
    expected_switch: &SessionControl,
    expected_updated: Option<OffsetDateTime>,
    check_record: bool,
) -> crate::Result<bool> {
    ctx.store.write(|tx| {
        let mut loaded = tx.sessions().loaded()?;
        if loaded.control.get(id) != Some(expected_switch)
            || !expected_switch.on
            || loaded.forgotten.contains_key(id)
            || (check_record
                && loaded.take(id).transpose()?.as_ref().map(|r| r.updated) != expected_updated)
        {
            return Ok(false);
        }
        tx.sessions()
            .change_control(|state| control::fail(state, id, ctx.clock.now()))?;
        Ok(true)
    })
}

fn digest(data: &Conversation, window: &Window) -> String {
    let through = data
        .messages
        .iter()
        .position(|m| m.uuid == window.through)
        .map_or(data.messages.as_slice(), |n| &data.messages[..=n]);
    crate::text::bytes_sha256(
        json!([window.request.messages, data.cwd, through])
            .to_string()
            .as_bytes(),
    )
}

/// A SHA-256 seal over `value`, labeled by what it seals.
pub(crate) fn seal(label: &str, value: &Value) -> String {
    crate::text::bytes_sha256(json!([label, value]).to_string().as_bytes())
}

pub(crate) fn masked_input(request: &SessionWindowInput, redactor: &Redactor) -> Value {
    let mut request = request.clone();
    request.voice.instructions = request
        .voice
        .instructions
        .take()
        .map(|text| request.voice.fill(&text));
    mask_value(json!(request), redactor)
}

pub(crate) fn mask_value(value: Value, redactor: &Redactor) -> Value {
    match value {
        Value::String(text) => Value::String(redactor.mask(&text)),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, mask_value(value, redactor)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|value| mask_value(value, redactor))
                .collect(),
        ),
        other @ (Value::Null | Value::Bool(_) | Value::Number(_)) => other,
    }
}

fn new_job<'a, E>(
    ctx: &'a Ctx<'a, E>,
    host: &'a Label,
    id: &'a SessionId,
    switch: &'a SessionControl,
    config: &'a Config,
    redactor: &'a Redactor,
    tally: &Tally,
) -> Job<'a, E> {
    Job {
        ctx,
        provider: switch.provider,
        host,
        config,
        redactor,
        id,
        switch,
        candidate_slots: tally.candidates.min(pass::MAX_CANDIDATES),
        write_slots: tally.writes.min(pass::MAX_COLLECTED),
        memory: MemoryTally {
            slots: tally.memory.min(memory::MAX_PER_PASS),
            ..MemoryTally::default()
        },
        windows: tally.windows.min(25),
        segments_sealed: 0,
        segments: None,
        dropped_terms: 0,
        warnings: Vec::new(),
        check_switch: true,
        catalog: None,
        catalog_project: None,
        persisted_targets: Vec::new(),
    }
}
