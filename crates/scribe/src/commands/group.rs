//! Quote-checked group opening, caller-bound seat membership and roster reads.

use super::hook::{self, ClearedPredecessor, payload};
use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{Output, Record, Records};
use clap::Subcommand;
use scribe_core::group::{GroupCloser, GroupConsent, JoinGroup, NewGroup, automatic_seat};
use scribe_core::{
    AuthorName, CloseGroupOutcome, Label, Models, Project, Provider, Related, SessionId,
    WriteRefusal, message::NewMessage, transcript,
};

const CLOSE_COMMAND: &str = "group close";

pub(super) const EXAMPLES: &str = "Examples:
  scribe group open --title 'Release review' --kind review --project scribe --quote 'open a release review'
  scribe group join <id> --as editor --author aide --role reviewer
  scribe group close <id>
  scribe group show <id> --json";

#[derive(Debug, Subcommand)]
pub(crate) enum GroupCommand {
    /// Open a group on a verbatim quote from the calling session
    #[command(
        after_help = "Example: scribe group open --title 'Release review' --kind review --quote 'open a release review'"
    )]
    Open {
        /// Title of the work
        #[arg(long)]
        title: String,
        /// Free label for the kind of work
        #[arg(long)]
        kind: String,
        /// Project slug
        #[arg(long, default_value = "global")]
        project: Project,
        /// Task, PR or repository reference, repeatable
        #[arg(long)]
        related: Vec<Related>,
        /// Words the user typed in this session
        #[arg(long)]
        quote: Option<String>,
    },
    /// Join a named seat under your own author name
    #[command(
        after_help = "Example: scribe group join <id> --as editor --author aide --role reviewer"
    )]
    Join {
        /// Group id, unique prefix, or project group slug
        id: String,
        /// Seat name (defaults to the short session id for a project group)
        #[arg(long = "as", value_name = "SEAT", value_parser = seat_name)]
        seat: Option<Label>,
        /// Your agent author name
        #[arg(long)]
        author: AuthorName,
        /// Role; defaults to member for a new project-group seat, otherwise the seat name.
        /// Omission on an existing seat keeps its current role.
        #[arg(long)]
        role: Option<String>,
        /// Explicitly replace the newest holder of an occupied seat
        #[arg(long)]
        replace: bool,
    },
    /// Store earlier typed turns for each current seat holder
    #[command(after_help = "Example: scribe group backfill <id> --json")]
    Backfill {
        /// Group id, unique prefix, or project group slug
        id: String,
    },
    /// Remove the calling session from its seat
    #[command(after_help = "Example: scribe group leave")]
    Leave,
    /// Summarize and close the group from its opener, seat holder or a quote
    #[command(after_help = "Example: scribe group close <id>")]
    Close {
        /// Group id, unique prefix, or project group slug
        id: String,
        /// Words the user typed in this session that contain the group id; closes
        /// the group for a session that holds no seat in it
        #[arg(long)]
        quote: Option<String>,
    },
    /// Show one group, its references and its seat chains
    #[command(after_help = "Example: scribe group show <id> --json")]
    Show {
        /// Group id, unique prefix, or project group slug
        id: String,
    },
    /// List groups newest first with their rosters
    #[command(after_help = "Example: scribe group list --json")]
    List,
}

fn seat_name(value: &str) -> Result<Label, scribe_core::RecordError> {
    Label::try_from(value.to_owned())
}

impl GroupCommand {
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        let command = match self {
            Self::Open { .. } => "group open",
            Self::Join { .. } => "group join",
            Self::Leave => "group leave",
            Self::Backfill { .. } => "group backfill",
            Self::Close { .. } => CLOSE_COMMAND,
            Self::List | Self::Show { .. } => return None,
        };
        Some(AuditMeta {
            command,
            stepwise: matches!(self, Self::Close { .. }),
        })
    }

    pub(super) fn subject(&self, ctx: &Context) -> AuditSubject {
        match self {
            Self::Join { id, author, .. } => AuditSubject {
                author: Some(author.to_string()),
                target: ctx.store().group(id).ok().map(|g| g.id),
            },
            Self::Close { id, .. } | Self::Backfill { id } => AuditSubject {
                author: None,
                target: ctx.store().group(id).ok().map(|g| g.id),
            },
            Self::Open { .. } | Self::Leave | Self::List | Self::Show { .. } => {
                AuditSubject::default()
            }
        }
    }
}

