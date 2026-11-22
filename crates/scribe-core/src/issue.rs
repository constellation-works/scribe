//! Issues: the memory of an incident that recurs across projects and what
//! fixed it (docs/design/scribe/2_design.md §11.4).
//!
//! An issue is a record kind of its own, not an entry, because it grows: a
//! header (title, symptom signature, status, the best-known cause and fix)
//! and one occurrence per time it happened, newest first. The agent in the
//! chat writes it on Daniel's typed request, whose words each occurrence
//! keeps verbatim; nothing settles it, and Daniel removes a wrong occurrence
//! or a wrong issue with `#scribe forget`.
//!
//! This module holds the values and the rules, all pure (STD-02 R5): the
//! ids, the bounds every field is checked against, the reopen-on-recurrence
//! rule, a move between issues and a forget. The store persists it
//! (`store/issues.rs`, `issues/<id>.md`).

use crate::error::{Error, Result, WriteRefusal};
use crate::record::{Author, Label, Project, Related, Topics};
use crate::session::SessionId;
use crate::verbatim;
use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;
use time::{Date, OffsetDateTime};

/// The id prefix, and the tag name no entry may take: `#scribe issue:` and
/// `entry add --tag issue` are refused, naming the issue commands.
pub const ISSUE_TAG: &str = "issue";
/// The commands that write and read issues, for a refusal that names them.
pub const ISSUE_COMMANDS: &str = "scribe issue add|occur|update|move to write an issue, scribe issue search|show|list to read one";
/// The longest title, in characters.
pub const TITLE_CHARS: usize = 120;
/// The most symptom or error strings in a signature.
pub const MAX_SIGNATURES: usize = 5;
/// The longest signature string, in characters.
pub const SIGNATURE_CHARS: usize = 200;
/// The longest cause or fix, in characters: the header's and an
/// occurrence's.
pub const CAUSE_CHARS: usize = 600;
/// The longest context summary of an occurrence, in characters.
pub const SUMMARY_CHARS: usize = 1200;
/// The longest request, in characters.
pub const REQUEST_CHARS: usize = 1200;
/// The most error lines an occurrence keeps.
pub const MAX_ERROR_LINES: usize = 10;
/// The longest error line, in characters.
pub const ERROR_LINE_CHARS: usize = 300;
/// The most links an occurrence keeps, besides its session.
pub const MAX_LINKS: usize = 10;
/// What a cause reads when none was found.
pub const CAUSE_NOT_FOUND: &str = "cause not found";
/// What a fix reads when none was found.
pub const FIX_NOT_FOUND: &str = "fix not found";

/// An issue's id, `issue-<n>` with `n` from 1 and no leading zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IssueId(NonZeroU32);

impl IssueId {
    /// The issue numbered `n`.
    #[must_use]
    pub fn new(n: NonZeroU32) -> Self {
        Self(n)
    }

    /// Its number.
    #[must_use]
    pub fn number(self) -> u32 {
        self.0.get()
    }

    /// The id after the highest of `existing`, as entry ids are allocated.
    #[must_use]
    pub fn next_in<'a>(existing: impl IntoIterator<Item = &'a IssueId>) -> Self {
        let highest = existing
            .into_iter()
            .map(|id| id.number())
            .max()
            .unwrap_or(0);
        Self(NonZeroU32::new(highest.saturating_add(1)).unwrap_or(NonZeroU32::MAX))
    }
}

impl fmt::Display for IssueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{ISSUE_TAG}-{}", self.0)
    }
}

/// A number written as it round-trips: digits, no sign, no leading zero.
fn number(text: &str) -> Option<NonZeroU32> {
    if text.is_empty() || text.starts_with('0') || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl FromStr for IssueId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        s.strip_prefix("issue-")
            .and_then(number)
            .map(Self)
            .ok_or_else(|| {
                invalid_id(format!(
                    "'{s}' is not an issue id; issue ids look like issue-3"
                ))
            })
    }
}

