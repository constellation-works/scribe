//! scribe-core's unit tests: the store tests that call the use cases
//! (`store/`), which `scribe-store` cannot see. The store's own tests are in
//! `scribe-store`.

pub(crate) use scribe_domain::fixtures::{T0, entry, session, tags_with_idea};
// `super::super::db` and `::write` in `store/`, as inside the store.
use crate::store::{db, write};

mod composition;
pub(crate) mod store;
