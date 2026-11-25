//! Unit tests for the store, one file per source file (STD-02 R19). As a child
//! of the crate root this module also reaches its private modules. The
//! store's tests that call the use cases are in scribe-core
//! (`src/tests/store/`); the helpers both share are in [`crate::testing`].

pub(crate) use crate::fixtures::{T0, entry, session};
pub(crate) use crate::testing::private_tempdir;

mod audit;
mod digest;
mod fsio;
mod migrated;
mod redact;