/// An issue, or one occurrence of it: `issue-<n>` or `issue-<n>#<m>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IssueRef {
    /// The issue.
    pub issue: IssueId,
    /// The occurrence's number, or `None` for the whole issue.
    pub occurrence: Option<NonZeroU32>,
}

impl fmt::Display for IssueRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.occurrence {
            Some(n) => write!(f, "{}#{n}", self.issue),
            None => write!(f, "{}", self.issue),
        }
    }
}

impl FromStr for IssueRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let (issue, occurrence) = match s.split_once('#') {
            Some((issue, n)) => (
                issue,
                Some(number(n).ok_or_else(|| {
                    invalid_id(format!(
                        "'{s}' does not name an occurrence; use <issue-id>#<n>, such as issue-3#2"
                    ))
                })?),
            ),
            None => (s, None),
        };
        Ok(Self {
            issue: issue.parse()?,
            occurrence,
        })
    }
}

/// Where an issue stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IssueStatus {
    /// Happening, or not yet known to be fixed.
    Open,
    /// Worked around; the cause remains.
    Mitigated,
    /// Fixed. A later occurrence reopens it.
    Resolved,
    /// Removed by Daniel's `#scribe forget`: every word blanked, the id kept
    /// so it is never allocated again.
    Forgotten,
}

impl IssueStatus {
    /// The statuses `issue update --status` may set.
    pub const SETTABLE: [&'static str; 3] = ["open", "mitigated", "resolved"];
    /// Every status, for `issue list --status`.
    pub const NAMES: [&'static str; 4] = ["open", "mitigated", "resolved", "forgotten"];

    /// The canonical token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Mitigated => "mitigated",
            Self::Resolved => "resolved",
            Self::Forgotten => "forgotten",
        }
    }
}

impl fmt::Display for IssueStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IssueStatus {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "open" => Ok(Self::Open),
            "mitigated" => Ok(Self::Mitigated),
            "resolved" => Ok(Self::Resolved),
            "forgotten" => Ok(Self::Forgotten),
            other => Err(format!(
                "'{other}' is not an issue status; use open, mitigated, resolved or forgotten"
            )),
        }
    }
}

/// One time the incident happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    /// Its number within the issue, from 1, never reused.
    pub n: NonZeroU32,
    /// The day it happened.
    pub date: Date,
    /// The host it happened on.
    pub host: Label,
    /// The project it happened in.
    pub project: Project,
    /// The session Daniel asked in.
    pub session: SessionId,
    /// The uuid of his typed turn that holds `request`.
    pub request_message: String,
    /// Written while the issue was `resolved`: it came back after the fix.
    pub recurred: bool,
    /// Where it was moved from (`issue-<n>#<m>`), or `None`.
    pub moved_from: Option<String>,
    /// The agent that wrote it.
    pub author: Author,
    /// The agent's summary of the context.
    pub summary: String,
    /// Daniel's request, word for word from his typed turn.
    pub request: String,
    /// Key error lines: the agent's evidence, never checked against the
    /// transcript.
    pub error_lines: Vec<String>,
    /// The root cause found, or [`CAUSE_NOT_FOUND`].
    pub cause: String,
    /// The fix applied, or [`FIX_NOT_FOUND`].
    pub fix: String,
    /// Orbit tasks, PRs and other sessions it links to, besides `session`.
    pub links: Vec<Related>,
}

impl Occurrence {
    /// Its heading in the file and in a detail view:
    /// `<n>. <date> — <host>, <project>`.
    #[must_use]
    pub fn heading(&self) -> String {
        format!(
            "{}. {} — {}, {}",
            self.n,
            crate::record::DateRange::day(self.date),
            self.host,
            self.project
        )
    }
}

/// Why an occurrence number left its issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoneReason {
    /// Moved to another issue.
    Moved,
    /// Removed by Daniel's `#scribe forget`.
    Forgotten,
}

