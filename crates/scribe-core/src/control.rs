//! `/scribe on|off|status` and the switch transitions behind it
//! (docs/design/scribe/2_design.md §5.1, §5.2).
//!
//! A prompt whose first line is `/scribe on`, `/scribe off` or
//! `/scribe status` is a control command, not a turn: the `UserPromptSubmit`
//! hook applies it to `control.json` and blocks the prompt. This module
//! recognises the line ([`parse`]) and changes a [`ControlState`] in memory
//! ([`switch`], [`mark_due`], [`mark_final`], [`settle`]); the store persists
//! the result under its lock and the hook words the reply.
//!
//! Every session starts off and has no switch at all until Daniel turns it
//! on, so a session never switched on is never mentioned in `control.json`
//! and a pass never reads its transcript. Pure values, no I/O (STD-02 R5).

use crate::session::{Cadence, ControlState, SessionControl, SessionId};
use time::OffsetDateTime;

/// The command word, as typed.
const COMMAND: &str = "/scribe";
/// The same command through Claude Code's plugin namespace
/// (`/<plugin>:<command>`).
const NAMESPACED: &str = "/scribe:scribe";

/// What a `/scribe` line asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Switch {
    /// Start cataloging this session.
    On,
    /// Stop cataloging this session; its record is kept.
    Off,
    /// Report the switch without changing it (also a bare `/scribe`).
    Status,
    /// Any other argument: refused with the usage.
    Unknown(String),
}

/// The control command on the prompt's first line, if there is one. The
/// word is matched exactly (`/scribe`, or `/scribe:scribe` as Claude Code
/// names a plugin command); the argument case-insensitively. Lines after
/// the first are ignored, as Claude Code does for a slash command.
#[must_use]
pub fn parse(prompt: &str) -> Option<Switch> {
    let first = prompt.lines().next()?.trim();
    let mut words = first.split_whitespace();
    let word = words.next()?;
    if word != COMMAND && word != NAMESPACED {
        return None;
    }
    let rest: Vec<&str> = words.collect();
    Some(match rest.as_slice() {
        [] => Switch::Status,
        [arg] if arg.eq_ignore_ascii_case("on") => Switch::On,
        [arg] if arg.eq_ignore_ascii_case("off") => Switch::Off,
        [arg] if arg.eq_ignore_ascii_case("status") => Switch::Status,
        other => Switch::Unknown(other.join(" ")),
    })
}

/// What [`switch`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The session was off (or never on) and is now on, with a pass due.
    SwitchedOn,
    /// The session was already on: nothing changed.
    AlreadyOn(SessionControl),
    /// The session was on and is now off; no further pass runs.
    SwitchedOff,
    /// The session was not on: nothing changed.
    AlreadyOff,
    /// A status request: nothing changed.
    Status(Option<SessionControl>),
    /// An argument that is not `on`, `off` or `status`: nothing changed.
    Unknown(String),
}

impl Change {
    /// Whether the state was changed and must be written.
    #[must_use]
    pub fn changed(&self) -> bool {
        match self {
            Self::SwitchedOn | Self::SwitchedOff => true,
            Self::AlreadyOn(_) | Self::AlreadyOff | Self::Status(_) | Self::Unknown(_) => false,
        }
    }
}

/// Apply `request` to `session`'s switch at `now`.
///
/// `on` records `since` and marks a pass due, so the first record is written
/// at the next pass; `on` again is a no-op. `off` clears `due` and `final`
/// too, so no pass runs for it any more; the record stays. `off`, `status`
/// and an unknown argument never create a switch for a session that has
/// none.
pub fn switch(
    state: &mut ControlState,
    session: &SessionId,
    request: &Switch,
    now: OffsetDateTime,
) -> Change {
    let current = state.sessions.get(session).cloned();
    match request {
        Switch::On => match current {
            Some(control) if control.on => Change::AlreadyOn(control),
            Some(_) | None => {
                state.sessions.insert(
                    session.clone(),
                    SessionControl {
                        on: true,
                        since: Some(now),
                        due: true,
                        final_pass: false,
                    },
                );
                Change::SwitchedOn
            }
        },
        Switch::Off => match state.sessions.get_mut(session) {
            Some(control) if control.on => {
                control.on = false;
                control.due = false;
                control.final_pass = false;
                Change::SwitchedOff
            }
            Some(_) | None => Change::AlreadyOff,
        },
        Switch::Status => Change::Status(current),
        Switch::Unknown(arg) => Change::Unknown(arg.clone()),
    }
}

