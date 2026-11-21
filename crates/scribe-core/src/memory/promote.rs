//! Promotion of a `now` item to `milestones` (docs/design/scribe/2_design.md
//! §13.4), on three rules Daniel edits under `[memory.promote]`:
//!
//! - (a) `profile_change`: the event changes a `profile` fact. The model
//!   gives the new profile statement (`promote`) and names the profile item
//!   it replaces (`same_as`); the pass writes the milestone and the new
//!   profile item and supersedes the old one;
//! - (b) `recurs_in_sessions`: the event recurs in that many distinct
//!   sessions, counted model-free after each write and each merge;
//! - (c) `user_says`: one of the phrases occurs in the user's turn (matched
//!   as capture triggers are).
//!
//! A promotion writes the milestone first, carrying the whole journal
//! ([`Promotion`]: the rule and every id), so a crash between the files is
//! completed by the next consistency scan ([`complete`]). The `now` item's
//! occurrences move to the milestone, as a merge moves them, so the user's
//! words are stored once and `forget` finds them in one place.
//!
//! Pure functions over loaded items: no I/O (STD-02 R5).

use super::{
    Applied, Checked, Draft, MemoryConfig, MemoryId, MemoryItem, MemoryStatus, Provenance, Refusal,
    Section, apply,
};
use crate::record::{Author, nullable};
use crate::tag::{Rule, RuleOrigin};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// The phrases rule (c) ships: the user saying an event is a milestone.
pub const USER_SAYS: &[&str] = &[
    "milestone",
    "big day",
    "remember this",
    "never forget",
    "life changing",
];
/// Rule (b)'s shipped count of distinct sessions.
pub const RECURS_IN_SESSIONS: u32 = 3;
/// The longest new profile statement rule (a) takes, in characters.
pub const PROMOTE_CHARS: usize = 160;

/// Which rule promoted an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionRule {
    /// (a) the event changes a profile fact.
    ProfileChange,
    /// (b) the event recurs in enough sessions.
    RecursInSessions,
    /// (c) the user says it is a milestone.
    UserSays,
}

impl PromotionRule {
    /// The rule's name, as `[memory.promote]` keys it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProfileChange => "profile_change",
            Self::RecursInSessions => "recurs_in_sessions",
            Self::UserSays => "user_says",
        }
    }
}

/// A promotion's journal: on the milestone from its first write, and on the
/// `now` item once it is `promoted`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Promotion {
    /// The rule that held.
    pub rule: PromotionRule,
    /// The milestone.
    pub to: MemoryId,
    /// Rule (a): the profile item the change supersedes, when there was one.
    #[serde(deserialize_with = "nullable")]
    pub profile_from: Option<MemoryId>,
    /// Rule (a): the new profile item.
    #[serde(deserialize_with = "nullable")]
    pub profile_to: Option<MemoryId>,
}

/// `[memory.promote]`, each field with its origin (`memory rules` prints
/// them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromoteRules {
    /// Rule (a).
    pub profile_change: Rule<bool>,
    /// Rule (b); 0 switches it off.
    pub recurs_in_sessions: Rule<u32>,
    /// Rule (c)'s phrases; a configured list replaces the shipped one.
    pub user_says: Rule<Vec<String>>,
}

impl Default for PromoteRules {
    fn default() -> Self {
        Self {
            profile_change: Rule {
                value: true,
                origin: RuleOrigin::Shipped,
            },
            recurs_in_sessions: Rule {
                value: RECURS_IN_SESSIONS,
                origin: RuleOrigin::Shipped,
            },
            user_says: Rule {
                value: USER_SAYS.iter().map(|s| (*s).to_owned()).collect(),
                origin: RuleOrigin::Shipped,
            },
        }
    }
}

impl PromoteRules {
    /// Whether rule (c) holds on the user's `turn`: one of the phrases
    /// occurs in it, matched as a capture trigger.
    #[must_use]
    pub fn user_says(&self, turn: &str) -> bool {
        self.user_says
            .value
            .iter()
            .any(|phrase| crate::trigger::find(phrase, turn).is_some())
    }

    /// Whether rule (b) holds for `item`: an active `now` item whose
    /// occurrences come from at least `recurs_in_sessions` distinct
    /// sessions.
    #[must_use]
    pub fn recurs(&self, item: &MemoryItem) -> bool {
        let n = self.recurs_in_sessions.value;
        n > 0
            && item.status == MemoryStatus::Active
            && item.section == Section::Now
            && item.sessions() >= usize::try_from(n).unwrap_or(usize::MAX)
    }
}

