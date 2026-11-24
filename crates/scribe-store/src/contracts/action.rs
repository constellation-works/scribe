//! The pickup shape the store keeps in `pickups.json`
//! (docs/design/scribe/2_design.md §4.6).

use crate::entry::EntryId;
use crate::record::{Author, Related};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A consumer's note on an action entry: it picked the entry up, with the
/// record it made (`result`), or, with `rejected`, that Daniel rejected
/// that result. Persisted in `pickups.json` (format 8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pickup {
    /// The entry it is about.
    pub entry: EntryId,
    /// The consumer, always an agent.
    pub author: Author,
    /// When it was recorded.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// What the consumer made of it (`task:<id>`), or `null`.
    #[serde(deserialize_with = "crate::record::nullable")]
    pub result: Option<Related>,
    /// Daniel rejected the result.
    pub rejected: bool,
}
