//! `/scribe on|off|status|link|catalog` and the switch transitions behind it
//! (docs/design/scribe/2_design.md §5.1, §5.2, §5.4, §5.8).
//!
//! A prompt whose first line is `/scribe on`, `/scribe off`,
//! `/scribe status`, `/scribe on --from <id>`, `/scribe link <id>` or
//! `/scribe catalog <id>` is a control command, not a turn: the `UserPromptSubmit`
//! hook applies it to `control.json` and blocks the prompt. This module
//! recognises the line ([`parse`]) and changes a [`ControlState`] in memory
//! ([`switch`], [`mark_due`], [`mark_final`], [`settle`]); the store persists
//! the result under its lock and the hook words the reply.
//!
//! Every session starts off. Restoring a handoff can leave an off switch
//! holding a pending link. Only an on switch schedules a pass.

use crate::activity::CatalogedBy;
use crate::record::Author;
use crate::session::{
    Cadence, ControlState, LinkError, LinkType, SessionControl, SessionId, SessionLink, check_link,
};
use std::collections::{BTreeMap, BTreeSet};
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
    /// Link this session to another (`/scribe link <id> [--type <type>]`);
    /// with `on`, also switch it on (`/scribe on --from <id>`).
    Link {
        /// The link to add to this session.
        link: SessionLink,
        /// Whether to switch the session on as well.
        on: bool,
    },
    /// Catalog a past session by name as Daniel (`/scribe catalog <id>`):
    /// the id, or a unique prefix of it, of the session to catalog.
    Catalog(SessionId),
    /// Resume from an active continue handoff by entry id or unique prefix.
    Continue(String),
    /// Join a project group on a typed command.
    Group(String),
    /// Add past sessions to a project group on a typed command
    /// (`/scribe group add <slug> <session-id>...`), creating the group when
    /// absent. The session it is typed in is not enrolled unless named.
    GroupAdd {
        /// The group's slug, as typed; the store checks it.
        slug: String,
        /// The named sessions, each an id or a prefix of one, at least one.
        sessions: Vec<SessionId>,
    },
    /// Move a named session to a project group.
    GroupMove {
        /// The session to move.
        session: SessionId,
        /// The target slug.
        slug: String,
    },
    /// Remove a named session from its group.
    GroupRemove(SessionId),
    /// Update a recorded session field.
    SessionUpdate {
        /// The record to update.
        session: SessionId,
        /// The field name.
        field: String,
        /// Its new value.
        value: String,
    },
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
    let words: Vec<&str> = words.collect();
    Some(match words.as_slice() {
        [] => Switch::Status,
        [arg] if arg.eq_ignore_ascii_case("on") => Switch::On,
        [arg] if arg.eq_ignore_ascii_case("off") => Switch::Off,
        [arg] if arg.eq_ignore_ascii_case("status") => Switch::Status,
        [arg, action, id, slug]
            if arg.eq_ignore_ascii_case("group") && action.eq_ignore_ascii_case("move") =>
        {
            id.parse().map_or_else(
                |_| Switch::Unknown(words.join(" ")),
                |session| Switch::GroupMove {
                    session,
                    slug: (*slug).to_owned(),
                },
            )
        }
        [arg, action, id]
            if arg.eq_ignore_ascii_case("group") && action.eq_ignore_ascii_case("remove") =>
        {
            id.parse()
                .map_or_else(|_| Switch::Unknown(words.join(" ")), Switch::GroupRemove)
        }
        [arg, action, id, field, value @ ..]
            if arg.eq_ignore_ascii_case("session")
                && action.eq_ignore_ascii_case("update")
                && (field.eq_ignore_ascii_case("title")
                    || field.eq_ignore_ascii_case("project"))
                && !value.is_empty()
                && (!field.eq_ignore_ascii_case("project") || value.len() == 1) =>
        {
            id.parse().map_or_else(
                |_| Switch::Unknown(words.join(" ")),
                |session| Switch::SessionUpdate {
                    session,
                    field: field.to_ascii_lowercase(),
                    value: value.join(" "),
                },
            )
        }
        [arg, slug] if arg.eq_ignore_ascii_case("group") => Switch::Group((*slug).to_owned()),
        [arg, add, slug, ids @ ..]
            if arg.eq_ignore_ascii_case("group") && add.eq_ignore_ascii_case("add") =>
        {
            ids.iter()
                .map(|id| id.parse::<SessionId>())
                .collect::<Result<Vec<_>, _>>()
                .ok()
                .filter(|sessions| !sessions.is_empty())
                .map_or_else(
                    || Switch::Unknown(words.join(" ")),
                    |sessions| Switch::GroupAdd {
                        slug: (*slug).to_owned(),
                        sessions,
                    },
                )
        }
        [arg, id] if arg.eq_ignore_ascii_case("continue") => Switch::Continue((*id).to_owned()),
        [arg, id] if arg.eq_ignore_ascii_case("catalog") => id
            .parse()
            .map_or_else(|_| Switch::Unknown(words.join(" ")), Switch::Catalog),
        [arg, rest @ ..] if arg.eq_ignore_ascii_case("on") || arg.eq_ignore_ascii_case("link") => {
            link_args(rest, arg.eq_ignore_ascii_case("on"))
                .unwrap_or_else(|| Switch::Unknown(words.join(" ")))
        }
        other => Switch::Unknown(other.join(" ")),
    })
}

