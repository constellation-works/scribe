//! `scribe store migrate`'s refusals: the rule the store's import runs
//! inside its write, before anything is imported
//! ([`Store::import_file_store`]).

use super::rules as admin;
use crate::{ImportFacts, Result, Store};
use time::OffsetDateTime;

/// Refuse `store migrate` before anything is imported: when there is no
/// archive to import, while a session is on, or while a rebuild
/// checkpoint file waits in a leftover `sessions/`. Each fact is read
/// only once the check before it passed.
///
/// # Errors
///
/// `migrate_blocked`, and the store's read errors.
pub(crate) fn check_migrate(facts: &ImportFacts<'_>) -> Result<()> {
    admin::check_migrate_pending(facts.pending()?)?;
    admin::check_migrate_none_on(&facts.control()?)?;
    let checkpoints = facts.checkpoint_files()?;
    admin::check_migrate_no_checkpoint(checkpoints.first().map(|(id, path)| (id, path.as_path())))
}

/// `store migrate` at `now`: the import, refused first by [`check_migrate`]
/// inside its write.
///
/// # Errors
///
/// `migrate_blocked`; `store_corrupt` for a file store whose records never
/// moved; store failures.
pub(crate) fn migrate(store: &Store, now: OffsetDateTime) -> Result<crate::Imported> {
    store.import_file_store(now, check_migrate)
}

/// `store migrate` in the store tests' call syntax. Only for tests.
#[cfg(any(test, feature = "test-support"))]
pub trait MigrateTestWrites {
    /// `store migrate` at `now`.
    ///
    /// # Errors
    ///
    /// As [`migrate`].
    fn migrate_store(&self, now: OffsetDateTime) -> Result<crate::Imported>;
}

#[cfg(any(test, feature = "test-support"))]
impl MigrateTestWrites for Store {
    fn migrate_store(&self, now: OffsetDateTime) -> Result<crate::Imported> {
        migrate(self, now)
    }
}