fn caller(ctx: &Context) -> Result<SessionId, CliError> {
    let key = match ctx.provider() {
        Provider::ClaudeCode => "CLAUDE_CODE_SESSION_ID",
        Provider::Codex => "CODEX_THREAD_ID",
    };
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            CliError::Usage(format!(
                "run this command in the calling session with a valid {key}"
            ))
        })
}

/// Missing or unverifiable evidence leaves the seat occupied. A predecessor
/// can be from either provider in this host's store. Only the predecessor's
/// own transcript is checked: another session's duplicate id elsewhere in the
/// tree is not evidence about it, while a duplicate of the predecessor's own
/// id still refuses.
fn ended(ctx: &Context, session: &SessionId) -> Result<bool, CliError> {
    let mut found = false;
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let view = ctx.with_provider(provider);
        if let Some(path) = payload::locate_own(&view, session)? {
            found = true;
            let old = payload::read_checked(&view, &path, session)
                .ok()
                .flatten()
                .and_then(|bytes| transcript::survey(&bytes).ended)
                .is_some_and(|last| transcript::is_day_old(last, ctx.now()));
            if !old {
                return Ok(false);
            }
        }
    }
    Ok(found)
}

/// The caller's typed turns from the start of its own transcript, for the
/// group it joins. A join does not need the transcript: one that is missing or
/// fails its check joins with no earlier turns, as it always has.
fn earlier_turns(ctx: &Context, session: &SessionId) -> Result<Vec<NewMessage>, CliError> {
    let Some(path) = payload::locate_own(ctx, session)? else {
        return Ok(Vec::new());
    };
    match payload::read_checked(ctx, &path, session) {
        Ok(Some(bytes)) => hook::typed_turns(ctx, session, &bytes),
        Ok(None) => Ok(Vec::new()),
        Err(payload::Unreadable::Refused(reason)) => {
            tracing::warn!(%session, %reason, "group join stores no earlier turns: transcript refused");
            Ok(Vec::new())
        }
        Err(payload::Unreadable::Io(source)) => Err(CliError::Read {
            what: path.display().to_string(),
            source,
        }),
    }
}

/// The caller's transcript bytes once the provenance check passes. `Ok(None)`
/// is evidence that is missing or refused, which the caller refuses without a
/// model call. A discovery or read I/O failure is an error, so its diagnostic
/// and `io_error` audit code survive instead of reading as missing evidence.
fn checked_transcript(ctx: &Context, session: &SessionId) -> Result<Option<Vec<u8>>, CliError> {
    let Some(path) = payload::locate(ctx, session)? else {
        return Ok(None);
    };
    match payload::read_checked(ctx, &path, session) {
        Ok(bytes) => Ok(bytes),
        Err(payload::Unreadable::Refused(_)) => Ok(None),
        Err(payload::Unreadable::Io(source)) => Err(CliError::Read {
            what: path.display().to_string(),
            source,
        }),
    }
}