/// Promote the `now` item at `index` under `rule`: a new milestone numbered
/// past the highest, with the item's statement, sensitivity and date (its
/// event date, else its latest occurrence's) and its occurrences; the item
/// becomes `promoted`. Both carry the journal. Returns the milestone's id;
/// an item that is not an active `now` item is left alone (`None`).
pub fn promote(
    items: &mut Vec<MemoryItem>,
    index: usize,
    rule: PromotionRule,
    by: &Author,
    now: OffsetDateTime,
) -> Option<MemoryId> {
    let item = items.get(index)?;
    if item.status != MemoryStatus::Active || item.section != Section::Now {
        return None;
    }
    let next = items.iter().map(|i| i.id.number()).max().unwrap_or(0) + 1;
    let to = MemoryId::numbered(next);
    let promotion = Promotion {
        rule,
        to: to.clone(),
        profile_from: None,
        profile_to: None,
    };
    let item = &mut items[index];
    let occurrences = std::mem::take(&mut item.occurrences);
    let event_date = item
        .event_date
        .or_else(|| occurrences.iter().map(|o| o.date).max().map(|d| d.date()));
    let milestone = MemoryItem {
        id: to.clone(),
        section: Section::Milestones,
        status: MemoryStatus::Active,
        statement: item.statement.clone(),
        project: None,
        event_date,
        sensitive: item.sensitive,
        sensitive_kind: item.sensitive_kind,
        acknowledged: None,
        retirement_reason: None,
        promoted_from: Some(item.id.clone()),
        promotion: Some(promotion.clone()),
        merged_into: None,
        superseded_by: None,
        unrelated_to: Vec::new(),
        author: by.clone(),
        updated_by: by.clone(),
        created: now,
        updated: now,
        occurrences,
    };
    item.status = MemoryStatus::Promoted;
    item.promotion = Some(promotion);
    item.updated = now;
    item.updated_by = by.clone();
    items.push(milestone);
    Some(to)
}

/// Supersede the profile item `from` by `to` (rule (a)), when `from` is an
/// active profile item other than `to`; records both on the milestone's
/// journal (and the promoted item's, when there is one). Returns whether
/// `from` was superseded.
pub fn supersede_profile(
    items: &mut [MemoryItem],
    milestone: &MemoryId,
    from: Option<&MemoryId>,
    to: &MemoryId,
    by: &Author,
    now: OffsetDateTime,
) -> bool {
    let from = from.filter(|f| *f != to).filter(|f| {
        items.iter().any(|i| {
            &i.id == *f && i.section == Section::Profile && i.status == MemoryStatus::Active
        })
    });
    for item in items.iter_mut() {
        let journal = item
            .promotion
            .as_mut()
            .filter(|p| &p.to == milestone && p.rule == PromotionRule::ProfileChange);
        if let Some(p) = journal {
            p.profile_from = from.cloned();
            p.profile_to = Some(to.clone());
        }
    }
    let Some(from) = from else {
        return false;
    };
    if let Some(old) = items.iter_mut().find(|i| &i.id == from) {
        old.status = MemoryStatus::Superseded;
        old.superseded_by = Some(to.clone());
        old.updated = now;
        old.updated_by = by.clone();
    }
    true
}

/// Complete every promotion a milestone journals but the store does not
/// hold (a crash between the files, §13.4): the `now` item `promoted`, with
/// the journal and no occurrence the milestone already has; the profile
/// item it names superseded. Returns the ids changed. Running it again
/// changes nothing.
pub fn complete(items: &mut [MemoryItem], by: &Author, now: OffsetDateTime) -> Vec<MemoryId> {
    let journals: Vec<(Option<MemoryId>, Promotion, Vec<String>)> = items
        .iter()
        .filter(|m| m.section == Section::Milestones)
        .filter_map(|m| {
            let promotion = m.promotion.clone()?;
            let messages = m.occurrences.iter().map(|o| o.message.clone()).collect();
            Some((m.promoted_from.clone(), promotion, messages))
        })
        .collect();
    let mut changed = Vec::new();
    for (from, promotion, messages) in journals {
        if let Some(item) = from
            .as_ref()
            .and_then(|f| items.iter_mut().find(|i| &i.id == f))
            .filter(|i| i.status == MemoryStatus::Active)
        {
            item.occurrences.retain(|o| !messages.contains(&o.message));
            item.status = MemoryStatus::Promoted;
            item.promotion = Some(promotion.clone());
            item.updated = now;
            item.updated_by = by.clone();
            changed.push(item.id.clone());
        }
        if let Some(to) = &promotion.profile_to
            && items.iter().any(|i| &i.id == to)
        {
            let from = promotion.profile_from.clone();
            if let Some(old) = from
                .as_ref()
                .and_then(|f| items.iter_mut().find(|i| &i.id == f))
                .filter(|i| i.status == MemoryStatus::Active && i.section == Section::Profile)
            {
                old.status = MemoryStatus::Superseded;
                old.superseded_by = Some(to.clone());
                old.updated = now;
                old.updated_by = by.clone();
                changed.push(old.id.clone());
            }
        }
    }
    changed
}

