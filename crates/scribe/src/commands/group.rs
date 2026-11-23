//! Quote-checked group opening, caller-bound seat membership and roster reads.

use super::hook::payload;
use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{Output, Record, Records};
use clap::Subcommand;
use scribe_core::group::{GroupConsent, JoinGroup, NewGroup};
use scribe_core::{
    AuthorName, Label, Project, Provider, Related, SessionId, WriteRefusal, transcript,
};

pub(super) const EXAMPLES: &str = "Examples:
  scribe group open --title 'Release review' --kind review --project scribe --quote 'open a release review'
  scribe group join <id> --as editor --author aide --role reviewer
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
        /// Group id or unique prefix
        id: String,
        /// Seat name within this group
        #[arg(long = "as", value_name = "SEAT", value_parser = seat_name)]
        seat: Label,
        /// Your agent author name
        #[arg(long)]
        author: AuthorName,
        /// Free role label (defaults to the seat name for a new seat)
        #[arg(long)]
        role: Option<String>,
        /// Explicitly replace the newest holder of an occupied seat
        #[arg(long)]
        replace: bool,
    },
    /// Remove the calling session from its seat
    #[command(after_help = "Example: scribe group leave")]
    Leave,
    /// Show one group, its references and its seat chains
    #[command(after_help = "Example: scribe group show <id> --json")]
    Show {
        /// Group id or unique prefix
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
            Self::List | Self::Show { .. } => return None,
        };
        Some(AuditMeta {
            command,
            stepwise: false,
        })
    }

    pub(super) fn subject(&self, ctx: &Context) -> AuditSubject {
        match self {
            Self::Join { id, author, .. } => AuditSubject {
                author: Some(author.to_string()),
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
                related,
            };
            (
                store.open_group(draft, &consent, ctx.now(), &ctx.redactor(&ctx.config()?)?)?,
                true,
            )
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
            let latest = group
                .seats
                .iter()
                .find(|s| s.name == seat.as_str())
                .and_then(|s| s.transcripts.last());
            let ended = match latest {
                Some(previous) if !replace && previous != &caller && ended(ctx, previous)? => {
                    Some(previous)
                }
                _ => None,
            };
            let join = JoinGroup {
                group: &group.id,
                session: &caller,
                seat: &seat,
                author: &author,
                role: role.as_deref(),
                replace,
                ended,
            };
            let changed = store.join_group(join, &ctx.redactor(&ctx.config()?)?)?;
            (group.id, changed)
        }
        GroupCommand::Leave => (store.leave_group(&caller(ctx)?)?, true),
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
        notices: if changed {
            Vec::new()
        } else {
            vec!["session already holds this seat".into()]
        },
    })
}
