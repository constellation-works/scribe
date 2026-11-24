//! `/scribe on|off|status|link|catalog`, intercepted by
//! `scribe hook user-prompt-submit` (docs/design/scribe/2_design.md §5.1,
//! §5.4, §5.8).
//!
//! A successful `/scribe continue <handoff-id>` passes the handoff to the
//! agent as context. For the other commands, the hook returns
//! `decision: "block"` with the status as the `reason` Claude Code shows
//! the user. `on` records when and marks a pass due; `on` again is a no-op
//! that says so; `off` stops further passes and keeps the record; `status`
//! and any other argument change nothing. `/scribe on --from <id>` and
//! `/scribe link <id> [--type continues]` link this session to another one
//! (`spawned_from` by default), in one locked store call; an unknown id, a
//! self-link, a cycle, a second parent or (for `link`) a session that is not
//! cataloged is refused with its code and changes nothing.
//!
//! `/scribe catalog <id>` is the user's only way to catalog a past session by
//! name: it writes the catalog switch with `cataloged_by: {kind: daniel}`
//! (as `session catalog` does, with the same checks and the same
//! `session_cataloged` refusal) and runs no window, because a catalog can
//! take many model calls; `session catalog` or the next `session pass` runs
//! them and keeps the attribution. Agents' prompts never reach here
//! (`payload::agent_originated`), so no CLI path records the user.
//!
//! The payload is checked like a `#scribe` line's (an accident guard, not a
//! security boundary): a transcript outside Claude Code's projects
//! directory, or named for another session, switches nothing. A change to the
//! switch, a link or a catalog is audited (`target` `session:<id>`), and a
//! refusal is audited with its code; a no-op or a status writes nothing, except
//! that an explicit off during deferred ancestry records and audits the opt-out.

use super::payload::{self, PromptPayload};
use crate::app::Context;
use crate::output::{HookOutcome, HookReply, Output};
use scribe_core::SessionVerbs;
use scribe_core::control::{self, Change, Switch};
use scribe_core::{Author, SessionControl, SessionId, SessionLink, SessionRecord};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The hook event this replies to.
const EVENT: &str = "UserPromptSubmit";
/// The code of a payload whose transcript failed the provenance check.
const PROVENANCE_FAILED: &str = "provenance_failed";
/// The code of a `/scribe` argument that is not a command.
const UNKNOWN_COMMAND: &str = "unknown_scribe_command";
/// The commands, for a refusal.
const USAGE: &str = "use /scribe on, /scribe off, /scribe status, /scribe on --from <id>, /scribe link <id> [--type spawned_from|continues], /scribe catalog <id>, /scribe group <slug>, /scribe group add <slug> <session-id>..., /scribe group move <session-id> <slug>, /scribe group remove <session-id>, /scribe session update <session-id> project|title <value> or /scribe continue <handoff-id>";

