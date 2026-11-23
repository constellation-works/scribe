//! Quote-checked group opening, caller-bound seat membership and roster reads.

use super::hook::{self, ClearedPredecessor, payload};
use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{Output, Record, Records};
use clap::Subcommand;
use scribe_core::group::{GroupConsent, JoinGroup, NewGroup, automatic_seat};
use scribe_core::{
    AuthorName, CloseGroupOutcome, Label, Models, Project, Provider, Related, SessionId,
    WriteRefusal, transcript,
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
    /// Remove the calling session from its seat
    #[command(after_help = "Example: scribe group leave")]
    Leave,
    /// Summarize and close the group from the session that opened it
    #[command(after_help = "Example: scribe group close <id>")]
    Close {
        /// Group id, unique prefix, or project group slug
        id: String,
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
            Self::Close { id } => AuditSubject {
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
/// can be from either provider in this host's store.
fn ended(ctx: &Context, session: &SessionId) -> Result<bool, CliError> {
    let mut found = false;
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let view = ctx.with_provider(provider);
        if let Some(path) = payload::locate(&view, session)? {
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
                related: store.resolve_relateds(&related)?,
            };
            let opened =
                store.open_group(draft, &consent, ctx.now(), &ctx.redactor(&ctx.config()?)?)?;
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
                        let path = payload::locate(ctx, &caller)?;
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
            let join = JoinGroup {
                group: &group.id,
                session: &caller,
                seat: &seat,
                author: &author,
                role: role.as_deref(),
                replace,
                ended,
                predecessor: predecessor.as_ref(),
            };
            let changed = store.join_group(join, &ctx.redactor(&ctx.config()?)?)?;
            if !changed {
                notices.push("session already holds this seat".into());
            }
            (group.id, changed)
        }
        GroupCommand::Leave => (store.leave_group(&caller(ctx)?)?, true),
        GroupCommand::Close { id } => {
            let session = caller(ctx)?;
            ctx.note_instruction_session(session.as_str());
            let now = ctx.now();
            let started = std::time::Instant::now();
            let config = ctx.config()?;
            let models = ctx.models(&config);
            let outcome = store.close_group_with_outcome(
                &id,
                &session,
                now,
                &ctx.redactor(&config)?,
                |evidence| -> Result<_, CliError> {
                    let path = payload::locate(ctx, &session)?;
                    if !path.as_deref().is_some_and(|path| {
                        payload::read_checked(ctx, path, &session)
                            .is_ok_and(|bytes| bytes.is_some())
                    }) {
                        return Err(scribe_core::Error::AgentWrite {
                            kind: WriteRefusal::GroupMembership,
                            message: "cannot verify the opening session's transcript on this host"
                                .into(),
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
                    instruction_message: None,
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