/// Normalize only exact typed skill-control invocations. Injected skill text
/// never reaches this function: host content provenance is checked first.
#[must_use]
pub fn command_text(original: &str) -> String {
    let text = original.trim();
    let args = text.strip_prefix("$scribe ").or_else(|| {
        let (label, rest) = text.split_once("](")?;
        if !matches!(label, "[$scribe]" | "[$scribe:scribe]")
            && !matches!(label, "[$scribe" | "[$scribe:scribe")
        {
            return None;
        }
        let (path, args) = rest.split_once(") ")?;
        if !path.ends_with("/scribe/SKILL.md") {
            return None;
        }
        Some(args)
    });
    if let Some(args) = args.filter(|s| !s.contains('\n')) {
        let candidate = format!("/scribe {args}");
        if matches!(
            parse(&candidate),
            Some(
                Switch::On
                    | Switch::Off
                    | Switch::Status
                    | Switch::Link { .. }
                    | Switch::Catalog(_)
                    | Switch::Group(_)
                    | Switch::GroupAdd { .. }
                    | Switch::GroupMove { .. }
                    | Switch::GroupRemove(_)
                    | Switch::SessionUpdate { .. }
            )
        ) {
            return candidate;
        }
    }
    original.to_owned()
}

/// Whether a typed Codex input is a session control, `/scribe …` or its
/// `$scribe …` skill form.
#[must_use]
pub fn is_control(text: &str) -> bool {
    parse(&command_text(text)).is_some()
}