/// Apply `switch` for the payload's session and block the prompt with the
/// result.
pub(super) fn run(
    switch: &Switch,
    payload: &PromptPayload,
    session: &SessionId,
    ctx: &Context,
) -> Output {
    let target = || Some(format!("session:{session}"));
    if let Err(reason) = payload::check(ctx, &payload.transcript_path, session) {
        return blocked(
            format!(
                "scribe: {} not applied — the hook payload failed the transcript check: {reason}",
                typed(switch)
            ),
            vec![HookOutcome {
                target: target(),
                error_code: Some(PROVENANCE_FAILED),
            }],
        );
    }
    let store = ctx.store();
    if let Switch::Link { link, on } = switch {
        return run_link(switch, link, *on, session, ctx);
    }
    if let Switch::Catalog(target) = switch {
        return run_catalog(switch, target, ctx);
    }
    if let Switch::Continue(id) = switch {
        if ctx.provider() != scribe_core::Provider::ClaudeCode {
            return blocked(
                "scribe: handoff continuation requires Claude Code".to_owned(),
                vec![HookOutcome {
                    target: target(),
                    error_code: Some("handoff_session"),
                }],
            );
        }
        return Output::Hook(super::continuation::restore(
            ctx,
            EVENT,
            session,
            scribe_core::HandoffSelection::Named(id),
        ));
    }
    let change = match switch {
        Switch::Link { .. }
        | Switch::Catalog(_)
        | Switch::Continue(_)
        | Switch::Group(_)
        | Switch::GroupAdd { .. }
        | Switch::GroupMove { .. }
        | Switch::GroupRemove(_)
        | Switch::SessionUpdate { .. }
        | Switch::Status => store
            .control()
            .map(|state| (Change::Status(state.get(session).cloned()), false)),
        Switch::Unknown(arg) => Ok((Change::Unknown(arg.clone()), false)),
        Switch::On | Switch::Off => store.has_deferred_ancestry(session).and_then(|pending| {
            store.update_control(|state| {
                let change =
                    control::switch_for_provider(state, session, switch, ctx.now(), ctx.provider());
                // An explicit opt-out must survive a later deferred inheritance.
                let recorded_off = pending
                    && matches!(switch, Switch::Off)
                    && !state.sessions.contains_key(session);
                if recorded_off {
                    state.sessions.insert(
                        session.clone(),
                        SessionControl {
                            provider: ctx.provider(),
                            ..SessionControl::default()
                        },
                    );
                }
                (change, recorded_off)
            })
        }),
    };
    let (change, recorded_off) = match change {
        Ok(change) => change,
        Err(err) => {
            let outcomes = match switch {
                Switch::On | Switch::Off => vec![HookOutcome {
                    target: target(),
                    error_code: Some(err.code()),
                }],
                Switch::Link { .. }
                | Switch::Catalog(_)
                | Switch::Continue(_)
                | Switch::Group(_)
                | Switch::GroupAdd { .. }
                | Switch::GroupMove { .. }
                | Switch::GroupRemove(_)
                | Switch::SessionUpdate { .. }
                | Switch::Status
                | Switch::Unknown(_) => Vec::new(),
            };
            return blocked(
                format!("scribe: {} failed — {}", typed(switch), chain(&err)),
                outcomes,
            );
        }
    };
    let outcomes = match &change {
        Change::SwitchedOn | Change::SwitchedOff => vec![HookOutcome {
            target: target(),
            error_code: None,
        }],
        Change::Unknown(_) => vec![HookOutcome {
            target: target(),
            error_code: Some(UNKNOWN_COMMAND),
        }],
        Change::AlreadyOff if recorded_off => vec![HookOutcome {
            target: target(),
            error_code: None,
        }],
        Change::AlreadyOn(_) | Change::AlreadyOff | Change::Status(_) => Vec::new(),
    };
    let record = match &change {
        Change::Status(_) => record_of(ctx, session),
        Change::SwitchedOn
        | Change::AlreadyOn(_)
        | Change::SwitchedOff
        | Change::AlreadyOff
        | Change::Unknown(_) => None,
    };
    blocked(status(&change, record.as_ref()), outcomes)
}

/// The command as the user typed it, for a reply.
fn typed(switch: &Switch) -> String {
    match switch {
        Switch::On => String::from("/scribe on"),
        Switch::Off => String::from("/scribe off"),
        Switch::Status => String::from("/scribe status"),
        Switch::Link { link, on: true } => {
            format!("/scribe on --from {} --type {}", link.session, link.kind)
        }
        Switch::Link { link, on: false } => {
            format!("/scribe link {} --type {}", link.session, link.kind)
        }
        Switch::Group(slug) => format!("/scribe group {slug}"),
        Switch::GroupAdd { slug, sessions } => {
            let ids: Vec<_> = sessions.iter().map(SessionId::as_str).collect();
            format!("/scribe group add {slug} {}", ids.join(" "))
        }
        Switch::GroupMove { session, slug } => format!("/scribe group move {session} {slug}"),
        Switch::GroupRemove(id) => format!("/scribe group remove {id}"),
        Switch::SessionUpdate {
            session,
            field,
            value,
        } => format!("/scribe session update {session} {field} {value}"),
        Switch::Continue(id) => format!("/scribe continue {id}"),
        Switch::Catalog(id) => format!("/scribe catalog {id}"),
        Switch::Unknown(arg) => format!("/scribe {arg}"),
    }
}

/// `/scribe catalog <id>`: start the user's catalog of a past session, or say
/// that one is already under way. Audited as a write when the switch was
/// written and as a refusal with its code; a resume writes nothing.
fn run_catalog(switch: &Switch, target: &SessionId, ctx: &Context) -> Output {
    match crate::commands::session::catalog::begin(ctx, target, Author::user()) {
        Ok(begun) => {
            let id = &begun.id;
            if begun.wrote_switch {
                blocked(
                    format!(
                        "scribe: cataloging {id} as yours — its windows run at the next session pass, or now with `scribe session catalog {id} --author <name>` in a shell; the record keeps cataloged_by user"
                    ),
                    vec![HookOutcome {
                        target: Some(format!("session:{id}")),
                        error_code: None,
                    }],
                )
            } else {
                blocked(
                    format!(
                        "scribe: {id} is already being cataloged — nothing changed; its windows run at the next session pass, or now with `scribe session catalog {id} --author <name>` in a shell"
                    ),
                    Vec::new(),
                )
            }
        }
        Err(err) => blocked(
            format!("scribe: {} refused — {err}", typed(switch)),
            vec![HookOutcome {
                target: Some(format!("session:{target}")),
                error_code: Some(err.code()),
            }],
        ),
    }
}

