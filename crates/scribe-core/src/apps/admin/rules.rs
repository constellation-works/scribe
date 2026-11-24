//! The admin app's rules: the `store migrate` refusals (`migrate_blocked`), as pure functions over
//! facts the store has already read. The store reads each fact lazily and
//! calls its check at the point of its move where it always refused, in
//! this order: nothing to import, a session switched on, a rebuild
//! checkpoint waiting.

use crate::store::refusal;
use crate::{ControlState, Result, SessionId, WriteRefusal};
use std::path::Path;

/// Refuse a migrate with nothing to import: no leftover file-store item
/// and every archive already imported (`pending` is false).
///
/// # Errors
///
/// `migrate_blocked` when nothing is pending.
pub fn check_migrate_pending(pending: bool) -> Result<()> {
    if !pending {
        return Err(refusal(
            WriteRefusal::MigrateBlocked,
            "the store is already in scribe.db and no pre-sqlite-* or pre-issue-* archive is left to import; there is nothing to migrate",
        ));
    }
    Ok(())
}

/// Refuse a migrate while any session in `control` is switched on.
///
/// # Errors
///
/// `migrate_blocked` naming the first session switched on.
pub fn check_migrate_none_on(control: &ControlState) -> Result<()> {
    if let Some((id, _)) = control.sessions.iter().find(|(_, c)| c.on) {
        return Err(refusal(
            WriteRefusal::MigrateBlocked,
            format!(
                "session {id} is switched on; switch it off (/scribe off) or let its pass settle, then migrate"
            ),
        ));
    }
    Ok(())
}

/// Refuse a migrate while a rebuild checkpoint file waits: `first` is the
/// first one, by session, when there is any.
///
/// # Errors
///
/// `migrate_blocked` naming the session and the file.
pub fn check_migrate_no_checkpoint(first: Option<(&SessionId, &Path)>) -> Result<()> {
    if let Some((id, path)) = first {
        return Err(refusal(
            WriteRefusal::MigrateBlocked,
            format!(
                "session {id} has a rebuild checkpoint at {}; finish it with `scribe session rebuild {id}` or delete the file, then migrate",
                path.display()
            ),
        ));
    }
    Ok(())
}