impl GoneReason {
    /// The canonical token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Moved => "moved",
            Self::Forgotten => "forgotten",
        }
    }
}

/// An occurrence number no longer in its issue, kept so the number is never
/// given again and a move can be followed. Ids only, never text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gone {
    /// The occurrence's number.
    pub n: NonZeroU32,
    /// Why it left.
    pub reason: GoneReason,
    /// For a move, where it went (`issue-<n>#<m>`).
    pub to: Option<String>,
}

/// An issue record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// Its id.
    pub id: IssueId,
    /// A one-line title.
    pub title: String,
    /// Where it stands.
    pub status: IssueStatus,
    /// The symptom or error strings a later search matches, 1 to 5.
    pub signature: Vec<String>,
    /// Every occurrence's project, sorted.
    pub projects: Vec<Project>,
    /// Every occurrence's host, sorted.
    pub hosts: Vec<Label>,
    /// Topics given when it was added.
    pub topics: Topics,
    /// Every occurrence's session and links, sorted.
    pub related: Vec<Related>,
    /// The agent that added it.
    pub author: Author,
    /// Who changed it last: an agent, or Daniel after his forget; `None`
    /// until changed.
    pub updated_by: Option<Author>,
    /// When it was added.
    pub created: OffsetDateTime,
    /// When it last changed.
    pub updated: OffsetDateTime,
    /// The best-known cause, or [`CAUSE_NOT_FOUND`].
    pub cause: String,
    /// The current best fix or mitigation, or [`FIX_NOT_FOUND`].
    pub fix: String,
    /// Its occurrences, newest (highest number) first.
    pub occurrences: Vec<Occurrence>,
    /// Occurrence numbers moved out or forgotten.
    pub gone: Vec<Gone>,
}

impl Issue {
    /// The occurrence numbered `n`.
    #[must_use]
    pub fn occurrence(&self, n: NonZeroU32) -> Option<&Occurrence> {
        self.occurrences.iter().find(|o| o.n == n)
    }

    /// The number the next occurrence takes: one past every number it has
    /// held, so a moved or forgotten number is never reused.
    #[must_use]
    pub fn next_occurrence(&self) -> NonZeroU32 {
        let highest = self
            .occurrences
            .iter()
            .map(|o| o.n.get())
            .chain(self.gone.iter().map(|g| g.n.get()))
            .max()
            .unwrap_or(0);
        NonZeroU32::new(highest.saturating_add(1)).unwrap_or(NonZeroU32::MAX)
    }

    /// Order the occurrences newest first and derive `projects`, `hosts`
    /// and `related` from them.
    pub fn refresh(&mut self) {
        self.occurrences.sort_by_key(|o| std::cmp::Reverse(o.n));
        self.gone.sort_by_key(|g| g.n);
        let mut projects: Vec<Project> =
            self.occurrences.iter().map(|o| o.project.clone()).collect();
        projects.sort();
        projects.dedup();
        self.projects = projects;
        let mut hosts: Vec<Label> = self.occurrences.iter().map(|o| o.host.clone()).collect();
        hosts.sort();
        hosts.dedup();
        self.hosts = hosts;
        let mut related: Vec<Related> = Vec::new();
        for o in &self.occurrences {
            if let Ok(session) = format!("session:{}", o.session).parse::<Related>() {
                related.push(session);
            }
            related.extend(o.links.iter().cloned());
        }
        related.sort();
        related.dedup();
        self.related = related;
    }

    /// Add `occurrence`, numbered next. On a `resolved` issue it reopens the
    /// issue and is marked `recurred` (§11.4). Returns its number.
    pub fn occur(
        &mut self,
        mut occurrence: Occurrence,
        by: Author,
        now: OffsetDateTime,
    ) -> NonZeroU32 {
        occurrence.n = self.next_occurrence();
        if self.status == IssueStatus::Resolved {
            self.status = IssueStatus::Open;
            occurrence.recurred = true;
        }
        let n = occurrence.n;
        self.occurrences.push(occurrence);
        self.touch(by, now);
        n
    }

