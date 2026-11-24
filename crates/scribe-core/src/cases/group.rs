//! Group membership verbs: opening a group, joining, adding and moving
//! members, leaving, a cleared session's seat and the deferred ancestry check
//! (2_design.md §17). Every decision and its write share one store write: the
//! verbs read and write the rows through [`Groups::rows`](crate::repo::Groups::rows).

use crate::checks::group::{check_additions, check_join, check_new_group, check_project_slug};
use crate::checks::refusal;
use crate::group::{
    Added, GroupAdded, GroupAddition, GroupConsent, JoinGroup, NewGroup, OpenedGroup, SeatHold,
    SeatStart,
};
use crate::message::NewMessage;
use crate::{Project, Redactor, Result, SessionId, Store, WriteRefusal};
use time::OffsetDateTime;

/// The group membership verbs, implemented for [`Store`].
pub trait GroupVerbs {
    /// Open a group on verified consent. The opener joins a seat separately.
    /// `earlier_turns` are the opener's typed turns from the start of its own
    /// transcript. They are stored with the group in the same transaction, so
    /// the line that started the work is kept even though it was typed before
    /// the group existed. An opener that is already a member of a group, or has
    /// been forgotten, stores none: its own join decided what it keeps.
    fn open_group(
        &self,
        draft: NewGroup,
        consent: &GroupConsent,
        earlier_turns: &[NewMessage],
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<OpenedGroup>;

    /// Create or join an open project group on a user-typed hook command.
    fn join_project_group(
        &self,
        slug: &str,
        project: &Project,
        session: &SessionId,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<String>;

    /// Add named past sessions to an open project group on a user-typed hook
    /// command, opening the group when absent. A session already in another
    /// group refuses the whole line, naming each such session and its group, so
    /// nothing changes. A session whose transcript was read has its typed turns
    /// stored, whatever their age, and its capture cursor set to the last of
    /// them; one without a transcript gets membership only. Backfilled turns
    /// must all be user turns belonging to their named session; otherwise the
    /// whole call is refused before it writes anything.
    fn add_project_group_sessions(
        &self,
        slug: &str,
        project: &Project,
        opener: &SessionId,
        additions: &[GroupAddition],
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<GroupAdded>;

    /// Append a member, or return false on retry. An ended transcript is named
    /// explicitly so a stale liveness check cannot authorize a different replacement.
    fn join_group(&self, join: JoinGroup<'_>, redactor: &Redactor) -> Result<bool>;

    /// Remove only the caller from its chain and remove a seat left empty.
    fn leave_group(&self, session: &SessionId) -> Result<String>;

    /// Move a member atomically, retaining its stored turns and capture cursor.
    fn move_project_group(
        &self,
        session: &SessionId,
        slug: &str,
        project: &Project,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<String>;

    /// Record that a start could not inspect its transcript yet.
    fn defer_ancestry(&self, session: &SessionId) -> Result<bool>;

    /// Whether a deferred ancestry check is pending for `session`.
    fn has_deferred_ancestry(&self, session: &SessionId) -> Result<bool>;

    /// Consume a deferred check once, without writing on ordinary prompts.
    fn take_deferred_ancestry(&self, session: &SessionId) -> Result<bool>;

    /// Inherit the predecessor's open seat for a newly cleared session.
    /// An ordinary start takes no write lock; any candidate write rechecks
    /// seat ownership inside the transaction.
    fn inherit_seat(&self, child: &SessionId, predecessor: Option<&SessionId>)
    -> Result<SeatStart>;
}

impl GroupVerbs for Store {
    fn open_group(
        &self,
        mut draft: NewGroup,
        consent: &GroupConsent,
        earlier_turns: &[NewMessage],
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<OpenedGroup> {
        check_new_group(&draft, consent, earlier_turns)?;
        redactor.group(&mut draft);
        self.write(|tx| {
            tx.groups().rows("open group in", |rows| {
                let still_open = rows.open_in_project(&draft.project)?;
                let n = rows.insert_group(&draft, consent.session(), now)?;
                let opener = consent.session();
                if !earlier_turns.is_empty()
                    && !rows.has_membership(opener)?
                    && !rows.session_forgotten(opener)?
                {
                    rows.store_turns(opener, earlier_turns, redactor)?;
                }
                Ok(OpenedGroup {
                    id: format!("g-{n}"),
                    still_open,
                })
            })
        })
    }

    fn join_project_group(
        &self,
        slug: &str,
        project: &Project,
        session: &SessionId,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<String> {
        check_project_slug(slug, redactor)?;
        self.write(|tx| {
            self.refuse_forgotten(session)?;
            tx.groups().rows("join project group in", |rows| {
                let held = rows.project_membership(session)?;
                if let Some((_, name)) = held {
                    return Err(refusal(
                        WriteRefusal::GroupMembership,
                        format!(
                            "session already belongs to group {}; leave it before joining",
                            redactor.mask(&name)
                        ),
                    ));
                }
                let existing = rows.open_project(slug)?;
                let n = match existing {
                    Some(n) => n,
                    None => rows.insert_project(slug, project, now, session)?,
                };
                rows.seat_member(n, session, now)?;
                Ok(format!("g-{n}"))
            })
        })
    }

    fn add_project_group_sessions(
        &self,
        slug: &str,
        project: &Project,
        opener: &SessionId,
        additions: &[GroupAddition],
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<GroupAdded> {
        check_project_slug(slug, redactor)?;
        check_additions(additions)?;
        self.write(|tx| {
            for addition in additions {
                self.refuse_forgotten(&addition.session)?;
            }
            tx.groups().rows("add to project group in", |rows| {
                let existing = rows.open_project(slug)?;
                let mut elsewhere = Vec::new();
                let mut already = Vec::new();
                for addition in additions {
                    let held = rows.project_membership(&addition.session)?;
                    match held {
                        Some((group, _)) if Some(group) == existing => {
                            already.push(addition.session.as_str());
                        }
                        Some((_, name)) => elsewhere.push(format!(
                            "session {} already belongs to group {}",
                            addition.session,
                            redactor.mask(&name)
                        )),
                        None => {}
                    }
                }
                if !elsewhere.is_empty() {
                    return Err(refusal(
                        WriteRefusal::GroupMembership,
                        format!("{}; nothing changed", elsewhere.join("; ")),
                    ));
                }
                let (n, created) = match existing {
                    Some(n) => (n, false),
                    None => (rows.insert_project(slug, project, now, opener)?, true),
                };
                let mut sessions = Vec::new();
                for addition in additions {
                    if already.contains(&addition.session.as_str()) {
                        sessions.push((addition.session.clone(), Added::AlreadyMember));
                        continue;
                    }
                    rows.seat_member(n, &addition.session, now)?;
                    let stored = match &addition.turns {
                        None => None,
                        Some(turns) => {
                            let inserted = rows.store_turns(&addition.session, turns, redactor)?;
                            if let Some(last) = turns.last() {
                                rows.set_cursor(&addition.session, &last.message)?;
                            }
                            Some(inserted)
                        }
                    };
                    sessions.push((addition.session.clone(), Added::Joined { stored }));
                }
                Ok(GroupAdded {
                    id: format!("g-{n}"),
                    created,
                    sessions,
                })
            })
        })
    }

    fn join_group(&self, join: JoinGroup<'_>, redactor: &Redactor) -> Result<bool> {
        check_join(&join)?;
        self.write(|tx| {
            self.refuse_forgotten(join.session)?;
            let n = tx.groups().number(join.group)?;
            tx.groups().rows("join group in", |rows| {
                let seat = join.seat.as_str();
                let closed = rows.is_closed(n)?;
                let held = rows.membership(join.session)?;
                if let Some((group, name)) = &held
                    && (*group != n || name != seat)
                {
                    return Err(refusal(
                        WriteRefusal::GroupMembership,
                        format!("the calling session already belongs to group g-{group}; leave it before joining"),
                    ));
                }
                if closed && held.is_none() {
                    return Err(refusal(
                        WriteRefusal::GroupMembership,
                        "the group is already closed",
                    ));
                }

                let role = join.role.map(|role| redactor.mask(role));
                let current = rows.seat_holder(n, seat)?;
                if let Some((stored_role, latest)) = current {
                    if role.as_ref().is_some_and(|r| r != &stored_role) {
                        return Err(refusal(
                            WriteRefusal::GroupMembership,
                            "the seat already has a different role; omit --role to keep it",
                        ));
                    }
                    if held.is_some() {
                        return Ok(false);
                    }
                    if !join.replace
                        && join.ended.map(SessionId::as_str) != Some(latest.as_str())
                        && join.predecessor.map(SessionId::as_str) != Some(latest.as_str())
                    {
                        return Err(refusal(
                            WriteRefusal::SeatOccupied,
                            format!("seat {seat} is held by session {latest}; pass --replace to take it over"),
                        ));
                    }
                } else {
                    let role = role.unwrap_or_else(|| redactor.mask(seat));
                    rows.insert_seat(n, seat, &role)?;
                }
                rows.insert_member(
                    n,
                    seat,
                    join.session,
                    join.author.as_str(),
                    join.snapshot_at,
                )?;
                // The capture starts at the session's own first line, not at the
                // join time, so the cursor takes over from its last stored turn.
                // With no stored turn the join time is the snapshot's, taken before
                // the transcript was read, so a turn typed since stays in capture.
                if let Some(last) = join.earlier_turns.last() {
                    rows.store_turns(join.session, join.earlier_turns, redactor)?;
                    rows.set_cursor(join.session, &last.message)?;
                }
                Ok(true)
            })
        })
    }

    fn leave_group(&self, session: &SessionId) -> Result<String> {
        leave(self, session, false)
    }

    fn move_project_group(
        &self,
        session: &SessionId,
        slug: &str,
        project: &Project,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<String> {
        check_project_slug(slug, redactor)?;
        self.write(|tx| {
            tx.groups().rows("move group member in", |rows| {
                let window = rows.member_window(session)?.ok_or_else(|| {
                    refusal(
                        WriteRefusal::GroupMembership,
                        "session has no group membership",
                    )
                })?;
                let old = window.group;
                let target = rows.open_project(slug)?;
                if target == Some(old) {
                    return Ok(format!("g-{old}"));
                }
                leave(self, session, true)?;
                let id = self.join_project_group(slug, project, session, now, redactor)?;
                if window.holds_seat {
                    rows.set_window(session, &window.joined, window.cursor.as_deref())?;
                } else {
                    rows.restart_window(session, now)?;
                }
                if let Some(n) = target {
                    rows.clear_summary(n)?;
                }
                Ok(id)
            })
        })
    }

    fn defer_ancestry(&self, session: &SessionId) -> Result<bool> {
        self.write(|tx| {
            tx.groups().rows(
                "defer ancestry in",
                |rows| Ok(rows.defer_ancestry(session)?),
            )
        })
    }

    fn has_deferred_ancestry(&self, session: &SessionId) -> Result<bool> {
        self.read().groups().deferred_ancestry(session)
    }

    fn take_deferred_ancestry(&self, session: &SessionId) -> Result<bool> {
        if !self.has_deferred_ancestry(session)? {
            return Ok(false);
        }
        self.write(|tx| {
            tx.groups().rows("consume deferred ancestry in", |rows| {
                Ok(rows.take_deferred_ancestry(session)?)
            })
        })
    }

    fn inherit_seat(
        &self,
        child: &SessionId,
        predecessor: Option<&SessionId>,
    ) -> Result<SeatStart> {
        #[derive(Default)]
        enum Snapshot {
            #[default]
            NoOpenGroups,
            AlreadySeated(SeatHold),
            Replaced,
            Inherit,
            Unjoined,
        }

        let predecessor = predecessor.filter(|parent| *parent != child);
        let snap = self.read().groups().read_rows(|rows| {
            if let Some(hold) = rows.held_seat(child)? {
                return Ok(Snapshot::AlreadySeated(hold));
            }
            if !rows.has_open_groups()? {
                return Ok(Snapshot::NoOpenGroups);
            }
            if rows.has_membership(child)? {
                return Ok(Snapshot::Replaced);
            }
            let Some(parent) = predecessor else {
                return Ok(Snapshot::Unjoined);
            };
            if rows.newest_open_seat(parent)?.is_none() {
                return Ok(Snapshot::Unjoined);
            }
            Ok(Snapshot::Inherit)
        })?;
        let Snapshot::Inherit = snap else {
            return Ok(match snap {
                Snapshot::NoOpenGroups => SeatStart::NoOpenGroups,
                Snapshot::AlreadySeated(hold) => SeatStart::AlreadySeated(hold),
                Snapshot::Replaced => SeatStart::Replaced,
                Snapshot::Unjoined => SeatStart::Unjoined,
                Snapshot::Inherit => unreachable!(),
            });
        };
        let Some(parent) = predecessor else {
            return Ok(SeatStart::Unjoined);
        };
        self.write(|tx| {
            self.refuse_forgotten(child)?;
            tx.groups().rows("inherit group seat in", |rows| {
                if let Some(hold) = rows.held_seat(child)? {
                    return Ok(SeatStart::AlreadySeated(hold));
                }
                if !rows.has_open_groups()? {
                    return Ok(SeatStart::NoOpenGroups);
                }
                if rows.has_membership(child)? {
                    return Ok(SeatStart::Replaced);
                }
                let Some((group, seat, role, author)) = rows.newest_open_seat(parent)? else {
                    return Ok(SeatStart::Unjoined);
                };
                rows.insert_member(group, &seat, child, &author, OffsetDateTime::now_utc())?;
                Ok(SeatStart::Inherited {
                    hold: SeatHold {
                        group: format!("g-{group}"),
                        seat,
                        role,
                    },
                    from: parent.clone(),
                })
            })
        })
    }
}

/// Remove `session` from its seat, and erase its group-only turns unless
/// `keep_turns` (a move keeps them).
fn leave(store: &Store, session: &SessionId, keep_turns: bool) -> Result<String> {
    store.write(|tx| {
        tx.groups().rows("leave group in", |rows| {
            let n = rows
                .remove_member(session, OffsetDateTime::now_utc())?
                .ok_or_else(|| {
                    refusal(
                        WriteRefusal::GroupMembership,
                        "the calling session has no group membership",
                    )
                })?;
            let erased = if keep_turns {
                0
            } else {
                rows.erase_group_turns(session)?
            };
            let cleared = rows.clear_set_summary(n)?;
            if erased > 0 {
                rows.optimize_search()?;
            }
            if erased > 0 || cleared > 0 {
                rows.truncate_wal()?;
            }
            Ok(format!("g-{n}"))
        })
    })
}
