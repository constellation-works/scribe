//! Memory verbs: acknowledging a `now` item, forgetting an item, restoring a
//! profile revision, and the profile refresh's read and write (§15.4–§15.6).
//! Each decides inside one store write, over the memory rows and the files
//! as the open journals leave them ([`Memory::commit`](crate::repository::Memory::commit)).

use super::refresh::{
    ApplyRefreshRequest, Basis, ProfileInputs, REFRESH_SESSIONS, REFRESH_SUMMARY_CHARS,
    REFRESH_VERBATIM_CHARS, REFRESH_VERBATIM_SESSIONS, RefreshWritten,
};
use crate::apps::tag::ConfigReads;
use crate::apps::tag::check_agent_author;
use crate::entry::EntryStatus;
use crate::memory::{self, Milestone, NowItem, Stem, profile};
use crate::models::{ProfileAnswer, ProfileSession};
use crate::repository::{HistoryRow, MemoryCommit};
use crate::store::refusal;
use crate::store::{bare, not_found};
use crate::{AuthorName, MemoryShown, Redactor, Result, SessionId, Store, WriteRefusal};
use std::collections::BTreeSet;
use time::OffsetDateTime;

/// The memory verbs, implemented for [`Store`].
pub trait MemoryVerbs {
    /// Mark a `now` item raised in `session` by the agent `author`
    /// (`memory ack`, §15.4): it limits unprompted mentions. The same
    /// session again changes nothing.
    ///
    /// # Errors
    ///
    /// `reserved_author`, `memory_not_found`, `memory_not_now` (a
    /// milestone), and store failures.
    fn ack_memory(
        &self,
        id: &str,
        session: SessionId,
        author: AuthorName,
        now: OffsetDateTime,
    ) -> Result<(NowItem, bool)>;

    /// Forget a `now` item or milestone for good for the agent `author`
    /// (§15.5): its file removed and its history rows deleted. The profile
    /// is corrected by editing it, not forgotten.
    ///
    /// # Errors
    ///
    /// `reserved_author`, `usage_error` for `profile`, `memory_not_found`,
    /// and store failures.
    fn forget_memory(
        &self,
        id: &str,
        author: AuthorName,
        now: OffsetDateTime,
    ) -> Result<MemoryShown>;

    /// Write profile revision `n` back as a new revision (`memory
    /// restore`). Restoring the revision the profile already is changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// `revision_not_found`, and store failures.
    fn restore_profile(&self, n: i64, now: OffsetDateTime) -> Result<Option<i64>>;

    /// What `memory refresh` sends the model (§15.6), read under the lock
    /// after any replay and migration, which it may write.
    ///
    /// # Errors
    ///
    /// A session or entry file that does not parse, and store failures.
    fn profile_inputs(&self, now: OffsetDateTime) -> Result<ProfileInputs>;

    /// Check a prepared refresh apply request against the live store and commit
    /// it (§15.6, §19.3.5): re-checks `Basis`, forgotten sessions,
    /// `profile::check`, milestone bounds and sensitive gates under the write
    /// lock, and recomputes the passed `now` items and the sessions the
    /// cursor covers instead of trusting the request's derived fields.
    ///
    /// # Errors
    ///
    /// `memory_changed`, `invalid_id`, a `profile_*` refusal, or store failures.
    fn write_refresh_prepared(
        &self,
        request: &ApplyRefreshRequest,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<RefreshWritten>;
}

impl MemoryVerbs for Store {
    fn ack_memory(
        &self,
        id: &str,
        session: SessionId,
        author: AuthorName,
        now: OffsetDateTime,
    ) -> Result<(NowItem, bool)> {
        check_agent_author(&author)?;
        self.write(|tx| {
            let stem = bare(id).to_owned();
            let ((item, changed), _) = tx.memory().commit(now, |rows, files| {
                let Some(mut item) = files.now.iter().find(|n| n.stem.as_str() == stem).cloned()
                else {
                    if files.milestones.iter().any(|m| m.stem.as_str() == stem) {
                        return Err(refusal(
                            WriteRefusal::MemoryNotNow,
                            format!("{stem} is a milestone; only now items are acknowledged"),
                        ));
                    }
                    return Err(not_found(id, self.root()));
                };
                if item.raised.as_ref().is_some_and(|r| r.session == session) {
                    return Ok((MemoryCommit::default(), (item, false)));
                }
                item.raised = Some(memory::Raised {
                    at: now,
                    session: session.clone(),
                });
                let commit = MemoryCommit {
                    ops: vec![rows.now_op(&item)?],
                    ..MemoryCommit::default()
                };
                Ok((commit, (item, true)))
            })?;
            Ok((item, changed))
        })
    }