    /// Take occurrence `n` out, recording where it went, and return it.
    ///
    /// # Errors
    ///
    /// `occurrence_not_found` when the issue has no occurrence `n`.
    pub fn take(&mut self, n: NonZeroU32, gone: Gone) -> Result<Occurrence> {
        let at = self
            .occurrences
            .iter()
            .position(|o| o.n == n)
            .ok_or_else(|| self.no_occurrence(n))?;
        let occurrence = self.occurrences.remove(at);
        self.gone.push(gone);
        Ok(occurrence)
    }

    /// Blank every word of the issue for good (Daniel's `#scribe forget
    /// <issue-id>`): its title, signature, cause, fix, topics and every
    /// occurrence go; the id, author and dates stay, and each occurrence
    /// number is kept as forgotten. Returns whether anything changed.
    pub fn forget(&mut self, by: Author, now: OffsetDateTime) -> bool {
        if self.status == IssueStatus::Forgotten {
            return false;
        }
        for occurrence in std::mem::take(&mut self.occurrences) {
            self.gone.push(Gone {
                n: occurrence.n,
                reason: GoneReason::Forgotten,
                to: None,
            });
        }
        self.status = IssueStatus::Forgotten;
        self.title.clear();
        self.signature.clear();
        self.cause.clear();
        self.fix.clear();
        self.topics = Topics::default();
        self.touch(by, now);
        true
    }

    /// Take a forgotten occurrence's `cause` and `fix` out of the header,
    /// which `issue add` and `issue move --new` copy from it: a header cause
    /// or fix that holds them falls back to the newest remaining
    /// occurrence's own that does not, else to the not-found text. Returns
    /// whether the header changed.
    pub fn unsay(&mut self, cause: &str, fix: &str) -> bool {
        let holds = |header: &str, words: &str, missing: &str| {
            !words.is_empty() && words != missing && header.contains(words)
        };
        let mut changed = false;
        if holds(&self.cause, cause, CAUSE_NOT_FOUND) {
            self.cause = self
                .occurrences
                .iter()
                .map(|o| o.cause.as_str())
                .find(|c| *c != CAUSE_NOT_FOUND && !holds(c, cause, CAUSE_NOT_FOUND))
                .unwrap_or(CAUSE_NOT_FOUND)
                .to_owned();
            changed = true;
        }
        if holds(&self.fix, fix, FIX_NOT_FOUND) {
            self.fix = self
                .occurrences
                .iter()
                .map(|o| o.fix.as_str())
                .find(|f| *f != FIX_NOT_FOUND && !holds(f, fix, FIX_NOT_FOUND))
                .unwrap_or(FIX_NOT_FOUND)
                .to_owned();
            changed = true;
        }
        changed
    }

    /// Record a change by `by` at `now` and derive the header lists again.
    pub fn touch(&mut self, by: Author, now: OffsetDateTime) {
        self.updated = now;
        self.updated_by = Some(by);
        self.refresh();
    }

    /// The `occurrence_not_found` refusal for `n`.
    #[must_use]
    pub fn no_occurrence(&self, n: NonZeroU32) -> Error {
        let moved = self
            .gone
            .iter()
            .find(|g| g.n == n)
            .map(|g| match (&g.reason, &g.to) {
                (GoneReason::Moved, Some(to)) => format!(" (it was moved to {to})"),
                (GoneReason::Moved | GoneReason::Forgotten, _) => {
                    format!(" (it was {})", g.reason.as_str())
                }
            })
            .unwrap_or_default();
        refusal(
            WriteRefusal::OccurrenceNotFound,
            format!(
                "{} has no occurrence {n}{moved}; run 'scribe issue show {}' to see its occurrences",
                self.id, self.id
            ),
        )
    }
}

