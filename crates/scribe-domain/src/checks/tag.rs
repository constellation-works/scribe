//! The checks of a store tag write (`scribe tag add`, `scribe tag update`).

use super::refusal;
use crate::{Result, WriteRefusal};

/// Refuse a tag description that is blank or over 300 characters.
///
/// # Errors
///
/// [`WriteRefusal::InvalidText`] for either.
pub fn check_description(description: &str) -> Result<()> {
    if description.trim().is_empty() || description.chars().count() > 300 {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "description must contain 1-300 characters",
        ));
    }
    Ok(())
}
