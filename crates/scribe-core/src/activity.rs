//! Whether a session record still describes something current, and who
//! cataloged it (docs/design/scribe/2_design.md §3, §5.7, store format 4).
//!
//! `active` is a second state axis beside a session's `status`: `status`
//! says whether the session has ended, `active` whether its content still
//! describes something current. An inactive session is hidden, not removed.
//! These are the persisted shapes of the last change of `active` or
//! `keep_active` and of a catalog marker, and (S10) the change itself:
//! [`set_active`] applies a `manual`, `age` or `removed` route to a loaded
//! record, and [`past_age`] says which sessions the `age` route takes.
//! Cataloging is S14's. Pure values (STD-02 R14) and no I/O (STD-02 R5); never
//! rename or retype a field without a store format bump.

use crate::record::{Author, nullable};
use crate::session::SessionRecord;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use time::{Duration, OffsetDateTime};

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

/// A change of a session's `active` or `keep_active`, before it is applied
/// (§5.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetActive {
    /// Hide the session. A `manual` deactivation of a pinned session also
    /// unpins it; an automatic one (`age`, `removed`) leaves a pinned
    /// session alone.
    Deactivate {
        /// Who asks.
        by: Author,
        /// Which route.
        route: ActiveRoute,
        /// Why, at most [`REASON_CHARS`] characters.
        reason: String,
        /// The record that justified it.
        cite: Option<String>,
    },
    /// Show the session again (`session activate`, always `manual`), and
    /// pin (`Some(true)`) or unpin (`Some(false)`) it.
    Activate {
        /// Who asks.
        by: Author,
        /// Why; scribe's own sentence when `None`.
        reason: Option<String>,
        /// `--pin` or `--unpin`.
        pin: Option<bool>,
    },
}

/// What [`set_active`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveOutcome {
    /// `active`, `keep_active` or both changed, and `active_change` records
    /// it. `unpinned` says a deactivation also cleared `keep_active`.
    Changed {
        /// A pinned session was unpinned by its deactivation.
        unpinned: bool,
    },
    /// It was already so: nothing changed.
    Unchanged,
    /// An automatic route met a pinned session: nothing changed.
    Pinned,
}

impl ActiveOutcome {
    /// Whether the record changed.
    #[must_use]
    pub fn changed(self) -> bool {
        matches!(self, Self::Changed { .. })
    }
}

/// When the session's last message was sent: its last segment's `to`.
/// A record with no segment yet falls back to the later of `started` and
/// `on_since`, never to `updated`, which a rebuild, link or catalog
/// rewrites without new conversation (§5.7).
#[must_use]
pub fn last_message(record: &SessionRecord) -> OffsetDateTime {
    record
        .log
        .last()
        .map_or_else(|| record.started.max(record.on_since), |s| s.to)
}

/// Whether `record` is due for the `age` route at `now`: active, not
/// pinned, and its last message more than `limit_days` days before `now`.
#[must_use]
pub fn past_age(record: &SessionRecord, limit_days: NonZeroU32, now: OffsetDateTime) -> bool {
    record.active
        && !record.keep_active
        && now - last_message(record) > Duration::days(i64::from(limit_days.get()))
}

/// The `age` route's change for `record`: scribe's own sentence, no cite.
#[must_use]
pub fn age_change(record: &SessionRecord, limit_days: NonZeroU32) -> SetActive {
    SetActive::Deactivate {
        by: Author::consistency(),
        route: ActiveRoute::Age,
        reason: format!(
            "no messages since {} (limit {} days)",
            last_message(record).date(),
            limit_days
        ),
        cite: None,
    }
}

/// Apply `request` to `record` at `now`. Only `active`, `keep_active` and
/// `active_change` change, and only when something does: never the text,
/// `updated` or the pass state (§5.7). A reason is cut to
/// [`REASON_CHARS`].
pub fn set_active(
    record: &mut SessionRecord,
    request: &SetActive,
    now: OffsetDateTime,
) -> ActiveOutcome {
    match request {
        SetActive::Deactivate {
            by,
            route,
            reason,
            cite,
        } => {
            let automatic = *route != ActiveRoute::Manual;
            if automatic && record.keep_active {
                return ActiveOutcome::Pinned;
            }
            if !record.active && !record.keep_active {
                return ActiveOutcome::Unchanged;
            }
            let unpinned = record.keep_active;
            record.active = false;
            record.keep_active = false;
            record.active_change = Some(ActiveChange {
                at: now,
                by: by.clone(),
                route: *route,
                reason: cut(reason.trim()),
                cite: cite.clone(),
            });
            ActiveOutcome::Changed { unpinned }
        }
        SetActive::Activate { by, reason, pin } => {
            let keep = pin.unwrap_or(record.keep_active);
            if record.active && record.keep_active == keep {
                return ActiveOutcome::Unchanged;
            }
            let mut did = Vec::new();
            if !record.active {
                did.push("activated");
            }
            if keep != record.keep_active {
                did.push(if keep { "pinned" } else { "unpinned" });
            }
            record.active = true;
            record.keep_active = keep;
            record.active_change = Some(ActiveChange {
                at: now,
                by: by.clone(),
                route: ActiveRoute::Manual,
                reason: reason.as_deref().map_or_else(
                    || format!("{} by hand", did.join(" and ")),
                    |r| cut(r.trim()),
                ),
                cite: None,
            });
            ActiveOutcome::Changed { unpinned: false }
        }
    }
}

fn cut(text: &str) -> String {
    text.chars().take(REASON_CHARS).collect()
}