/// A new occurrence's text and where it happened, before checks and
/// redaction. Its number is given when it is added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewOccurrence {
    /// The day it happened.
    pub date: Date,
    /// The host.
    pub host: Label,
    /// The project.
    pub project: Project,
    /// The session Daniel asked in.
    pub session: SessionId,
    /// The uuid of his turn holding the request.
    pub request_message: String,
    /// His request, word for word.
    pub request: String,
    /// The context summary.
    pub summary: String,
    /// Key error lines.
    pub error_lines: Vec<String>,
    /// The root cause, if found.
    pub cause: Option<String>,
    /// The fix, if found.
    pub fix: Option<String>,
    /// Links to tasks, PRs and other sessions.
    pub links: Vec<Related>,
}

impl NewOccurrence {
    /// The occurrence it becomes once checked and normalized: one-line
    /// fields flattened, a missing cause or fix "not found". Numbered 1
    /// until added.
    ///
    /// # Errors
    ///
    /// `invalid_text` for an empty or over-long field, a line that would
    /// break the file's sections, or more error lines or links than allowed.
    pub fn check(self, author: Author) -> Result<Occurrence> {
        let request = one_line("--request", &self.request, REQUEST_CHARS)?;
        let summary = block("--summary", &self.summary, SUMMARY_CHARS, "### ")?;
        if summary.is_empty() {
            return Err(text_refusal("--summary must not be empty"));
        }
        if self.error_lines.len() > MAX_ERROR_LINES {
            return Err(text_refusal(format!(
                "at most {MAX_ERROR_LINES} error lines; keep the key ones"
            )));
        }
        let mut error_lines = Vec::with_capacity(self.error_lines.len());
        for line in &self.error_lines {
            if line.contains(['\n', '\r']) {
                return Err(text_refusal(
                    "an error line must be one line; pass each as its own --error-line",
                ));
            }
            if line.trim().is_empty() {
                return Err(text_refusal("an error line must not be empty"));
            }
            if line.chars().count() > ERROR_LINE_CHARS {
                return Err(text_refusal(format!(
                    "an error line must contain at most {ERROR_LINE_CHARS} characters"
                )));
            }
            error_lines.push(line.trim_end().to_owned());
        }
        let mut links = self.links;
        let own = format!("session:{}", self.session);
        links.retain(|l| l.to_string() != own);
        links.sort();
        links.dedup();
        if links.len() > MAX_LINKS {
            return Err(text_refusal(format!("at most {MAX_LINKS} links")));
        }
        if self.request_message.trim().is_empty() {
            return Err(text_refusal("the request's message id must not be empty"));
        }
        Ok(Occurrence {
            n: NonZeroU32::MIN,
            date: self.date,
            host: self.host,
            project: self.project,
            session: self.session,
            request_message: self.request_message,
            recurred: false,
            moved_from: None,
            author,
            summary,
            request,
            error_lines,
            cause: optional_line("--cause", self.cause.as_deref(), CAUSE_NOT_FOUND)?,
            fix: optional_line("--fix", self.fix.as_deref(), FIX_NOT_FOUND)?,
            links,
        })
    }
}

/// A new issue's header, before checks and redaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewIssue {
    /// The title.
    pub title: String,
    /// The signature strings.
    pub signature: Vec<String>,
    /// Topics.
    pub topics: Topics,
    /// The best-known cause, if any.
    pub cause: Option<String>,
    /// The best fix, if any.
    pub fix: Option<String>,
}

impl NewIssue {
    /// The checked title, signature, cause and fix.
    ///
    /// # Errors
    ///
    /// `invalid_text` for an empty or over-long title, a signature of the
    /// wrong size, or a cause or fix that is too long or holds a `## ` line.
    pub fn check(&self) -> Result<(String, Vec<String>, String, String)> {
        let title = one_line("--title", &self.title, TITLE_CHARS)?;
        let signature = check_signature(&self.signature)?;
        let cause = header_text("--cause", self.cause.as_deref(), CAUSE_NOT_FOUND)?;
        let fix = header_text("--fix", self.fix.as_deref(), FIX_NOT_FOUND)?;
        Ok((title, signature, cause, fix))
    }
}

