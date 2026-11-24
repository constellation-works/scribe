//! scribe-core's unit tests: the store tests that call the use cases
//! (`store/`), which `scribe-store` cannot see. The store's own tests are in
//! `scribe-store`.

pub(crate) use scribe_domain::fixtures::{T0, entry, session};
// `super::super::db` and `::write` in `store/`, as inside the store.
use crate::store::{db, write};

mod composition;
mod config;
mod redact;
pub(crate) mod store;
mod voice;

/// The `idea` catcher scribe shipped until 2_design.md §15.9 phase 2, now
/// a config example (specs/catchers.md): an action tag whose claims the
/// pass writes itself.
pub const IDEA_CATCHER: &str = r#"
[tags.idea]
action = true
description = "a future consideration worth revisiting"
[tags.idea.capture]
from = "user"
route = "write"
triggers = ["have * an idea", "had an idea", "here's an idea", "what do you think about * idea", "what if we"]
criteria = "The user proposes something that could be built, changed or tried, and the session does not carry it out."
not = "A direction to act now; a preference; a question of fact; an idea the session goes on to carry out."
"#;

/// The shipped tags plus [`IDEA_CATCHER`].
pub fn tags_with_idea() -> crate::tag::TagTable {
    crate::config::Config::parse(IDEA_CATCHER).unwrap().tags
}
