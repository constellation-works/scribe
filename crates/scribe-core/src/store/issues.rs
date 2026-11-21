//! Issue records on disk, `<root>/issues/<id>.md` (docs/design/scribe/
//! 2_design.md §11.4): load, resolve, and the locked writes `issue add`,
//! `occur`, `update` and `move` run, and Daniel's `#scribe forget` of an
//! issue or one occurrence.
//!
//! Every write holds the store lock, reads the issue again under it and
//! replaces the file whole through `write_atomic`; new free text passes the
//! redactor first. Reads take no lock. Ids are allocated as entry ids are:
//! one past the highest on disk, unreadable files included, so a forgotten
//! issue's id is never given again.

use super::agent::refusal;
use super::ids::resolve;
use super::issue_file::{decode_issue, encode_issue};
use super::{RECORD_EXT, Redactor, Skipped, Store, fsio, visible};
use crate::error::{Error, Result, WriteRefusal};
use crate::issue::{
    Gone, GoneReason, Issue, IssueFilter, IssueId, IssuePatch, IssueRef, IssueStatus, NewIssue,
    NewOccurrence, check_signature,
};
use crate::record::{Author, Listing, Topics};
use std::io;
use std::num::NonZeroU32;
use std::path::PathBuf;
use time::OffsetDateTime;

/// The issues directory's name under the data directory.
pub const ISSUES_DIR: &str = "issues";

/// What [`Store::list_issues`] found.
#[derive(Debug)]
pub struct IssueList {
    /// The matching issues, most recently updated first.
    pub listing: Listing<Issue>,
    /// Issue files left out because they did not parse.
    pub skipped: Vec<Skipped>,
}

/// Where `issue move` sends an occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveTo {
    /// An existing issue, by id or unique prefix.
    Issue(String),
    /// A new issue with this title, signature and topics.
    New {
        /// Its title.
        title: String,
        /// Its signature strings.
        signature: Vec<String>,
        /// Its topics.
        topics: Topics,
    },
}

/// What [`Store::move_occurrence`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    /// The issue it left, with the number recorded as moved.
    pub from: Issue,
    /// Its number there.
    pub from_n: NonZeroU32,
    /// The issue it joined.
    pub to: Issue,
    /// Its number there.
    pub to_n: NonZeroU32,
}

impl Store {
    /// The file issue `id` lives in.
    #[must_use]
    pub fn issue_path(&self, id: IssueId) -> PathBuf {
        self.root.join(ISSUES_DIR).join(format!("{id}{RECORD_EXT}"))
    }

    /// Issues matching `filter`, most recently updated first. A missing
    /// store or directory is empty.
    pub fn list_issues(&self, filter: &IssueFilter) -> Result<IssueList> {
        let (issues, skipped) = self.load_issues()?;
        Ok(IssueList {
            listing: filter.select(issues),
            skipped,
        })
    }

    /// Every issue on disk, forgotten ones included, and the files that did
    /// not parse.
    pub fn issues(&self) -> Result<(Vec<Issue>, Vec<Skipped>)> {
        self.load_issues()
    }

    /// The issue with this id.
    ///
    /// # Errors
    ///
    /// `issue_not_found` naming the file looked for; a store error
    /// otherwise.
    pub fn issue(&self, id: IssueId) -> Result<Issue> {
        let path = self.issue_path(id);
        if !self.root_exists()?
            || !fsio::inspect_subdir(&self.root.join(ISSUES_DIR))?
            || !fsio::inspect_file(&path)?
        {
            return Err(refusal(
                WriteRefusal::IssueNotFound,
                format!(
                    "no issue {id}: {} does not exist; run 'scribe issue list' with the same --root to see existing ids",
                    path.display()
                ),
            ));
        }
        decode_issue(&path, &fsio::read(&path)?, &id.to_string())
    }

    /// The issue, and occurrence if any, that `input` names: `<issue-id>`
    /// or `<issue-id>#<n>`, the id exact or a unique prefix of at least
    /// four characters. The occurrence is checked to exist. Reads only.
    ///
    /// # Errors
    ///
    /// `issue_not_found`, `occurrence_not_found`, `id_ambiguous`, or a usage
    /// error for an input that is not an id.
    pub fn resolve_issue(&self, input: &str) -> Result<(Issue, Option<NonZeroU32>)> {
        let (id_part, n) = match input.split_once('#') {
            Some((id, n)) => {
                let parsed: IssueRef = format!("issue-1#{n}").parse()?;
                (id, parsed.occurrence)
            }
            None => (input, None),
        };
        let id = self.resolve_issue_id(id_part)?;
        let issue = self.issue(id)?;
        if let Some(n) = n
            && issue.occurrence(n).is_none()
        {
            return Err(issue.no_occurrence(n));
        }
        Ok((issue, n))
    }