/// What `issue update` changes. Every field is optional; one must be set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssuePatch {
    /// A new header cause.
    pub cause: Option<String>,
    /// A new header fix.
    pub fix: Option<String>,
    /// A new status: open, mitigated or resolved.
    pub status: Option<IssueStatus>,
}

impl IssuePatch {
    /// Check the patch.
    ///
    /// # Errors
    ///
    /// `invalid_text` for an empty patch, a cause or fix out of bounds, or
    /// a `forgotten` status, which only `#scribe forget` sets.
    pub fn check(&self) -> Result<()> {
        if self.cause.is_none() && self.fix.is_none() && self.status.is_none() {
            return Err(text_refusal(
                "nothing to update; pass --cause, --fix or --status",
            ));
        }
        if self.status == Some(IssueStatus::Forgotten) {
            return Err(text_refusal(
                "an issue is forgotten only by the user's #scribe forget <issue-id>; use open, mitigated or resolved",
            ));
        }
        if let Some(cause) = &self.cause {
            header_text("--cause", Some(cause), CAUSE_NOT_FOUND)?;
        }
        if let Some(fix) = &self.fix {
            header_text("--fix", Some(fix), FIX_NOT_FOUND)?;
        }
        Ok(())
    }

    /// Apply it to `issue`; returns whether anything changed. The caller has
    /// redacted the text, which may have lengthened it, so its bounds are
    /// checked again; on a refusal `issue` is unchanged.
    ///
    /// # Errors
    ///
    /// `invalid_text` naming `--cause` or `--fix` when the redacted text is
    /// out of bounds.
    pub fn apply(&self, issue: &mut Issue) -> Result<bool> {
        let cause = match &self.cause {
            Some(cause) => Some(header_text("--cause", Some(cause), CAUSE_NOT_FOUND)?),
            None => None,
        };
        let fix = match &self.fix {
            Some(fix) => Some(header_text("--fix", Some(fix), FIX_NOT_FOUND)?),
            None => None,
        };
        let before = (issue.cause.clone(), issue.fix.clone(), issue.status);
        if let Some(cause) = cause {
            issue.cause = cause;
        }
        if let Some(fix) = fix {
            issue.fix = fix;
        }
        if let Some(status) = self.status {
            issue.status = status;
        }
        Ok(before != (issue.cause.clone(), issue.fix.clone(), issue.status))
    }
}

/// Which issues a listing keeps. Every field set must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueFilter {
    /// Only issues with this status; `None` keeps every status but
    /// `forgotten`.
    pub status: Option<IssueStatus>,
    /// Only issues with an occurrence in this project.
    pub project: Option<Project>,
    /// Only issues with an occurrence on this host.
    pub host: Option<Label>,
    /// At most this many, newest first.
    pub limit: Option<std::num::NonZeroUsize>,
}

impl IssueFilter {
    /// Whether `issue` passes.
    #[must_use]
    pub fn matches(&self, issue: &Issue) -> bool {
        let status = match self.status {
            Some(status) => issue.status == status,
            None => issue.status != IssueStatus::Forgotten,
        };
        status
            && self
                .project
                .as_ref()
                .is_none_or(|p| issue.projects.contains(p))
            && self.host.as_ref().is_none_or(|h| issue.hosts.contains(h))
    }

    /// The matching issues, most recently updated first, and how many
    /// matched before the limit.
    #[must_use]
    pub fn select(&self, issues: Vec<Issue>) -> crate::Listing<Issue> {
        let mut items: Vec<Issue> = issues.into_iter().filter(|i| self.matches(i)).collect();
        items.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
        let total = items.len();
        if let Some(limit) = self.limit {
            items.truncate(limit.get());
        }
        crate::Listing { items, total }
    }
}