    fn forget_memory(
        &self,
        id: &str,
        author: AuthorName,
        now: OffsetDateTime,
    ) -> Result<MemoryShown> {
        check_agent_author(&author)?;
        if id.trim() == "profile" {
            return Err(refusal(
                WriteRefusal::InvalidId,
                "the profile is not forgotten: edit memory/profile.md outside Pinned, and the next refresh takes the edit as a correction",
            ));
        }
        self.write(|tx| {
            let stem = bare(id).to_owned();
            let (shown, journal) = tx.memory().commit(now, |rows, files| {
                let (shown, op) = if let Some(item) =
                    files.now.iter().find(|n| n.stem.as_str() == stem)
                {
                    (MemoryShown::Now(item.clone()), rows.remove_now(&item.stem))
                } else if let Some(item) = files.milestones.iter().find(|m| m.stem.as_str() == stem)
                {
                    (
                        MemoryShown::Milestone(item.clone()),
                        rows.remove_milestone(&item.stem),
                    )
                } else {
                    return Err(not_found(id, self.root()));
                };
                let commit = MemoryCommit {
                    ops: vec![op],
                    forget_history: vec![stem.clone()],
                    forget_messages: vec![stem.clone()],
                    ..MemoryCommit::default()
                };
                Ok((commit, shown))
            })?;
            // Keep the removal operation and discard earlier copies of this
            // file, including copies in already closed journals.
            tx.memory().scrub_copies(&journal)?;
            Ok(shown)
        })
    }

    fn restore_profile(&self, n: i64, now: OffsetDateTime) -> Result<Option<i64>> {
        self.write(|tx| {
            let (_, journal) = tx.memory().commit(now, |rows, files| {
                let revision = rows.revision(n)?.ok_or_else(|| {
                    refusal(
                        WriteRefusal::RevisionNotFound,
                        format!("no profile revision {n}; the first is 1"),
                    )
                })?;
                let (latest, _) = rows.revisions()?;
                let same = latest.is_some_and(|l| l.content == revision.content)
                    && files.profile.as_deref() == Some(revision.content.as_str());
                if same {
                    return Ok((MemoryCommit::default(), ()));
                }
                let commit = MemoryCommit {
                    ops: vec![rows.profile_op(&revision.content)],
                    revision: Some(revision.content),
                    ..MemoryCommit::default()
                };
                Ok((commit, ()))
            })?;
            Ok(journal.revision())
        })
    }

