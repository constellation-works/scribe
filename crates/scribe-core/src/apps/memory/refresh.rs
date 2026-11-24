//! `scribe memory refresh` (2_design.md §15.6): the profile rebuilt by a
//! model call, an audited write.

use super::Error;
use super::verbs::MemoryVerbs;
use crate::memory::{Milestone, NowItem, Stem};
use crate::models::{MemoryContext, ProfileRequest, ProfileSession};
use crate::runtime::{Ctx, PortError};
use crate::{Models, Provider, SessionId};
use std::collections::BTreeSet;
use std::time::Duration;
use time::{Date, OffsetDateTime};

/// `memory refresh`'s model budget, fixed (2_design.md §15.6).
pub const REFRESH_BUDGET: Duration = Duration::from_secs(120);

/// `memory refresh` with `models`: read the inputs under the lock, ask the
/// model without it, then check and write under the lock again (§15.6).
/// `None` when nothing is new.
///
/// # Errors
///
/// [`Error`]: the config, the redactor, the model, or the store.
pub fn refresh<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    models: &dyn Models,
) -> Result<Option<RefreshWritten>, Error<E>> {
    let config = ctx.store.read().tags().config()?.for_provider(provider);
    let redactor = ctx.redaction.redactor(&config).map_err(Error::Port)?;
    let store = ctx.store;
    let inputs = store.profile_inputs(ctx.clock.now())?;
    if inputs.nothing_new() {
        return Ok(None);
    }
    let request = inputs.request(
        config.models.profile.clone(),
        REFRESH_BUDGET,
        config.voice(crate::voice::ModelUse::Profile),
    );
    let answer = models.profile(&request)?;
    Ok(Some(store.write_refresh(
        &inputs,
        &answer,
        &redactor,
        ctx.clock.now(),
    )?))
}

/// The most sessions one refresh reads.
pub const REFRESH_SESSIONS: usize = 40;

/// Each session's summary in a refresh, in characters.
pub const REFRESH_SUMMARY_CHARS: usize = 1500;

/// How many sessions a refresh reads verbatim.
pub const REFRESH_VERBATIM_SESSIONS: usize = 3;

/// Each verbatim session's user text, in characters.
pub const REFRESH_VERBATIM_CHARS: usize = 12_000;

/// What `memory refresh` reads before it asks the model (§15.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileInputs {
    /// The current profile, or the empty template.
    pub profile: String,
    /// Its Pinned section, which the rebuild copies.
    pub pinned: String,
    /// The user's hand edits since the last revision.
    pub corrections: Option<String>,
    /// Migrated `profile` and `projects` items, for a first profile.
    pub hints: Vec<String>,
    /// The sessions read, oldest first.
    pub sessions: Vec<ProfileSession>,
    /// Sessions after the cursor left for later runs.
    pub backlog: usize,
    /// `now` items still ahead or within their 14 days.
    pub now: Vec<NowItem>,
    /// `now` items past their 14 days.
    pub passed: Vec<NowItem>,
    /// Every milestone.
    pub milestones: Vec<Milestone>,
    /// Active preference entries: id and what they say.
    pub preferences: Vec<(String, String)>,
    /// Every tag name in the table, which the profile may cite.
    pub tags: Vec<String>,
    /// Every entry id, which the profile may cite.
    pub known: BTreeSet<String>,
    /// Where the cursor moves once the profile is written: the last session read.
    pub cursor_to: Option<(OffsetDateTime, SessionId)>,
    /// What the write checks has not changed since these inputs were read.
    pub basis: Basis,
}

/// What a refresh's write checks has not changed since its inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Basis {
    /// The latest profile revision's number.
    pub latest: Option<i64>,
    /// The digest of the profile file.
    pub file: Option<String>,
    /// The refresh cursor.
    pub cursor: Option<(OffsetDateTime, String)>,
    /// The stems of the `now` files.
    pub now: Vec<Stem>,
}

impl ProfileInputs {
    /// Whether a refresh has nothing to do: no session after the cursor, no
    /// hand edit and no passed `now` item. It then calls no model and writes
    /// nothing.
    #[must_use]
    pub fn nothing_new(&self) -> bool {
        self.sessions.is_empty() && self.corrections.is_none() && self.passed.is_empty()
    }

    /// The model request for these inputs.
    #[must_use]
    pub fn request(
        &self,
        model: String,
        budget: std::time::Duration,
        voice: crate::voice::Voice,
    ) -> ProfileRequest {
        let ctx = |stem: &Stem, date: Date, title: &str| MemoryContext {
            stem: stem.to_string(),
            date: crate::record::nullable_date::format(date),
            title: title.to_owned(),
        };
        ProfileRequest {
            model,
            budget,
            profile: self.profile.clone(),
            corrections: self.corrections.clone(),
            hints: self.hints.clone(),
            sessions: self.sessions.clone(),
            now: self
                .now
                .iter()
                .map(|n| ctx(&n.stem, n.date, &n.title))
                .collect(),
            passed: self
                .passed
                .iter()
                .map(|n| ctx(&n.stem, n.date, &n.title))
                .collect(),
            milestones: self
                .milestones
                .iter()
                .map(|m| ctx(&m.stem, m.date, &m.title))
                .collect(),
            preferences: self.preferences.clone(),
            voice,
        }
    }
}

/// What `memory refresh` wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefreshWritten {
    /// The new profile revision, when the profile changed.
    pub revision: Option<i64>,
    /// Milestones written, by stem.
    pub milestones: Vec<String>,
    /// Passed `now` items removed (to a milestone or to history), by stem.
    pub removed: Vec<String>,
    /// Sessions read.
    pub sessions: usize,
    /// Sessions after the new cursor left for later runs.
    pub backlog: usize,
}