/// Check a signature: 1 to [`MAX_SIGNATURES`] distinct one-line strings of
/// at most [`SIGNATURE_CHARS`] characters.
///
/// # Errors
///
/// `invalid_text` when it is not.
pub fn check_signature(signature: &[String]) -> Result<Vec<String>> {
    if signature.is_empty() || signature.len() > MAX_SIGNATURES {
        return Err(text_refusal(format!(
            "give 1 to {MAX_SIGNATURES} --signature strings: the symptoms or error text a later search matches"
        )));
    }
    let mut out: Vec<String> = Vec::with_capacity(signature.len());
    for item in signature {
        let item = one_line("--signature", item, SIGNATURE_CHARS)?;
        if !out.contains(&item) {
            out.push(item);
        }
    }
    Ok(out)
}

/// Whether `word`, as a tag name typed in an entry write or a `#scribe`
/// line, is the reserved issue tag.
#[must_use]
pub fn is_reserved_tag(word: &str) -> bool {
    word.eq_ignore_ascii_case(ISSUE_TAG)
}

/// The refusal of an entry write under the reserved issue tag.
#[must_use]
pub fn reserved_tag() -> Error {
    refusal(
        WriteRefusal::ReservedTag,
        format!("'{ISSUE_TAG}' is reserved for issue records, not entries; use {ISSUE_COMMANDS}"),
    )
}

fn refusal(kind: WriteRefusal, message: impl Into<String>) -> Error {
    Error::AgentWrite {
        kind,
        message: message.into(),
    }
}

fn text_refusal(message: impl Into<String>) -> Error {
    refusal(WriteRefusal::InvalidText, message)
}

fn invalid_id(message: String) -> Error {
    refusal(WriteRefusal::InvalidId, message)
}

/// `text` on one line (every whitespace run one space, ends trimmed), not
/// empty and at most `max` characters.
fn one_line(flag: &str, text: &str, max: usize) -> Result<String> {
    let flat = verbatim::normalize(text).trim().to_owned();
    if flat.is_empty() {
        return Err(text_refusal(format!("{flag} must not be empty")));
    }
    if flat.chars().count() > max {
        return Err(text_refusal(format!(
            "{flag} must contain at most {max} characters"
        )));
    }
    Ok(flat)
}

/// An occurrence's cause or fix: one line, or `missing` when absent.
fn optional_line(flag: &str, text: Option<&str>, missing: &str) -> Result<String> {
    match text {
        None => Ok(missing.to_owned()),
        Some(text) => one_line(flag, text, CAUSE_CHARS),
    }
}

/// A header cause or fix: lines kept, ends trimmed, at most
/// [`CAUSE_CHARS`] characters, no `## ` line; `missing` when absent.
fn header_text(flag: &str, text: Option<&str>, missing: &str) -> Result<String> {
    match text {
        None => Ok(missing.to_owned()),
        Some(text) => {
            let kept = block(flag, text, CAUSE_CHARS, "## ")?;
            if kept.is_empty() {
                return Err(text_refusal(format!("{flag} must not be empty")));
            }
            Ok(kept)
        }
    }
}

/// Multi-line text: `\r\n` as `\n`, ends trimmed, at most `max`
/// characters, and no line starting with `heading` (or `## `), which would
/// open a section of the file.
fn block(flag: &str, text: &str, max: usize, heading: &str) -> Result<String> {
    let kept = text.replace("\r\n", "\n").trim().to_owned();
    if kept.chars().count() > max {
        return Err(text_refusal(format!(
            "{flag} must contain at most {max} characters"
        )));
    }
    if kept
        .lines()
        .any(|line| line.starts_with(heading) || line.starts_with("## "))
    {
        return Err(text_refusal(format!(
            "{flag} must not hold a line starting with '{}' or '## ': it would open a section of the issue file",
            heading.trim_end()
        )));
    }
    Ok(kept)
}
