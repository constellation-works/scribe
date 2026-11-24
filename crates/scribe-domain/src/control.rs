//! `/scribe on|off|status|link|catalog` and the switch transitions behind it
//! (docs/design/scribe/2_design.md §5.1, §5.2, §5.4, §5.8).
//!
//! A prompt whose first line is `/scribe on`, `/scribe off`,
//! `/scribe status`, `/scribe on --from <id>`, `/scribe link <id>` or
//! `/scribe catalog <id>` is a control command, not a turn: the `UserPromptSubmit`
//! hook applies it to `control.json` and blocks the prompt. This module
//! recognises the line ([`parse`]). The switch transitions that change a
//! `ControlState` in memory live in the session app's rules
//! (`scribe-core/src/apps/session/rules/control.rs`); the store persists the
//! result under its lock and the hook words the reply. This half stays in
//! the domain while the domain's `transcript` reads it (ORB-27921 PR 5-6).
//!
//! Every session starts off. Restoring a handoff can leave an off switch
//! holding a pending link. Only an on switch schedules a pass.

use crate::session::{LinkType, SessionId, SessionLink};

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
