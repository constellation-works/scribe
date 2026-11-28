//! scribe-core's unit tests: the store tests that call the use cases
//! (`store/`), which `scribe-store` cannot see. The store's own tests are in
//! `scribe-store`.

pub(crate) mod catchers;
mod composition;
mod config;
mod redact;
pub(crate) mod store;
mod voice;
