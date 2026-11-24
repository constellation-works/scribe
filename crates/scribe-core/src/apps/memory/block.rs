//! The memory block (2_design.md §15.4), as `memory show`, `brief` and the
//! session-start brief read it (ORB-27707 step 3d). Read-only: no lock, no
//! write, no replay.

use crate::MemoryView;
use crate::memory;
use std::path::Path;

/// The warnings a read of `view` from the store at `root` carries.
#[must_use]
pub fn warnings(view: &MemoryView, root: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    if view.pending {
        warnings.push(String::from(
            "warning: memory_pending: a memory write was interrupted; the profile shown is its committed revision, and the next memory write completes it",
        ));
    }
    if view.read_only_copy {
        warnings.push(format!(
            "read-only copy: {} has no -wal or -shm file and its directory is read-only, so it was opened immutable",
            root.join(crate::DB_FILE).display()
        ));
    }
    for skipped in &view.skipped {
        warnings.push(format!("warning: skipped {}", skipped.error));
    }
    if view.unmigrated > 0 {
        warnings.push(format!(
            "warning: {} format-24 memory item(s) not read; the next store write archives them",
            view.unmigrated
        ));
    }
    warnings
}

/// The block's text.
#[must_use]
pub fn text(view: &MemoryView) -> String {
    memory::block(view.profile.as_ref(), &view.now, &view.milestones)
}

/// Whether the block holds nothing: no profile, no `now` item and no
/// milestone shown.
#[must_use]
pub fn is_empty(view: &MemoryView) -> bool {
    view.profile.is_none()
        && view.now.is_empty()
        && memory::newest_milestones(&view.milestones).is_empty()
}
