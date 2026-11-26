//! Unit tests for the store, one file per source file (STD-02 R19). As a child
//! of the crate root this module also reaches its private modules. The
//! store's tests that call the use cases are in scribe-core
//! (`src/tests/store/`); the helpers both share are in [`crate::testing`].

#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "store unit tests stage files, permissions and synchronization channels directly"
)]

mod audit;

mod digest;

mod files;

mod migrated;

mod redact;

pub(crate) use crate::fixtures::{T0, entry, session};
pub(crate) use crate::testing::private_tempdir;