/// The words after `on` (`--from <id> [--type <type>]`) or after `link`
/// (`<id> [--type <type>]`), in any order; `None` when they do not parse.
/// The type defaults to `spawned_from`.
fn link_args(words: &[&str], on: bool) -> Option<Switch> {
    let mut target: Option<SessionId> = None;
    let mut kind: Option<LinkType> = None;
    let mut it = words.iter();
    while let Some(word) = it.next() {
        if word.eq_ignore_ascii_case("--type") {
            if kind.is_some() {
                return None;
            }
            kind = Some(it.next()?.parse().ok()?);
        } else if on && word.eq_ignore_ascii_case("--from") {
            if target.is_some() {
                return None;
            }
            target = Some(it.next()?.parse().ok()?);
        } else if !on && target.is_none() && !word.starts_with('-') {
            target = Some(word.parse().ok()?);
        } else {
            return None;
        }
    }
    Some(Switch::Link {
        link: SessionLink {
            kind: kind.unwrap_or(LinkType::SpawnedFrom),
            session: target?,
        },
        on,
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

/// What [`link`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Linked {
    /// Whether the link is new; `false` when the session already had it.
    pub added: bool,
    /// Whether `/scribe on --from` switched the session on too.
    pub switched_on: bool,
    /// Whether the caller must add the link to the session's existing
    /// record. When the session has no record yet, a new link goes to its
    /// switch's `pending_links` in `state` instead, and this is `false`.
    pub write_record: bool,
}

/// Add `link` to `session` at `now`, switching it on first when `on`.
/// `records` holds every session record's links, by id (a session with a
/// record has a key, even with no links). The target must be known (a
/// record or a switch), not `session` itself, and not lead back to it
/// ([`check_link`]); a session that is neither recorded nor switched on is
/// refused unless `on`. Nothing in `state` changes when it is refused.
///
/// # Errors
///
/// The [`LinkError`] that refused it.
pub fn link(
    state: &mut ControlState,
    records: &BTreeMap<SessionId, Vec<SessionLink>>,
    session: &SessionId,
    link: &SessionLink,
    on: bool,
    now: OffsetDateTime,
) -> Result<Linked, LinkError> {
    apply_link(
        state,
        records,
        session,
        link,
        LinkSwitch::Manual { on },
        now,
    )
}

/// How to set the child's switch when adding a session link.
#[derive(Clone, Copy)]
pub enum LinkSwitch {
    /// Set the switch as the link says.
    Manual {
        /// Whether the child is switched on.
        on: bool,
    },
    /// The verified handoff identifies a parent even if it was never on.
    Inherit,
}

/// Apply `link` from `session` to the switch state, as [`LinkSwitch`] says.
pub fn apply_link(
    state: &mut ControlState,
    records: &BTreeMap<SessionId, Vec<SessionLink>>,
    session: &SessionId,
    link: &SessionLink,
    mode: LinkSwitch,
    now: OffsetDateTime,
) -> Result<Linked, LinkError> {
    let has_record = records.contains_key(session);
    if matches!(mode, LinkSwitch::Manual { on: false })
        && !has_record
        && !state.sessions.contains_key(session)
    {
        return Err(LinkError::NotCataloged);
    }
    let mut known: BTreeSet<SessionId> = records.keys().cloned().collect();
    known.extend(state.sessions.keys().cloned());
    if matches!(mode, LinkSwitch::Inherit) {
        known.insert(link.session.clone());
    }
    let mut graph = records.clone();
    for (id, control) in &state.sessions {
        graph
            .entry(id.clone())
            .or_default()
            .extend(control.pending_links.iter().cloned());
    }
    let added = check_link(session, link, &known, &graph)?;
    let switched_on = match mode {
        LinkSwitch::Manual { on } => {
            on && switch(state, session, &Switch::On, now) == Change::SwitchedOn
        }
        LinkSwitch::Inherit if !added || state.sessions.contains_key(session) => false, // Preserve an explicit or later child switch.
        LinkSwitch::Inherit if state.get(&link.session).is_some_and(|s| s.on) => {
            switch(state, session, &Switch::On, now) == Change::SwitchedOn
        }
        LinkSwitch::Inherit => {
            switch(state, session, &Switch::Off, now);
            if !has_record {
                state.sessions.entry(session.clone()).or_default();
            }
            false
        }
    };
    let mut write_record = false;
    if added {
        if has_record {
            write_record = true;
        } else if let Some(control) = state.sessions.get_mut(session) {
            control.pending_links.push(link.clone());
        }
    }
    Ok(Linked {
        added,
        switched_on,
        write_record,
    })
}

/// Whether `session catalog` may start or resume `session`.
///
/// A switch `catalog` wrote (`cataloged_by` set) that is still on and due
/// resumes, even when a partial record exists. Any other switch, or a
/// record with no such switch, is refused (`session_cataloged`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogGate {
    /// No record and no switch: write one and catalog.
    Start,
    /// A catalog switch is still due. Keep `since` and `cataloged_by`.
    Resume(SessionControl),
    /// A record or a non-catalog switch already exists.
    Refused,
}

/// Decide [`CatalogGate`] without changing `state`. `has_record` is true
/// when a session file for `session` exists, including one that did not parse.
#[must_use]
pub fn catalog_gate(state: &ControlState, session: &SessionId, has_record: bool) -> CatalogGate {
    if let Some(control) = state.get(session) {
        if control.cataloged_by.is_some() && control.on && control.due {
            return CatalogGate::Resume(control.clone());
        }
        return CatalogGate::Refused;
    }
    if has_record {
        CatalogGate::Refused
    } else {
        CatalogGate::Start
    }
}

/// Switch `session` on as a cataloged-by-name session. The caller has
/// already decided [`CatalogGate::Start`]. `since` and `cataloged_by.at`
/// are both `now`.
pub fn catalog_on(state: &mut ControlState, session: &SessionId, by: Author, now: OffsetDateTime) {
    state.sessions.insert(
        session.clone(),
        SessionControl {
            provider: crate::Provider::ClaudeCode,
            on: true,
            since: Some(now),
            due: true,
            final_pass: false,
            consecutive_failures: 0,
            retry_after: None,
            pending_links: Vec::new(),
            cataloged_by: Some(CatalogedBy { at: now, by }),
        },
    );
}

/// Switch and stamp provider in the same control-state transaction.
pub fn switch_for_provider(
    state: &mut ControlState,
    session: &SessionId,
    request: &Switch,
    now: OffsetDateTime,
    provider: crate::Provider,
) -> Change {
    let change = switch(state, session, request, now);
    if let Some(control) = state.sessions.get_mut(session) {
        control.provider = provider;
    }
    change
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
                let (pending_links, cataloged_by) = current
                    .map(|c| (c.pending_links, c.cataloged_by))
                    .unwrap_or_default();
                state.sessions.insert(
                    session.clone(),
                    SessionControl {
                        on: true,
                        since: Some(now),
                        due: true,
                        pending_links,
                        cataloged_by,
                        ..SessionControl::default()
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
                control.consecutive_failures = 0;
                control.retry_after = None;
                Change::SwitchedOff
            }
            Some(_) | None => Change::AlreadyOff,
        },
        Switch::Status => Change::Status(current),
        // A link is applied by the store's link call, and a catalog by
        // `catalog_gate` and `catalog_on`, not here.
        Switch::Link { .. }
        | Switch::Catalog(_)
        | Switch::Continue(_)
        | Switch::Group(_)
        | Switch::GroupAdd { .. }
        | Switch::GroupMove { .. }
        | Switch::GroupRemove(_)
        | Switch::SessionUpdate { .. } => Change::Status(current),
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
/// the final one). `due` is cleared, unless
/// the session ended while a non-final pass was running, in which case the
/// final pass is still owed. Returns whether the state changed.
pub fn settle(state: &mut ControlState, session: &SessionId, ran_final: bool) -> bool {
    let Some(control) = state.sessions.get_mut(session) else {
        return false;
    };
    let before = control.clone();
    control.consecutive_failures = 0;
    control.retry_after = None;
    if ran_final || !control.final_pass {
        control.due = false;
        control.final_pass = false;
    }
    *control != before
}

/// A failed pass stays due, with automatic retries after 5, 10, 20, 40,
/// then at most 60 minutes. Partial progress does not reset this counter.
pub fn fail(state: &mut ControlState, session: &SessionId, now: OffsetDateTime) {
    if let Some(control) = state.sessions.get_mut(session).filter(|c| c.on) {
        control.due = true;
        control.consecutive_failures = control.consecutive_failures.saturating_add(1);
        let shift = control.consecutive_failures.saturating_sub(1).min(4);
        let minutes = (5i64 << shift).min(60);
        control.retry_after = Some(now.saturating_add(time::Duration::minutes(minutes)));
    }
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