/// When the `Stop` hook looks at a session: when its last pass ran (the
/// record's `updated`, else when it was switched on) and how many of
/// Daniel's turns arrived since.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Since {
    /// When the last pass ran.
    pub last_pass: OffsetDateTime,
    /// User turns since then.
    pub turns: u32,
}

/// Mark `session` due when it is on and the cadence says so (see
/// [`Cadence::pass_due`]). Returns whether the state changed: a session that
/// is off, unknown or already due is left as it is.
pub fn mark_due(
    state: &mut ControlState,
    session: &SessionId,
    cadence: &Cadence,
    since: Since,
    now: OffsetDateTime,
) -> bool {
    let Some(control) = state.sessions.get_mut(session) else {
        return false;
    };
    if !control.on || control.due {
        return false;
    }
    if cadence.pass_due(control, since.last_pass, now, since.turns) {
        control.due = true;
        return true;
    }
    false
}

/// Whether the `Stop` hook needs [`Since`] for `session` at all: only a
/// session that is on and not yet due. Lets the hook skip reading the
/// record and the transcript for every other session.
#[must_use]
pub fn needs_cadence(state: &ControlState, session: &SessionId) -> bool {
    state.get(session).is_some_and(|c| c.on && !c.due)
}

/// The session ended: mark its last pass due and final. Only a session that
/// is on; returns whether the state changed.
pub fn mark_final(state: &mut ControlState, session: &SessionId) -> bool {
    let Some(control) = state.sessions.get_mut(session) else {
        return false;
    };
    if !control.on || (control.due && control.final_pass) {
        return false;
    }
    control.due = true;
    control.final_pass = true;
    true
}

/// A pass for `session` is settled: it ran (`ran_final` says whether it was
/// the final one), or it failed too often to retry. `due` is cleared, unless
/// the session ended while a non-final pass was running, in which case the
/// final pass is still owed. Returns whether the state changed.
pub fn settle(state: &mut ControlState, session: &SessionId, ran_final: bool) -> bool {
    let Some(control) = state.sessions.get_mut(session) else {
        return false;
    };
    let before = control.clone();
    if ran_final || !control.final_pass {
        control.due = false;
        control.final_pass = false;
    }
    *control != before
}

/// The sessions a pass should run for, oldest id first: on and due. A
/// session that is off is never passed, whatever its flags say.
#[must_use]
pub fn due(state: &ControlState) -> Vec<(SessionId, SessionControl)> {
    state
        .sessions
        .iter()
        .filter(|(_, c)| c.on && c.due)
        .map(|(id, c)| (id.clone(), c.clone()))
        .collect()
}

/// The sessions `ids` names, for a pass Daniel asks for by id, oldest id
/// first and each once, whether or not `due` is set: a pass whose `due` was
/// cleared can be run again. A session that is off or has no switch is
/// never passed, so the first such id is the `Err`.
///
/// # Errors
///
/// The first id (in id order) whose session is not on.
pub fn chosen(
    state: &ControlState,
    ids: &[SessionId],
) -> Result<Vec<(SessionId, SessionControl)>, SessionId> {
    let ids: std::collections::BTreeSet<&SessionId> = ids.iter().collect();
    ids.into_iter()
        .map(|id| match state.sessions.get(id) {
            Some(control) if control.on => Ok((id.clone(), control.clone())),
            Some(_) | None => Err(id.clone()),
        })
        .collect()
}