pub(super) fn run(command: GroupCommand, ctx: &Context) -> Result<Output, CliError> {
    let store = ctx.store();
    let mut notices = Vec::new();
    let (id, changed) = match command {
        GroupCommand::Open {
            title,
            kind,
            project,
            related,
            quote,
        } => {
            let session = caller(ctx)?;
            ctx.note_instruction_session(session.as_str());
            let quote = quote.filter(|q| !q.trim().is_empty()).ok_or_else(|| {
                scribe_core::Error::AgentWrite {
                    kind: WriteRefusal::QuoteRequired,
                    message: "pass --quote with the user's own words from this session".into(),
                }
            })?;
            let path = payload::locate(ctx, &session)?;
            let bytes = path
                .and_then(|p| payload::read_checked(ctx, &p, &session).ok().flatten())
                .ok_or_else(|| scribe_core::Error::AgentWrite {
                    kind: WriteRefusal::QuoteNotFound,
                    message: format!(
                        "cannot verify a typed quote in session {session} on this host"
                    ),
                })?;
            let consent = GroupConsent::verify(session, ctx.provider(), &bytes, &quote)?;
            ctx.note_instruction_message(consent.message());
            let draft = NewGroup {
                title,
                kind,
                project,
                related: super::resolve_relateds(ctx, &related)?,
            };
            // The opener's earlier typed turns are group record from its first
            // line. A transcript that cannot be read as turns stores none.
            let earlier = hook::typed_turns(ctx, consent.session(), &bytes)?;
            let opened = store.open_group(
                draft,
                &consent,
                &earlier,
                ctx.now(),
                &ctx.redactor(&ctx.config()?)?,
            )?;
            for id in opened.still_open {
                notices.push(format!("warning: group {id} is still open in this project"));
            }
            (opened.id, true)
        }
        GroupCommand::Join {
            id,
            seat,
            author,
            role,
            replace,
        } => {
            let caller = caller(ctx)?;
            let group = store.group(&id)?;
            let seat = match seat {
                Some(seat) => seat,
                None if group.slug.is_some() => {
                    // A retry keeps the seat this session already holds, even
                    // when the short name it first missed has since been freed.
                    match group.seats.iter().find(|s| s.transcripts.contains(&caller)) {
                        Some(held) => Label::sanitized(&held.name, "member"),
                        None => automatic_seat(&caller, |name| {
                            Ok::<_, std::convert::Infallible>(
                                group.seats.iter().any(|s| s.name == name),
                            )
                        })
                        .unwrap_or_else(|e| match e {}),
                    }
                }
                None => return Err(CliError::Usage("pass --as for a named seat".into())),
            };
            let seat_exists = group.seats.iter().any(|s| s.name == seat.as_str());
            let role = role
                .or_else(|| (group.slug.is_some() && !seat_exists).then(|| "member".to_owned()));
            let latest = group
                .seats
                .iter()
                .find(|s| s.name == seat.as_str())
                .and_then(|s| s.transcripts.last());
            let (ended, predecessor) = match latest {
                Some(previous) if !replace && previous != &caller => {
                    let predecessor = if ctx.provider() == Provider::ClaudeCode {
                        let path = payload::locate_own(ctx, &caller)?;
                        match hook::cleared_predecessor(ctx, path.as_deref(), &caller) {
                            ClearedPredecessor::One(parent) if &parent == previous => Some(parent),
                            ClearedPredecessor::None
                            | ClearedPredecessor::One(_)
                            | ClearedPredecessor::Multiple(_) => None,
                        }
                    } else {
                        None
                    };
                    let ended = if predecessor.is_none() && ended(ctx, previous)? {
                        Some(previous)
                    } else {
                        None
                    };
                    (ended, predecessor)
                }
                _ => (None, None),
            };
            // Taken before the transcript is read, so a first turn appended
            // after an empty snapshot is not dated before the membership.
            let snapshot_at = ctx.now();
            let earlier = earlier_turns(ctx, &caller)?;
            let join = JoinGroup {
                group: &group.id,
                session: &caller,
                seat: &seat,
                author: &author,
                role: role.as_deref(),
                replace,
                ended,
                predecessor: predecessor.as_ref(),
                earlier_turns: &earlier,
                snapshot_at,
            };
            let changed = store.join_group(join, &ctx.redactor(&ctx.config()?)?)?;
            if !changed {
                notices.push("session already holds this seat".into());
            }
            (group.id, changed)
        }
        GroupCommand::Backfill { id } => {
            let group = store.group(&id)?;
            if group.closed.is_some() {
                return Err(scribe_core::Error::AgentWrite {
                    kind: WriteRefusal::GroupMembership,
                    message: "the group is already closed".into(),
                }
                .into());
            }
            let mut report = crate::output::GroupBackfill {
                group: group.id.clone(),
                members: Vec::new(),
                stored: 0,
                skipped: 0,
            };
            let mut batches = Vec::new();
            for seat in &group.seats {
                let Some(session) = seat.transcripts.last() else {
                    continue;
                };
                let mut turns = None;
                for provider in [Provider::ClaudeCode, Provider::Codex] {
                    let view = ctx.with_provider(provider);
                    if let Ok(Some(path)) = payload::locate(&view, session)
                        && let Ok(Some(bytes)) = payload::read_checked(&view, &path, session)
                        && let Ok(found) = hook::typed_turns(&view, session, &bytes)
                    {
                        turns = Some(found);
                        break;
                    }
                }
                let unavailable = turns.is_none();
                let turns = turns.unwrap_or_default();
                report.members.push(crate::output::GroupBackfillMember {
                    seat: seat.name.clone(),
                    session: session.to_string(),
                    stored: 0,
                    skipped: 0,
                    transcript_unavailable: unavailable,
                });
                batches.push((session.clone(), turns));
            }
            let counts =
                store.backfill_group_turns(&group.id, &batches, &ctx.redactor(&ctx.config()?)?)?;
            for ((member, (_, turns)), stored) in
                report.members.iter_mut().zip(&batches).zip(counts)
            {
                member.stored = stored;
                member.skipped = turns.len() - stored;
                report.stored += stored;
                report.skipped += member.skipped;
            }
            return Ok(Output::GroupBackfill(report));
        }
        GroupCommand::Leave => (store.leave_group(&caller(ctx)?)?, true),
        GroupCommand::Close { id, quote } => {
            let session = caller(ctx)?;
            ctx.note_instruction_session(session.as_str());
            let now = ctx.now();
            let started = std::time::Instant::now();
            let config = ctx.config()?;
            let models = ctx.models(&config);
            // Resolve the alias once: consent, close and audit all bind to this
            // canonical id, so a slug reused after a close cannot redirect them.
            let group = store.group(&id)?;
            let consent = match quote {
                None => None,
                Some(quote) if quote.trim().is_empty() => {
                    return Err(scribe_core::Error::AgentWrite {
                        kind: WriteRefusal::QuoteRequired,
                        message: "pass --quote with the user's own words from this session".into(),
                    }
                    .into());
                }
                Some(quote) => {
                    let bytes = checked_transcript(ctx, &session)?.ok_or_else(|| {
                        scribe_core::Error::AgentWrite {
                            kind: WriteRefusal::QuoteNotFound,
                            message: format!(
                                "cannot verify a typed quote in session {session} on this host"
                            ),
                        }
                    })?;
                    let consent = GroupConsent::verify_close(
                        session.clone(),
                        ctx.provider(),
                        &bytes,
                        &quote,
                        &group.id,
                    )?;
                    ctx.note_instruction_message(consent.message());
                    Some(consent)
                }
            };
            let closer = consent
                .as_ref()
                .map_or(GroupCloser::Session(&session), GroupCloser::Quoted);
            let outcome = store.close_group_with_outcome(
                &group.id,
                closer,
                now,
                &ctx.redactor(&config)?,
                |evidence| -> Result<_, CliError> {
                    if checked_transcript(ctx, &session)?.is_none() {
                        return Err(scribe_core::Error::AgentWrite {
                            kind: WriteRefusal::GroupMembership,
                            message: format!(
                                "cannot verify the transcript of session {session} on this host"
                            ),
                        }
                        .into());
                    }
                    Ok(models.group_summary(&config.models.ask, evidence)?)
                },
                |group_id| scribe_core::AuditEvent {
                    at: now,
                    command: CLOSE_COMMAND.to_owned(),
                    status: scribe_core::AuditStatus::Success,
                    target: Some(group_id.to_owned()),
                    error_code: None,
                    author: None,
                    instruction_session: Some(session.as_str().to_owned()),
                    instruction_message: consent.as_ref().map(|c| c.message().to_owned()),
                    issue: None,
                    duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                },
            )?;
            if matches!(&outcome, CloseGroupOutcome::Closed { .. }) {
                ctx.note_audit_committed();
            }
            let (id, changed) = outcome.into_result()?;
            if !changed {
                notices.push("group is already closed".into());
            }
            (id, changed)
        }
        GroupCommand::Show { id } => {
            return Ok(Output::Show {
                record: Record::Group(Box::new(store.group(&id)?)),
            });
        }
        GroupCommand::List => {
            let groups = store.groups()?;
            let total = groups.len();
            return Ok(list_output(
                Records::Groups(groups),
                total,
                &[],
                &ListNotice {
                    noun: "groups",
                    criteria: Vec::new(),
                    empty: "no session groups in the store",
                },
            ));
        }
    };
    Ok(Output::Written {
        record: Record::Group(Box::new(store.group(&id)?)),
        changed,
        notices,
    })
}
