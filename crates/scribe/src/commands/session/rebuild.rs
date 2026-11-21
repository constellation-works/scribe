//! Rebuild a record from the first transcript message. Each window is written
//! to an isolated dot-file; only the complete rebuild replaces the record.

use super::pass::{distill, short_reason, window_warnings};
use crate::app::Context;
use crate::commands::hook::payload::{self, Transcript};
use crate::error::CliError;
use crate::output::{Output, PassPayload, PassReport};
use clap::Args;
use scribe_core::{
    Author, AuthorName, Config, Models, SegmentOrigin, SegmentState, SessionId, SessionStatus,
    pass, transcript,
};
use std::collections::BTreeSet;

pub(in crate::commands) const COMMAND: &str = "session rebuild";
pub(super) const EXAMPLES: &str = "\
Replaces the header and log only after every transcript window succeeds.
Interrupted runs resume their private checkpoint; --confirm is required.

Examples:
  scribe session rebuild <id> --author <name> --confirm
  scribe session rebuild <id> --author <name> --confirm --json";

#[derive(Debug, Args)]
pub(crate) struct RebuildArgs {
    /// Session record to replace: its id or a unique prefix of it (at least four characters)
    #[arg(value_name = "ID")]
    pub(crate) id: SessionId,
    /// Name of the agent requesting the rebuild (lowercase letters, digits, '.', '_' or '-')
    #[arg(long, value_name = "NAME")]
    pub(crate) author: AuthorName,
    /// Confirm replacing the current header and log
    #[arg(long)]
    pub(crate) confirm: bool,
}

pub(super) fn run(args: &RebuildArgs, ctx: &Context) -> Result<Output, CliError> {
    run_with(args, ctx, |config| ctx.models(config))
}

pub(in crate::commands) fn run_with<M: Models>(
    args: &RebuildArgs,
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
) -> Result<Output, CliError> {
    scribe_core::check_agent_author(&args.author)?;
    let store = ctx.store();
    let id = store.resolve_session(&args.id)?;
    let mut original = store.session(&id)?.record;
    let scoped = ctx.with_provider(
        original
            .provider
            .as_str()
            .parse()
            .map_err(CliError::Usage)?,
    );
    let ctx = &scoped;
    if !args.confirm {
        return Err(CliError::ConfirmRequired {
            id: id.clone(),
            segments: original.log.len(),
            passes: original.passes,
        });
    }
    if store.control()?.get(&id).is_some_and(|c| c.on && c.due) {
        return Err(scribe_core::Error::SessionBusy(id.clone()).into());
    }
    let path = payload::locate(ctx, &id)?.ok_or_else(|| CliError::TranscriptMissing(id.clone()))?;
    // An early refusal before the lock and the model are set up; it reads
    // nothing. The loop below checks and reads each pass through one handle.
    match payload::check(ctx, &path, &id) {
        Ok(Transcript::Present) => {}
        Ok(Transcript::Missing) => return Err(CliError::TranscriptMissing(id.clone())),
        Err(reason) => {
            return Err(CliError::TranscriptRefused {
                id: id.clone(),
                reason,
            });
        }
    }
    let config = ctx.config()?;
    let redactor = ctx.redactor(&config)?;
    let adapter = models(&config);
    let _rebuild = store.lock_rebuild(&original)?;
    // Re-read after taking the per-session lock: another rebuilder may have finished.
    original = store.session(&id)?.record;
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
        author: Some(scribe_core::Author {
            kind: scribe_core::AuthorKind::Agent,
            name: args.author.clone(),
        }),
        memory_written: 0,
        memory_refused: std::collections::BTreeMap::new(),
    };
    let records = store
        .list_sessions(&scribe_core::SessionFilter {
            include_inactive: true,
            ..Default::default()
        })?
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
        let bytes = match payload::read_checked(ctx, &path, &id) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(CliError::TranscriptMissing(id.clone())),
            Err(payload::Unreadable::Refused(reason)) => {
                return Err(CliError::TranscriptRefused {
                    id: id.clone(),
                    reason,
                });
            }
            Err(payload::Unreadable::Io(source)) => {
                return Err(CliError::Read {
                    what: path.display().to_string(),
                    source,
                });
            }
        };
        let messages = transcript::parse(&bytes);
        let conversation = pass::conversation(&messages);
        if conversation.is_empty() {
            return Err(CliError::TranscriptMissing(id.clone()));
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
            let now = ctx.now();
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
                    || store.control()?.get(&id).is_some_and(|s| s.final_pass));
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
            );
            let answer = match distill(&id, request, &conversation[..end], &adapter) {
                Ok(answer) => answer,
                Err(error) => {
                    report.result = "failed";
                    report.error_code = Some(error.code);
                    report.error = Some(short_reason(&redactor, &error.message));
                    return Ok(Output::Pass(PassReport {
                        passes: vec![report],
                        remaining: 0,
                        notices: vec![String::from(
                            "rebuild checkpoint kept; repeat session rebuild with --confirm to resume",
                        )],
                        catalog: false,
                        switch_written: false,
                        persisted_targets: Vec::new(),
                        memory_targets: Vec::new(),
                    }));
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
            return Err(CliError::TranscriptMissing(id.clone()));
        };
        let switch = store.control()?;
        let final_pass = original.status == SessionStatus::Final
            || switch.get(&id).is_some_and(|s| s.final_pass);
        let sealed_at_finish = final_pass && finished.open_segment().is_some();
        if final_pass {
            finished = pass::finalized(&finished, ctx.now());
        }
        finished.updated = ctx.now();
        finished.rebuilt_by = Some(Author::agent(args.author.clone()));
        match store.finish_rebuild(finished, expected, &redactor)? {
            Some(written) => {
                report.segments_sealed += usize::from(sealed_at_finish);
                report.passes = Some(written.passes);
                report.segments = Some(written.log.len());
                return Ok(Output::Pass(PassReport {
                    passes: vec![report],
                    remaining: 0,
                    notices: Vec::new(),
                    catalog: false,
                    switch_written: false,
                    persisted_targets: Vec::new(),
                    memory_targets: Vec::new(),
                }));
            }
            None => {
                original = store.session(&id)?.record;
                expected = original.updated;
                // The next iteration re-reads the transcript and folds in any newer messages.
            }
        }
    }
}
