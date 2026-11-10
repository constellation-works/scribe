//! Whether a session record still describes something current, and who
//! cataloged it (docs/design/scribe/2_design.md §3, §5.7, store format 4).
//!
//! `active` is a second state axis beside a session's `status`: `status`
//! says whether the session has ended, `active` whether its content still
//! describes something current. An inactive session is hidden, not removed.
//! These are the persisted shapes of the last change of `active` or
//! `keep_active` and of a catalog marker; the routes that change them are
//! S10's and S14's. Pure values (STD-02 R14) and no I/O (STD-02 R5); never
//! rename or retype a field without a store format bump.

use crate::record::{Author, nullable};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// The longest `active_change.reason`, in characters.
pub const REASON_CHARS: usize = 300;

/// How `active` or `keep_active` last changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActiveRoute {
    /// The session's last message is older than the configured limit.
    Age,
    /// A later record says the session's feature was removed or replaced.
    Removed,
    /// `session activate` or `session deactivate`, by an agent or Daniel.
    Manual,
}

impl ActiveRoute {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Age => "age",
            Self::Removed => "removed",
            Self::Manual => "manual",
        }
    }
}

/// The last change of a session's `active` or `keep_active`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveChange {
    /// When it changed.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// Who changed it.
    pub by: Author,
    /// Which route changed it.
    pub route: ActiveRoute,
    /// Why, at most [`REASON_CHARS`] characters; free text, redacted.
    pub reason: String,
    /// The record that justified it (`session:<id>#<n>`, an entry id), or
    /// `null`.
    #[serde(deserialize_with = "nullable")]
    pub cite: Option<String>,
}

/// Who cataloged a session by name, and when (`session catalog`, S14).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogedBy {
    /// When it was cataloged.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// Who asked for it.
    pub by: Author,
}