    fn resolve_issue_id(&self, input: &str) -> Result<IssueId> {
        if let Ok(id) = input.parse::<IssueId>() {
            return Ok(id);
        }
        let ids: Vec<String> = self.issue_ids()?.iter().map(ToString::to_string).collect();
        match resolve(input, &ids)? {
            Some(i) => ids[i].parse(),
            None => Err(refusal(
                WriteRefusal::IssueNotFound,
                format!(
                    "no issue matches '{input}' under {}; run 'scribe issue list' with the same --root to see existing ids",
                    self.root.join(ISSUES_DIR).display()
                ),
            )),
        }
    }

    /// Add a new issue with its first occurrence.
    ///
    /// # Errors
    ///
    /// `invalid_text` for a field out of bounds; a store error otherwise.
    pub fn add_issue(
        &self,
        new: &NewIssue,
        occurrence: NewOccurrence,
        author: Author,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Issue> {
        let (title, signature, cause, fix) = new.check()?;
        let occurrence = occurrence.check(author.clone())?;
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("issue add")?;
        let ids = self.issue_ids()?;
        let mut issue = Issue {
            id: IssueId::next_in(&ids),
            title,
            status: IssueStatus::Open,
            signature,
            projects: Vec::new(),
            hosts: Vec::new(),
            topics: new.topics.clone(),
            related: Vec::new(),
            author: author.clone(),
            updated_by: None,
            created: now,
            updated: now,
            cause,
            fix,
            occurrences: Vec::new(),
            gone: Vec::new(),
        };
        issue.occur(occurrence, author, now);
        issue.updated_by = None;
        redactor.issue(&mut issue);
        self.write_issue(&issue, true)?;
        Ok(issue)
    }

    /// Add an occurrence to the issue `input` names. On a `resolved` issue
    /// it reopens the issue and is marked `recurred`.
    ///
    /// # Errors
    ///
    /// `issue_not_found`, `issue_forgotten`, `invalid_text`; a store error
    /// otherwise.
    pub fn occur_issue(
        &self,
        input: &str,
        occurrence: NewOccurrence,
        author: Author,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Issue, NonZeroU32)> {
        let mut occurrence = occurrence.check(author.clone())?;
        redactor.occurrence(&mut occurrence);
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("issue occur")?;
        let (mut issue, _) = self.resolve_issue(input)?;
        writable(&issue)?;
        let n = issue.occur(occurrence, author, now);
        self.write_issue(&issue, false)?;
        Ok((issue, n))
    }

    /// Change an issue's header cause, fix or status. Returns whether it
    /// changed; an unchanged issue is not written.
    ///
    /// # Errors
    ///
    /// `issue_not_found`, `issue_forgotten`, `invalid_text`; a store error
    /// otherwise.
    pub fn update_issue(
        &self,
        input: &str,
        patch: &IssuePatch,
        author: Author,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Issue, bool)> {
        patch.check()?;
        let masked = IssuePatch {
            cause: patch.cause.as_deref().map(|t| redactor.mask(t)),
            fix: patch.fix.as_deref().map(|t| redactor.mask(t)),
            status: patch.status,
        };
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("issue update")?;
        let (mut issue, _) = self.resolve_issue(input)?;
        writable(&issue)?;
        let changed = masked.apply(&mut issue)?;
        if changed {
            issue.touch(author, now);
            self.write_issue(&issue, false)?;
        }
        Ok((issue, changed))
    }

    /// Move occurrence `from` (`<issue-id>#<n>`) to another issue or a new
    /// one. It keeps its text and metadata and records `moved_from`; its old
    /// number is kept in the source as moved, naming where it went. The
    /// destination is written first, so a crash between the two writes
    /// leaves a copy rather than losing the occurrence.
    ///
    /// # Errors
    ///
    /// A usage error when `from` names no occurrence or the destination is
    /// the same issue; `issue_not_found`, `occurrence_not_found`,
    /// `issue_forgotten`, `invalid_text`; a store error otherwise.
    pub fn move_occurrence(
        &self,
        from: &str,
        to: &MoveTo,
        author: Author,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Moved> {
        let new_header = match to {
            MoveTo::New {
                title, signature, ..
            } => Some((
                crate::issue::NewIssue {
                    title: title.clone(),
                    signature: signature.clone(),
                    topics: Topics::default(),
                    cause: None,
                    fix: None,
                }
                .check()
                .map(|(title, ..)| title)?,
                check_signature(signature)?,
            )),
            MoveTo::Issue(_) => None,
        };
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("issue move")?;
        let (mut source, n) = self.resolve_issue(from)?;
        let Some(from_n) = n else {
            return Err(refusal(
                WriteRefusal::InvalidId,
                format!("'{from}' names no occurrence; give <issue-id>#<n>"),
            ));
        };
        writable(&source)?;
        let (mut target, new) = match (to, new_header) {
            (MoveTo::Issue(input), _) => {
                let (target, _) = self.resolve_issue(input)?;
                if target.id == source.id {
                    return Err(refusal(
                        WriteRefusal::InvalidId,
                        format!(
                            "{} is already in {}; name another issue or --new",
                            from, source.id
                        ),
                    ));
                }
                writable(&target)?;
                (target, false)
            }
            (MoveTo::New { topics, .. }, Some((title, signature))) => {
                let ids = self.issue_ids()?;
                let occurrence = source
                    .occurrence(from_n)
                    .ok_or_else(|| source.no_occurrence(from_n))?;
                let mut issue = Issue {
                    id: IssueId::next_in(&ids),
                    title,
                    status: IssueStatus::Open,
                    signature,
                    projects: Vec::new(),
                    hosts: Vec::new(),
                    topics: topics.clone(),
                    related: Vec::new(),
                    author: author.clone(),
                    updated_by: None,
                    created: now,
                    updated: now,
                    cause: occurrence.cause.clone(),
                    fix: occurrence.fix.clone(),
                    occurrences: Vec::new(),
                    gone: Vec::new(),
                };
                redactor.issue(&mut issue);
                (issue, true)
            }
            (MoveTo::New { .. }, None) => {
                return Err(refusal(
                    WriteRefusal::InvalidText,
                    "a new issue needs a title",
                ));
            }
        };
        let to_n = target.next_occurrence();
        let mut occurrence = source.take(
            from_n,
            Gone {
                n: from_n,
                reason: GoneReason::Moved,
                to: Some(format!("{}#{to_n}", target.id)),
            },
        )?;
        occurrence.n = to_n;
        occurrence.moved_from = Some(format!("{}#{from_n}", source.id));
        target.occurrences.push(occurrence);
        if new {
            target.refresh();
        } else {
            target.touch(author.clone(), now);
        }
        source.touch(author, now);
        self.write_issue(&target, new)?;
        self.write_issue(&source, false)?;
        Ok(Moved {
            from: source,
            from_n,
            to: target,
            to_n,
        })
    }

    /// Forget an issue, or one occurrence of it, for good (Daniel's
    /// `#scribe forget <issue-id>` or `<issue-id>#<n>`): its words leave the
    /// file, which is replaced whole; the id and the occurrence numbers stay,
    /// marked forgotten, so neither is given again. A forgotten occurrence's
    /// cause and fix also leave every header that holds them, here and in
    /// each issue it was moved from ([`Issue::unsay`]). Returns whether anything
    /// changed: forgetting what is already forgotten changes nothing.
    ///
    /// # Errors
    ///
    /// `issue_not_found`, `occurrence_not_found`; a store error otherwise.
    pub fn forget_issue(
        &self,
        target: IssueRef,
        by: Author,
        now: OffsetDateTime,
    ) -> Result<(Issue, bool)> {
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("issue forget")?;
        let mut issue = self.issue(target.issue)?;
        // The occurrences whose words go: their cause and fix also leave
        // every header they were copied into, here and along their moves.
        let going: Vec<_> = issue
            .occurrences
            .iter()
            .filter(|o| target.occurrence.is_none_or(|n| o.n == n))
            .map(|o| (o.n, o.cause.clone(), o.fix.clone()))
            .collect();
        let changed = match target.occurrence {
            None => issue.forget(by.clone(), now),
            Some(n) => {
                if issue.occurrence(n).is_none()
                    && issue
                        .gone
                        .iter()
                        .any(|g| g.n == n && g.reason == GoneReason::Forgotten)
                {
                    false
                } else {
                    issue.take(
                        n,
                        Gone {
                            n,
                            reason: GoneReason::Forgotten,
                            to: None,
                        },
                    )?;
                    for (_, cause, fix) in &going {
                        issue.unsay(cause, fix);
                    }
                    issue.touch(by.clone(), now);
                    true
                }
            }
        };
        if changed {
            self.write_issue(&issue, false)?;
            self.unsay_moved_from(issue.id, &going, &by, now)?;
        }
        Ok((issue, changed))
    }

    /// Take forgotten occurrences' words out of the headers of the issues
    /// they were moved from, following each move back (a `moved` gone
    /// entry naming `<issue>#<n>`) to where it was first written. The caller
    /// holds the lock. Issues that are unreadable are left alone.
    fn unsay_moved_from(
        &self,
        id: IssueId,
        going: &[(NonZeroU32, String, String)],
        by: &Author,
        now: OffsetDateTime,
    ) -> Result<()> {
        let (mut issues, _) = self.load_issues()?;
        let mut changed = std::collections::BTreeSet::new();
        for (n, cause, fix) in going {
            let mut at = vec![format!("{id}#{n}")];
            while let Some(to) = at.pop() {
                for other in &mut issues {
                    let from: Vec<_> = other
                        .gone
                        .iter()
                        .filter(|g| g.reason == GoneReason::Moved && g.to.as_deref() == Some(&to))
                        .map(|g| format!("{}#{}", other.id, g.n))
                        .collect();
                    if from.is_empty() {
                        continue;
                    }
                    if other.unsay(cause, fix) {
                        other.touch(by.clone(), now);
                        changed.insert(other.id);
                    }
                    at.extend(from);
                }
            }
        }
        for issue in issues.iter().filter(|i| changed.contains(&i.id)) {
            self.write_issue(issue, false)?;
        }
        Ok(())
    }

    /// Every issue id on disk, unreadable files included.
    fn issue_ids(&self) -> Result<Vec<IssueId>> {
        let dir = self.root.join(ISSUES_DIR);
        if !self.root_exists()? || !fsio::inspect_subdir(&dir)? {
            return Ok(Vec::new());
        }
        Ok(visible(fsio::list_dir(&dir)?)
            .filter_map(|name| name.strip_suffix(RECORD_EXT)?.parse().ok())
            .collect())
    }

    fn load_issues(&self) -> Result<(Vec<Issue>, Vec<Skipped>)> {
        let mut issues = Vec::new();
        let mut skipped = Vec::new();
        let dir = self.root.join(ISSUES_DIR);
        if !self.root_exists()? || !fsio::inspect_subdir(&dir)? {
            return Ok((issues, skipped));
        }
        for name in visible(fsio::list_dir(&dir)?) {
            let Some(stem) = name.strip_suffix(RECORD_EXT) else {
                continue;
            };
            let path = dir.join(&name);
            if !fsio::inspect_file(&path)? {
                continue;
            }
            match decode_issue(&path, &fsio::read(&path)?, stem) {
                Ok(issue) => issues.push(issue),
                Err(error @ Error::Corrupt { .. }) => skipped.push(Skipped { path, error }),
                Err(other) => return Err(other),
            }
        }
        Ok((issues, skipped))
    }

    /// Write `issue` whole. A new issue never replaces a file. The caller
    /// holds the lock.
    fn write_issue(&self, issue: &Issue, new: bool) -> Result<()> {
        self.prepare_subdir(&self.root.join(ISSUES_DIR))?;
        let path = self.issue_path(issue.id);
        if new && fsio::inspect_file(&path)? {
            return Err(Error::io(
                "create",
                path,
                io::Error::from(io::ErrorKind::AlreadyExists),
            ));
        }
        fsio::write_atomic(&path, &encode_issue(&path, issue)?)
    }
}

/// Refuse a write to a forgotten issue.
fn writable(issue: &Issue) -> Result<()> {
    if issue.status == IssueStatus::Forgotten {
        return Err(refusal(
            WriteRefusal::IssueForgotten,
            format!("{} was forgotten; add a new issue instead", issue.id),
        ));
    }
    Ok(())
}