    fn profile_inputs(&self, now: OffsetDateTime) -> Result<ProfileInputs> {
        self.write(|tx| {
            let config = tx.tags().config()?;
            let (entries, skipped) = tx.entries().all()?;
            if let Some(bad) = skipped.into_iter().next() {
                return Err(bad.error);
            }
            let mut loaded = tx.sessions().loaded()?;
            if let Some((_, bad)) = loaded.skipped.pop() {
                return Err(bad.error);
            }
            let records = loaded.records;
            let today = now.date();
            let (inputs, _) = tx.memory().commit(now, |rows, files| {
                let (latest, closed) = rows.revisions()?;
                let cursor = rows.cursor()?;
                let file_sha = files.profile.as_deref().map(|text| rows.digest(text));
                let current = files
                    .profile
                    .clone()
                    .or_else(|| latest.as_ref().map(|r| r.content.clone()))
                    .unwrap_or_else(profile::template);
                let corrections = match (&files.profile, &closed) {
                    (Some(file), Some(closed))
                        if closed.file_sha256.as_deref() != file_sha.as_deref() =>
                    {
                        let d = profile::diff(&closed.content, file);
                        (!d.is_empty()).then_some(d)
                    }
                    _ => None,
                };
                let pinned = profile::pinned(&current)
                    .or_else(|| closed.as_ref().and_then(|c| profile::pinned(&c.content)))
                    .unwrap_or_default()
                    .to_owned();
                let hints = if latest.is_none() {
                    rows.history(&["profile", "projects"])?
                        .into_iter()
                        .map(|h| {
                            let first = h.text.lines().next().unwrap_or_default().to_owned();
                            format!("{} ({}): {first}", h.kind, h.item)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let mut due: Vec<&crate::SessionRecord> = records
                    .iter()
                    .filter(|r| r.active && after_cursor(r, cursor.as_ref()))
                    .collect();
                due.sort_by(|a, b| (a.updated, a.id.as_str()).cmp(&(b.updated, b.id.as_str())));
                let backlog = due.len().saturating_sub(REFRESH_SESSIONS);
                due.truncate(REFRESH_SESSIONS);
                let signals = rows.signals()?;
                let mut texts = Vec::new();
                for r in &due {
                    texts.push(rows.user_text(r.id.as_str())?);
                }
                let mut ranked: Vec<usize> = (0..due.len()).collect();
                ranked.sort_by(|&a, &b| {
                    let sa = signals.get(due[a].id.as_str()).copied().unwrap_or(0);
                    let sb = signals.get(due[b].id.as_str()).copied().unwrap_or(0);
                    sb.cmp(&sa)
                        .then_with(|| texts[b].chars().count().cmp(&texts[a].chars().count()))
                });
                let verbatim: BTreeSet<usize> = ranked
                    .into_iter()
                    .filter(|&i| !texts[i].is_empty())
                    .take(REFRESH_VERBATIM_SESSIONS)
                    .collect();
                let sessions = due
                    .iter()
                    .enumerate()
                    .map(|(i, r)| ProfileSession {
                        id: r.id.to_string(),
                        project: r.project.to_string(),
                        date: crate::record::nullable_date::format(r.updated.date()),
                        title: r.title.clone(),
                        summary: cut(
                            &format!("{}\nGoals: {}", r.summary.trim(), r.goals.trim()),
                            REFRESH_SUMMARY_CHARS,
                        ),
                        verbatim: verbatim
                            .contains(&i)
                            .then(|| cut(&texts[i], REFRESH_VERBATIM_CHARS)),
                    })
                    .collect();
                let cursor_to = due.last().map(|r| (r.updated, r.id.clone()));
                let (passed, ahead): (Vec<NowItem>, Vec<NowItem>) = files
                    .now
                    .iter()
                    .cloned()
                    .partition(|n| memory::expired(n.date, today));
                let preferences = entries
                    .iter()
                    .filter(|e| e.status == EntryStatus::Active && e.tag.as_str() == "preferences")
                    .map(|e| (e.id.to_string(), cut(&one_line(&e.said), 160)))
                    .collect();
                let inputs = ProfileInputs {
                    profile: current,
                    pinned,
                    corrections,
                    hints,
                    sessions,
                    backlog,
                    now: ahead,
                    passed,
                    milestones: files.milestones.clone(),
                    preferences,
                    tags: config
                        .tags
                        .tags()
                        .iter()
                        .map(|t| t.name.to_string())
                        .collect(),
                    known: entries.iter().map(|e| e.id.to_string()).collect(),
                    cursor_to,
                    basis: Basis {
                        latest: latest.map(|r| r.n),
                        file: file_sha,
                        cursor,
                        now: files.now.iter().map(|n| n.stem.clone()).collect(),
                    },
                };
                Ok((MemoryCommit::default(), inputs))
            })?;
            Ok(inputs)
        })
    }

    fn write_refresh_prepared(
        &self,
        request: &ApplyRefreshRequest,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<RefreshWritten> {
        let cursor_to = match &request.cursor_to {
            Some(cursor) => {
                let id: SessionId = cursor.session.parse().map_err(|err| {
                    refusal(
                        WriteRefusal::InvalidId,
                        format!("cursor_to.session is not a valid session id: {err}"),
                    )
                })?;
                Some((cursor.at, id))
            }
            None => None,
        };
        self.write(|tx| {
            let forgotten = tx.sessions().forgotten_ids()?;
            if request
                .session_ids
                .iter()
                .any(|session| forgotten.iter().any(|id| id.as_str() == session))
            {
                return Err(refusal(
                    WriteRefusal::MemoryChanged,
                    "a session was forgotten while the profile was rebuilt; run 'orbit job run scribe-memory-refresh' again",
                ));
            }
            let config = tx.tags().config()?;
            let (entries, skipped) = tx.entries().all()?;
            if let Some(bad) = skipped.into_iter().next() {
                return Err(bad.error);
            }
            let tags: Vec<String> = config
                .tags
                .tags()
                .iter()
                .map(|t| t.name.to_string())
                .collect();
            let known: BTreeSet<String> = entries.iter().map(|e| e.id.to_string()).collect();
            let mut loaded = tx.sessions().loaded()?;
            if let Some((_, bad)) = loaded.skipped.pop() {
                return Err(bad.error);
            }
            let records = loaded.records;
            let masks = |text: &str| redactor.mask(text) != text;
            let (written, journal) = tx.memory().commit(now, |rows, files| {
                let (latest, closed) = rows.revisions()?;
                let basis = Basis {
                    latest: latest.as_ref().map(|r| r.n),
                    file: files.profile.as_deref().map(|text| rows.digest(text)),
                    cursor: rows.cursor()?,
                    now: files.now.iter().map(|n| n.stem.clone()).collect(),
                };
                if basis != request.basis {
                    return Err(refusal(
                        WriteRefusal::MemoryChanged,
                        "memory changed while the profile was rebuilt; nothing was written, run 'orbit job run scribe-memory-refresh' again",
                    ));
                }
                let current = files
                    .profile
                    .clone()
                    .or_else(|| latest.as_ref().map(|r| r.content.clone()))
                    .unwrap_or_else(profile::template);
                let pinned = profile::pinned(&current)
                    .or_else(|| closed.as_ref().and_then(|c| profile::pinned(&c.content)))
                    .unwrap_or_default()
                    .to_owned();
                let passed = checked_passed(&request.passed_stems, &files.now, now)?;
                let backlog =
                    check_refresh_sessions(request, cursor_to.as_ref(), basis.cursor.as_ref(), &records, now)?;
                plan_refresh_commit(
                    rows,
                    files,
                    latest.as_ref(),
                    closed.as_ref(),
                    &pinned,
                    &tags,
                    &known,
                    &passed,
                    &request.session_ids,
                    backlog,
                    cursor_to.as_ref(),
                    &request.answer,
                    &masks,
                    now,
                )
            })?;
            let mut written = written;
            written.revision = journal.revision();
            Ok(written)
        })
    }
}

/// The refusal for an apply request whose derived fields fall outside what
/// `prepare` would cover on the live store.
fn not_prepared(what: &str) -> crate::Error {
    refusal(
        WriteRefusal::MemoryChanged,
        format!(
            "{what} does not match what 'memory prepare' covers; nothing was written, run 'orbit job run scribe-memory-refresh' again"
        ),
    )
}

/// Whether session `r` sorts after `cursor`, the order a refresh reads in.
fn after_cursor(r: &crate::SessionRecord, cursor: Option<&(OffsetDateTime, String)>) -> bool {
    cursor.is_none_or(|(at, id)| (r.updated, r.id.as_str()) > (*at, id.as_str()))
}

/// The request's `passed_stems`, recomputed under the write lock: each must
/// name a `now` item that is past its 14 days today (§15.6). A stem that no
/// longer names an item, or names one still current, refuses the request.
fn checked_passed(
    stems: &[Stem],
    now_items: &[NowItem],
    now: OffsetDateTime,
) -> Result<Vec<NowItem>> {
    let today = now.date();
    let mut passed: Vec<NowItem> = Vec::with_capacity(stems.len());
    for stem in stems {
        let Some(item) = now_items.iter().find(|n| &n.stem == stem) else {
            return Err(refusal(
                WriteRefusal::MemoryChanged,
                "memory changed while the profile was rebuilt; nothing was written, run 'orbit job run scribe-memory-refresh' again",
            ));
        };
        if !memory::expired(item.date, today) {
            return Err(not_prepared(
                "passed_stems names a now item within its 14 days, which",
            ));
        }
        if !passed.iter().any(|existing| existing.stem == item.stem) {
            passed.push(item.clone());
        }
    }
    Ok(passed)
}

/// Check the request's `session_ids` and `cursor_to` against the sessions
/// `prepare` reads after the live cursor (§15.6), and return the backlog
/// left after `cursor_to`, recomputed. Every id must be an active session
/// after the cursor, at most [`REFRESH_SESSIONS`] of them; `cursor_to` must
/// be one of them, not in the future, and skip no active session between
/// the cursor and itself.
fn check_refresh_sessions(
    request: &ApplyRefreshRequest,
    cursor_to: Option<&(OffsetDateTime, SessionId)>,
    cursor: Option<&(OffsetDateTime, String)>,
    records: &[crate::SessionRecord],
    now: OffsetDateTime,
) -> Result<usize> {
    let due: Vec<&crate::SessionRecord> = records
        .iter()
        .filter(|r| r.active && after_cursor(r, cursor))
        .collect();
    let ids: BTreeSet<&str> = request.session_ids.iter().map(String::as_str).collect();
    if ids.len() != request.session_ids.len() || ids.len() > REFRESH_SESSIONS {
        return Err(not_prepared("session_ids"));
    }
    if !ids
        .iter()
        .all(|id| due.iter().any(|r| r.id.as_str() == *id))
    {
        return Err(not_prepared("session_ids"));
    }
    let Some((at, session)) = cursor_to else {
        if !ids.is_empty() {
            return Err(not_prepared("cursor_to"));
        }
        return Ok(due.len());
    };
    let to = (*at, session.as_str());
    let behind = cursor.is_some_and(|(c_at, c_id)| to <= (*c_at, c_id.as_str()));
    if *at > now || behind || !ids.contains(session.as_str()) {
        return Err(not_prepared("cursor_to"));
    }
    if due
        .iter()
        .any(|r| (r.updated, r.id.as_str()) <= to && !ids.contains(r.id.as_str()))
    {
        return Err(not_prepared("cursor_to"));
    }
    Ok(due
        .iter()
        .filter(|r| (r.updated, r.id.as_str()) > to)
        .count())
}

#[allow(clippy::too_many_arguments)]
fn plan_refresh_commit(
    rows: &crate::repository::MemoryRows<'_>,
    files: &crate::repository::MemoryFiles,
    latest: Option<&crate::repository::Revision>,
    closed: Option<&crate::repository::Revision>,
    pinned: &str,
    tags: &[String],
    known: &BTreeSet<String>,
    passed: &[NowItem],
    session_ids: &[String],
    backlog: usize,
    cursor_to: Option<&(OffsetDateTime, SessionId)>,
    answer: &ProfileAnswer,
    masks: &dyn Fn(&str) -> bool,
    now: OffsetDateTime,
) -> Result<(MemoryCommit, RefreshWritten)> {
    let text = if answer.profile.ends_with('\n') {
        answer.profile.clone()
    } else {
        format!("{}\n", answer.profile)
    };
    profile::check(&text, pinned, tags, known, masks).map_err(|r| {
        let kind = match r {
            profile::ProfileRefusal::Shape => WriteRefusal::ProfileShape,
            profile::ProfileRefusal::Pinned => WriteRefusal::ProfilePinned,
            profile::ProfileRefusal::TooLong => WriteRefusal::ProfileTooLong,
            profile::ProfileRefusal::UnknownLink => WriteRefusal::ProfileUnknownLink,
            profile::ProfileRefusal::Sensitive => WriteRefusal::ProfileSensitive,
        };
        refusal(
            kind,
            format!(
                "the rebuilt profile was refused ({}); nothing was written",
                r.code()
            ),
        )
    })?;
    let sources: Vec<SessionId> = session_ids.iter().filter_map(|s| s.parse().ok()).collect();
    let mut commit = MemoryCommit::default();
    let mut out = RefreshWritten {
        sessions: session_ids.len(),
        backlog,
        ..RefreshWritten::default()
    };
    let mut promoted: Vec<&Stem> = Vec::new();
    let mut stones: Vec<Milestone> = Vec::new();
    for m in &answer.milestones {
        let bad = || {
            refusal(
                WriteRefusal::ProfileMilestone,
                "a promoted milestone names no passed now item, or has no real date, title or text within bounds; nothing was written",
            )
        };
        let title = one_line(&m.title);
        let body = one_line(&m.text);
        let date = crate::record::parse_day(m.date.trim()).map_err(|_| bad())?;
        if title.is_empty()
            || title.chars().count() > memory::TITLE_CHARS
            || body.is_empty()
            || body.chars().count() > memory::TEXT_CHARS
            || !memory::plausible_date(date, now.date())
        {
            return Err(bad());
        }
        if masks(&title)
            || masks(&body)
            || memory::finance_word(&title)
            || memory::finance_word(&body)
        {
            return Err(refusal(
                WriteRefusal::ProfileSensitive,
                "a promoted milestone holds a credential or a finance word; nothing was written",
            ));
        }
        let from = match m.now.as_deref().map(bare).filter(|s| !s.is_empty()) {
            Some(stem) => Some(
                passed
                    .iter()
                    .find(|n| n.stem.as_str() == stem)
                    .ok_or_else(bad)?,
            ),
            None => None,
        };
        if let Some(item) = from {
            promoted.push(&item.stem);
            commit
                .link_copies
                .push((item.stem.to_string(), Stem::new(date, &title).to_string()));
        }
        stones.push(Milestone {
            stem: Stem::new(date, &title),
            date,
            sources: from.map_or_else(|| sources.clone(), |n| n.sources.clone()),
            updated: now,
            title,
            text: body,
        });
    }
    for stone in stones {
        if !files.milestones.iter().any(|m| m.stem == stone.stem) {
            out.milestones.push(stone.stem.to_string());
            commit.ops.push(rows.milestone_op(&stone)?);
        }
    }
    for item in passed {
        if !files.now.iter().any(|n| n.stem == item.stem) {
            continue;
        }
        if !promoted.contains(&&item.stem) {
            commit.history.push(HistoryRow {
                item: item.stem.to_string(),
                kind: String::from("now"),
                text: format!("{}\n{}", item.title, item.text),
                sources: item.sources.iter().map(ToString::to_string).collect(),
                removed_at: now,
            });
        }
        out.removed.push(item.stem.to_string());
        commit.ops.push(rows.remove_now(&item.stem));
    }
    let changed = latest.is_none_or(|l| l.content != text)
        || files.profile.as_deref() != Some(text.as_str())
        || closed.is_none_or(|c| c.file_sha256.as_deref() != Some(rows.digest(&text).as_str()));
    if changed {
        commit.ops.insert(0, rows.profile_op(&text));
        commit.revision = Some(text);
    }
    commit.cursor = cursor_to.cloned();
    Ok((commit, out))
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
