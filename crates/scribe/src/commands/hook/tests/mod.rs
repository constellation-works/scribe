//! Unit tests for the hook modules, one file per source file (STD-02 R19).

#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "hook unit tests stage transcript files and inspect scribe.db directly"
)]

pub(super) mod group_add;
mod user_prompt_submit;
