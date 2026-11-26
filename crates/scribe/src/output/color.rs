//! Semantic color (STD-01 R18): a domain value maps to one of the kit's
//! roles in one table; the kit paints the role where color is on. The text
//! always prints; color only adds emphasis, so stripping it loses nothing.

use cw_cli::Role;

/// The one mapping from an entry status to a role: anything but `active`
/// is hidden from search, so it is de-emphasized.
pub(crate) fn role_for_status(status: &str) -> Role {
    if status == "active" {
        Role::Neutral
    } else {
        Role::Muted
    }
}
