//! The store's tests that call the use cases (`*Verbs`), so they live in
//! scribe-core over `scribe-store` (ORB-27707 step 4b). The helpers and
//! store internals they share with the store's own tests come from
//! `scribe_store::testing`.

pub(crate) use scribe_store::testing::{
    SESSION_BODY, Seed, entry_text, on_disk, private_tempdir, seed, session_text,
};

mod archive;
mod db;
mod documents;
mod memory;
mod rebuild;
mod sessions;
// The store facade's tests, named as they were inside the store.
#[allow(clippy::module_inception)]
mod store;
