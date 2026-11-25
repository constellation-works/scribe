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

/// Job-local continuation and remaining session budget.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Cursor {
    sessions: Vec<SessionId>,
    next: usize,
    tally: Tally,
    remaining: usize,
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
}

/// Caps across windows of the same session.
#[derive(Clone, Debug, Deserialize, Serialize)]
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

/// Optimistic basis from prepare. Carries no transcript text.
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
    fn first(self) -> Option<T> {
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
    /// Stable, allowlisted model failure code.
    pub error_code: Option<String>,
    /// Optional diagnostic, redacted and bounded before use.
    pub reason: Option<String>,
}

pub(crate) fn fail<E: PortError>(
    ctx: &Ctx<'_, E>,
    host: &Label,
    input: FailInput,
) -> Result<Option<PassPayload>, PassRunError<E>> {
    let Some(basis) = input.basis else {
        return Ok(None);
    };
    let config = ctx.store.read().tags().config()?;
    let redactor = ctx
        .redaction
        .redactor(&config)
        .map_err(PassRunError::Port)?;
    let code = match input.error_code.as_deref() {
        Some("model_timeout") => "model_timeout",
        Some("model_unavailable") => "model_unavailable",
        Some("model_malformed") => "model_malformed",
        _ => "model_failed",
    };
    let failure = PassError::new(
        code,
        input.reason.unwrap_or_else(|| "the model failed".into()),
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

fn select<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: &SessionPassInput,
) -> Result<Cursor, PassRunError<E>> {
    let mut named = input.sessions.clone();
    for id in &mut named {
        if input.resolve {
            match ctx.store.read().sessions().resolve(id) {
                Ok(resolved) => *id = resolved,
                Err(crate::Error::SessionNotFound { .. }) => {}
                Err(err) => return Err(err.into()),
            }
        }
        refuse_forgotten(ctx.store, id)?;
    }
    let state = ctx.store.read().sessions().control()?;
    let (selected, max) = if named.is_empty() {
        (
            control::due(&state),
            input
                .max_sessions
                .unwrap_or(pass::MAX_PER_RUN)
                .min(pass::MAX_PER_RUN),
        )
    } else {
        (
            control::chosen(&state, &named).map_err(PassRunError::SessionNotOn)?,
            input.max_sessions.unwrap_or(named.len()),
        )
    };
    let now = ctx.clock.now();
    let mut cursor = Cursor::default();
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
        Some(progress) => progress.cursor,
        None => select(ctx, &input.selection)?,
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
    let id = result
        .cursor
        .sessions
        .get(result.cursor.next)
        .cloned()
        .filter(|_| {
            selection
                .deadline_epoch_s
                .is_none_or(|deadline| ctx.clock.now().unix_timestamp() < deadline)
        });
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
        };
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
            if err.code != "record_changed" {
                if err.write_error {
                    return Err(PassRunError::Step(err));
                }
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
        mut cursor,
        answer,
        header,
        retry_answer,
        repair,
        previous_status,
        correction,
    } = input;
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
    let id = &basis.session_id;
    if cursor.sessions.get(cursor.next) != Some(id) {
        return Err(PassRunError::Input(
            "the pass cursor does not name the prepared session",
        ));
    }
    let answer = if previous_status == Some(ApplyStatus::RetryWindow) {
        retry_answer
    } else {
        answer
    };
    let Some(mut answer) = answer.and_then(ActivityAnswer::first) else {
        return Err(PassRunError::Input(
            "one session answer is required for the prepared window",
        ));
    };
    if previous_status == Some(ApplyStatus::RetryHeader) {
        let Some(header) = header.and_then(ActivityAnswer::first) else {
            return Err(PassRunError::Input(
                "a compressed session header is required",
            ));
        };
        answer.replace_header(header);
    }

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
    if found.control.get(id) != Some(&basis.switch)
        || !basis.switch.on
        || found.forgotten.contains_key(id)
        || previous.map(|p| p.updated) != basis.expected_updated
    {
        return stale(cursor, &mut job);
    }
    let data = job.read_transcript().map_err(PassRunError::Step)?;
    let Some(data) = data else {
        return stale(cursor, &mut job);
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
        Err(err) => return Err(PassRunError::Step(err)),
    };

    let conversation = &data.conversation[..window.end];
    let distilled = match pass::check(answer.clone(), conversation, &window.request) {
        Ok(distilled) => Ok(distilled),
        Err(_) if repair => pass::repair(answer, conversation, &window.request),
        Err(err) => {
            let refusal = super::short_reason(&redactor, &err.to_string());
            let voice = config.voice(crate::voice::ModelUse::SessionHeader);
            if let Some(request) =
                pass::compression_request(&answer, conversation, &window.request, voice)
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
                Err(err) => return Err(PassRunError::Step(err)),
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

fn masked_input(request: &SessionWindowInput, redactor: &Redactor) -> Value {
    let mut request = request.clone();
    request.voice.instructions = request
        .voice
        .instructions
        .take()
        .map(|text| request.voice.fill(&text));
    mask_value(json!(request), redactor)
}

fn mask_value(value: Value, redactor: &Redactor) -> Value {
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

#[allow(clippy::too_many_arguments)]
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
        persisted_targets: Vec::new(),
    }
}
