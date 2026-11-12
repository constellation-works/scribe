//! Semantic color (STD-01 R18): domain values map to a small closed set of
//! roles in one table, and roles map to the basic 16-color palette. The text
//! always prints; color only adds emphasis, so stripping it loses nothing.
//!
//! Only the roles this CLI uses exist. Add `Error` or `Active` when a value
//! needs one, not before.

/// What a cell's color means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// Needs attention.
    Warn,
    /// Healthy or current: an open session.
    Ok,
    /// De-emphasized.
    Muted,
    /// No emphasis; also the fallback for unmapped values.
    Neutral,
}

/// The one mapping from an entry status to a role: anything but `active`
/// is hidden from `ask`, so it is de-emphasized.
pub(crate) fn role_for_status(status: &str) -> Role {
    if status == "active" {
        Role::Neutral
    } else {
        Role::Muted
    }
}

/// Wrap `text` in the role's escape codes when color is allowed.
pub(crate) fn paint(text: &str, role: Role, color: bool) -> String {
    let code = match role {
        Role::Warn => "33",  // yellow
        Role::Ok => "32",    // green
        Role::Muted => "90", // bright black
        Role::Neutral => return text.to_owned(),
    };
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}