/// `/scribe on --from <id>` or `/scribe link <id>`: link the session (and
/// switch it on for the first), audited as a write when anything changed
/// and as a refusal with the link's code.
fn run_link(
    switch: &Switch,
    link: &SessionLink,
    on: bool,
    session: &SessionId,
    ctx: &Context,
) -> Output {
    let target = || Some(format!("session:{session}"));
    // A unique prefix of the target is stored, and reported, as the full id.
    let resolved = match crate::commands::resolve_session_or_typed(ctx, &link.session) {
        Ok(id) => id,
        Err(err) => {
            return blocked(
                format!("scribe: {} refused — {}", typed(switch), chain(&err)),
                vec![HookOutcome {
                    target: target(),
                    error_code: Some(err.code()),
                }],
            );
        }
    };
    let link = &SessionLink {
        session: resolved,
        kind: link.kind,
    };
    let relation = format!("{} {}", link.kind, link.session);
    match ctx
        .store()
        .link_session_for_provider(session, link, on, ctx.now(), ctx.provider())
    {
        Ok(linked) => {
            let outcomes = if linked.added || linked.switched_on {
                vec![HookOutcome {
                    target: target(),
                    error_code: None,
                }]
            } else {
                Vec::new()
            };
            let reply = match (linked.switched_on, linked.added) {
                (true, true) => format!(
                    "scribe: on — this session is cataloged from now on and linked: {relation}; its record is written at the next session pass (/scribe off stops it)"
                ),
                (true, false) => format!(
                    "scribe: on — this session is cataloged from now on; it was already linked: {relation}"
                ),
                (false, true) => format!("scribe: linked — this session is {relation}"),
                (false, false) => {
                    format!("scribe: already linked: {relation} — nothing changed")
                }
            };
            blocked(reply, outcomes)
        }
        Err(err) => blocked(
            format!("scribe: {} refused — {}", typed(switch), chain(&err)),
            vec![HookOutcome {
                target: target(),
                error_code: Some(err.code()),
            }],
        ),
    }
}

/// The reply for a change: what is true now.
fn status(change: &Change, record: Option<&SessionRecord>) -> String {
    match change {
        Change::SwitchedOn => String::from(
            "scribe: on — this session is cataloged from now on; its record is written at the next session pass (/scribe off stops it)",
        ),
        Change::AlreadyOn(control) => format!(
            "scribe: already on since {} — nothing changed",
            since(control)
        ),
        Change::SwitchedOff => {
            String::from("scribe: off — no further passes for this session; its record is kept")
        }
        Change::AlreadyOff => String::from("scribe: this session is not on — nothing changed"),
        Change::Status(None) => String::from(
            "scribe: off — this session was never switched on (/scribe on starts cataloging it)",
        ),
        Change::Status(Some(control)) => {
            let switch = if control.on {
                format!("on since {}", since(control))
            } else {
                String::from("off")
            };
            let due = match (control.due, control.final_pass) {
                (true, true) => "; the final pass is due",
                (true, false) => "; a pass is due",
                (false, _) => "",
            };
            let mut links: Vec<String> = record
                .map(|r| {
                    r.links
                        .iter()
                        .map(|l| format!("{} {}", l.kind, l.session))
                        .collect()
                })
                .unwrap_or_default();
            links.extend(
                control
                    .pending_links
                    .iter()
                    .map(|l| format!("{} {}", l.kind, l.session)),
            );
            let links = if links.is_empty() {
                String::new()
            } else {
                format!("; linked: {}", links.join(", "))
            };
            let record = record.map_or_else(
                || String::from("no record yet"),
                |r| {
                    format!(
                        "record {} with {} {}, last written {}",
                        r.status.as_str(),
                        r.passes,
                        if r.passes == 1 { "pass" } else { "passes" },
                        timestamp(r.updated)
                    )
                },
            );
            format!("scribe: {switch}{due}; {record}{links}")
        }
        Change::Unknown(arg) => format!("scribe: unknown command \"/scribe {arg}\" — {USAGE}"),
    }
}

/// The session's record, if it has one that reads; a status is still
/// given without it.
fn record_of(ctx: &Context, session: &SessionId) -> Option<SessionRecord> {
    match ctx.store().session(session) {
        Ok(view) => Some(view.record),
        Err(scribe_core::Error::SessionNotFound { .. }) => None,
        Err(err) => {
            tracing::warn!(%session, %err, "cannot read the session record for /scribe status");
            None
        }
    }
}

fn since(control: &SessionControl) -> String {
    control
        .since
        .map_or_else(|| String::from("an unknown time"), timestamp)
}

fn timestamp(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_else(|_| at.to_string())
}

/// An error and its causes on one line.
fn chain(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// Block the prompt with `reason`.
fn blocked(reason: String, outcomes: Vec<HookOutcome>) -> Output {
    Output::Hook(HookReply {
        event: EVENT,
        message: None,
        context: None,
        block: Some(reason),
        outcomes,
    })
}