/// Promote every active `now` item rule (b) holds for, in id order.
/// Returns `(now item, milestone)` per promotion.
pub fn recount(
    items: &mut Vec<MemoryItem>,
    rules: &PromoteRules,
    by: &Author,
    now: OffsetDateTime,
) -> Vec<(MemoryId, MemoryId)> {
    let due: Vec<MemoryId> = items
        .iter()
        .filter(|i| rules.recurs(i))
        .map(|i| i.id.clone())
        .collect();
    let mut out = Vec::new();
    for id in due {
        let Some(index) = items.iter().position(|i| i.id == id) else {
            continue;
        };
        if let Some(to) = promote(items, index, PromotionRule::RecursInSessions, by, now) {
            out.push((id, to));
        }
    }
    out
}

/// One promotion a write made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Promoted {
    /// The `now` item promoted; `None` for a milestone written directly.
    pub item: Option<MemoryId>,
    /// The milestone.
    pub to: MemoryId,
    /// The rule that held.
    pub rule: PromotionRule,
    /// Rule (a): the new profile item.
    pub profile_to: Option<MemoryId>,
    /// Rule (a): the profile item it superseded.
    pub profile_from: Option<MemoryId>,
}

/// What [`write`] did with one draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    /// The draft's own item.
    pub applied: Applied,
    /// The promotions it caused, rule (b)'s recount included.
    pub promoted: Vec<Promoted>,
}

/// Write one pass draft with the promotion rules (§13.4): rule (a) when
/// the element carries a profile statement, `profile_change` is on and the
/// `profile` section is on; else rule (c) when one of `user_says` occurs in
/// the turn. A `milestones` element is written as a milestone only when
/// one holds, and is otherwise a `now` element (refused `section_off` when
/// `now` is off). A `now` element one holds for is written, then promoted.
/// Rule (a) then writes the new profile item and supersedes the one
/// `same_as` names. Last, rule (b) is recounted over every item. A
/// replayed turn promotes nothing, so a replayed window changes nothing.
///
/// # Errors
///
/// [`Refusal::SectionOff`] for a `milestones` element no rule holds for
/// while `now` is off.
pub fn write(
    items: &mut Vec<MemoryItem>,
    draft: &Draft,
    provenance: &Provenance,
    config: &MemoryConfig,
    now: OffsetDateTime,
) -> Result<Written, Refusal> {
    let section = draft.checked.section;
    let dated = matches!(section, Section::Now | Section::Milestones);
    let rule = if !dated {
        None
    } else if config.promote.profile_change.value
        && config.section_on(Section::Profile)
        && draft.checked.promote.is_some()
    {
        Some(PromotionRule::ProfileChange)
    } else if config.promote.user_says(&draft.turn) {
        Some(PromotionRule::UserSays)
    } else {
        None
    };
    let mut draft = draft.clone();
    if section == Section::Milestones && rule.is_none() {
        if !config.section_on(Section::Now) {
            return Err(Refusal::SectionOff);
        }
        draft.checked.section = Section::Now;
    }
    let by = Author::session_pass();
    let applied = apply(items, &draft, provenance, now);
    let mut promoted = Vec::new();
    let fresh = match &applied {
        Applied::Created(id) | Applied::Appended(id) => Some(id.clone()),
        Applied::Replayed(_) => None,
    };
    if let (Some(rule), Some(id)) = (rule, fresh) {
        let index = items.iter().position(|i| i.id == id);
        let target = index.and_then(|index| match items[index].section {
            Section::Now => promote(items, index, rule, &by, now).map(|to| (Some(id.clone()), to)),
            // A milestone written directly journals its own rule, so a
            // crash before the profile item is completed by the scan.
            Section::Milestones if matches!(applied, Applied::Created(_)) => {
                items[index].promotion = Some(Promotion {
                    rule,
                    to: id.clone(),
                    profile_from: None,
                    profile_to: None,
                });
                Some((None, id.clone()))
            }
            Section::Milestones | Section::Profile | Section::Projects | Section::Working => None,
        });
        if let Some((item, to)) = target {
            let mut done = Promoted {
                item,
                to: to.clone(),
                rule,
                profile_to: None,
                profile_from: None,
            };
            if let (PromotionRule::ProfileChange, Some(statement)) =
                (rule, draft.checked.promote.clone())
            {
                let profile = Draft {
                    checked: Checked {
                        section: Section::Profile,
                        statement,
                        same_as: None,
                        event_date: None,
                        sensitive: false,
                        sensitive_kind: None,
                        promote: None,
                        ..draft.checked.clone()
                    },
                    event_date: None,
                    ..draft.clone()
                };
                let profile_to = match apply(items, &profile, provenance, now) {
                    Applied::Created(id) | Applied::Appended(id) | Applied::Replayed(id) => id,
                };
                let from = draft.checked.same_as.clone();
                if supersede_profile(items, &to, from.as_ref(), &profile_to, &by, now) {
                    done.profile_from = from;
                }
                done.profile_to = Some(profile_to);
            }
            promoted.push(done);
        }
    }
    for (item, to) in recount(items, &config.promote, &by, now) {
        promoted.push(Promoted {
            item: Some(item),
            to,
            rule: PromotionRule::RecursInSessions,
            profile_to: None,
            profile_from: None,
        });
    }
    Ok(Written { applied, promoted })
}
