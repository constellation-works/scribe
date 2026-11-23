//! Issue records left by an older build become `issue` entries
//! (docs/design/scribe/2_design.md §15.11), model-free, and their files move
//! aside to `pre-issue-<date>/`.
//!
//! The issue record kind was removed in §15.9 phase 2: an incident is now an
//! ordinary `issue` entry (a catcher's collected entry, or an agent's). This
//! module only reads `issues/<id>.md` at the shape formats 17 to 23 wrote,
//! once, inside a write's transaction: each issue that kept its text becomes one
//! entry, written by the agent that wrote the issue:
//!
//! - content: the title, then the best-known cause and fix;
//! - quote: the user's verified request of its first occurrence, word for
//!   word;
//! - related: the issue's links, then each occurrence's session not
//!   already among them;
//! - project and host: those of its first occurrence.
//!
//! An issue the user forgot has no text left, so it writes nothing. Each
//! file moves aside after its entry is written, so a crash between the two
//! leaves the file, and the next run finds the entry already there (same
//! tag, content and quote) and only moves it. An issue file that does not
//! parse stays in `issues/` and is tried again on the next write.

use super::db::{self, DB_FILE, docs};
use super::documents::{encode_hex_32, matches_issue_source};
use super::format::{parse_frontmatter, split_frontmatter};
use super::fsio;
use super::{ENTRIES_DIR, RECORD_EXT, Skipped, Store, visible};
use crate::entry::{ContentFrom, Entry, EntryId, EntryStatus, TagName};
use crate::error::{Error, Result};
use crate::record::{Author, AuthorKind, Label, Project, Related, Topics};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// Where issue records lived.
pub(super) const ISSUES_DIR: &str = "issues";
const ARCHIVE_PREFIX: &str = "pre-issue-";
/// The tag a converted issue is filed under.
pub(super) const ISSUE_TAG: &str = "issue";
/// What an issue record said when no cause or fix was known.
const CAUSE_NOT_FOUND: &str = "cause not found";
const FIX_NOT_FOUND: &str = "fix not found";
/// The request line's openings, at format 23 and before it.
const ASKED: [&str; 2] = ["**The user asked:** \"", "**Daniel asked:** \""];
const OCCURRENCES: &str = "## Occurrences";
const OCCURRENCE_CAUSE: &str = "**Cause:** ";
const OCCURRENCE_FIX: &str = "**Fix:** ";

/// The frontmatter keys this import reads; the rest of a strict issue
/// file's keys are accepted and ignored.
#[derive(Deserialize)]
struct Front {
    title: String,
    status: String,
    topics: Topics,
    related: Vec<Related>,
    author: Author,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    occurrences: Vec<OccurrenceFront>,
}

#[derive(Deserialize)]
struct OccurrenceFront {
    n: NonZeroU32,
    host: Label,
    project: Project,
    /// The session the occurrence was asked in; `None` before format 17
    /// kept it.
    #[serde(default)]
    session: Option<String>,
}

/// One occurrence's text, as the body holds it.
struct OccurrenceText {
    request: String,
    cause: String,
    fix: String,
}

/// What one issue file converts to: `None` for a forgotten issue.
struct Converted {
    number: NonZeroU32,
    entry: Option<Entry>,
}

/// A legacy issue file and the ordinary entry it imports as.
pub(super) struct IssueSource {
    pub(super) path: PathBuf,
    pub(super) entry: Entry,
    /// Identifies the original filename and file contents across renames.
    pub(super) token: String,
}

impl Store {
    /// Convert every issue record in `issues/` to an `issue` entry and move
    /// its file to `pre-issue-<date>/` after the write commits, `now` dating
    /// both. Runs inside the write's transaction. A no-op when `issues/` is absent or empty.
    ///
    /// # Errors
    ///
    /// A store error reading entries or writing one; an issue file that
    /// does not parse is left in place, not an error.
    pub(super) fn convert_issues(&self, conn: &db::Conn, now: OffsetDateTime) -> Result<()> {
        let dir = self.root.join(ISSUES_DIR);
        if !self.root_exists()? || !fsio::inspect_subdir(&dir)? {
            return Ok(());
        }
        let names: Vec<String> = visible(fsio::list_dir(&dir)?)
            .filter(|name| name.ends_with(RECORD_EXT))
            .collect();
        if names.is_empty() {
            return Ok(());
        }
        let db_path = self.root.join(DB_FILE);
        let pending = docs::pending_archive_forgets(conn, &db_path)?;
        let tag = TagName::try_from(ISSUE_TAG.to_owned())
            .map_err(|e| Error::corrupt(self.root.join(ENTRIES_DIR), e))?;
        let (mut entries, skipped) = self.load_entries()?;
        if !skipped.is_empty() {
            // Without every entry, an id or an earlier conversion could be
            // missed; the next write tries again.
            tracing::warn!(
                skipped = skipped.len(),
                "not converting issue records: some entries are unreadable"
            );
            return Ok(());
        }
        let mut ids = entry_ids(&entries, &skipped);
        ids.extend(self.candidate_reserved_ids()?);
        let aside = self.root.join(format!(
            "{ARCHIVE_PREFIX}{}",
            crate::record::DateRange::day(now.date())
        ));
        for name in names {
            let path = dir.join(&name);
            if !fsio::inspect_file(&path)? {
                continue;
            }
            let bytes = fsio::read(&path)?;
            let converted = match convert(&path, &bytes, &tag) {
                Ok(converted) => converted,
                Err(error @ Error::Corrupt { .. }) => {
                    tracing::warn!(path = %path.display(), %error, "issue record not converted");
                    continue;
                }
                Err(other) => return Err(other),
            };
            let has_erasure = !pending.is_empty() && {
                let token = source_token(&name, &bytes);
                pending
                    .iter()
                    .any(|item| matches_issue_source(item, &token))
            };
            if let Some(mut entry) = converted.entry
                && !has_erasure
            {
                let done = entries.iter().any(|e| {
                    e.tag == entry.tag && e.content == entry.content && e.quote == entry.quote
                });
                if !done {
                    // The issue's own number when it is free, so an old
                    // `issue-<n>` cite names the entry it became.
                    let own = EntryId::new(tag.clone(), NonZeroU64::from(converted.number));
                    entry.id = if ids.contains(&own) {
                        EntryId::next_in(&tag, &ids)
                    } else {
                        own
                    };
                    entry.review_after = self
                        .config_only()
                        .ok()
                        .and_then(|config| config.tags.get(&tag).cloned())
                        .and_then(|def| def.review_after(entry.created.date()));
                    self.write_entry(&entry, true)?;
                    ids.push(entry.id.clone());
                    entries.push(entry);
                }
            }
            // The file moves once the entry commits; a failed commit leaves
            // it in `issues/` for the next write.
            let to = aside.join(&name);
            let from = path.clone();
            if !self.after(|after| after.renames.push((from, to.clone()))) {
                self.prepare_subdir(&aside)?;
                fsio::rename(&path, &to)?;
            }
        }
        Ok(())
    }

    /// Include live legacy files whose conversion may still be waiting for
    /// its post-commit rename.
    pub(super) fn has_issue_sources(&self) -> Result<bool> {
        Ok(fsio::list_dir(&self.root)?
            .iter()
            .any(|name| name == ISSUES_DIR || name.starts_with(ARCHIVE_PREFIX)))
    }

    /// Identify the source of an imported issue before erasing the entry.
    /// An edited entry still has its original quote and creation metadata.
    /// When multiple distinct sources match, the original body can
    /// disambiguate them. Otherwise refuse to erase any of them.
    pub(super) fn issue_forget_tokens(&self, entry: &Entry) -> Result<Vec<String>> {
        if entry.tag.as_str() != ISSUE_TAG
            || entry.content_from != ContentFrom::Agent
            || entry.author.kind != AuthorKind::Agent
            || entry.stated_by.is_some()
            || entry.quote.is_none()
        {
            return Ok(Vec::new());
        }
        let mut sources: Vec<_> = self
            .issue_sources()?
            .into_iter()
            .filter(|source| {
                let original = &source.entry;
                entry.author == original.author
                    && entry.created == original.created
                    && entry.host == original.host
                    && entry.project == original.project
                    && entry.quote == original.quote
            })
            .collect();
        if let Some(first) = sources.first()
            && sources
                .iter()
                .any(|s| s.entry.content != first.entry.content)
        {
            if entry.updated_by.is_none()
                && sources.iter().any(|s| s.entry.content == entry.content)
            {
                sources.retain(|s| s.entry.content == entry.content);
            } else {
                return Err(Error::corrupt(
                    &first.path,
                    format!(
                        "cannot identify the original issue file for {}: multiple archived issues match its creation metadata and quote",
                        entry.id
                    ),
                ));
            }
        }
        Ok(sources.into_iter().map(|source| source.token).collect())
    }

    /// Use the same parser as migration, which can assign an entry a
    /// different number than the original issue's.
    pub(super) fn issue_sources(&self) -> Result<Vec<IssueSource>> {
        let tag =
            TagName::try_from(ISSUE_TAG.to_owned()).map_err(|e| Error::corrupt(&self.root, e))?;
        let mut entries = Vec::new();
        // A concurrent move goes from issues/ to a pre-issue-* directory.
        // Read the source directory before listing the destinations so a
        // move to a newly created archive cannot hide the source from both.
        read_issue_sources(&self.root.join(ISSUES_DIR), &tag, &mut entries)?;
        for name in fsio::list_dir(&self.root)?
            .into_iter()
            .filter(|n| n.starts_with(ARCHIVE_PREFIX))
        {
            read_issue_sources(&self.root.join(name), &tag, &mut entries)?;
        }
        Ok(entries)
    }
}

fn read_issue_sources(dir: &Path, tag: &TagName, sources: &mut Vec<IssueSource>) -> Result<()> {
    if !fsio::inspect_subdir(dir)? {
        return Ok(());
    }
    for name in visible(fsio::list_dir(dir)?).filter(|n| n.ends_with(RECORD_EXT)) {
        let path = dir.join(&name);
        if !fsio::inspect_file(&path)? {
            continue;
        }
        let bytes = fsio::read(&path)?;
        match convert(&path, &bytes, tag) {
            Ok(Converted {
                entry: Some(entry), ..
            }) => {
                sources.push(IssueSource {
                    path,
                    entry,
                    token: source_token(&name, &bytes),
                });
            }
            Ok(Converted { entry: None, .. }) | Err(Error::Corrupt { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn source_token(name: &str, bytes: &[u8]) -> String {
    // Keep this identifier distinct from free text in the salted obligation.
    let digest = encode_hex_32(&Sha256::digest(bytes).into());
    format!("scribe-legacy-issue\0{name}\0{digest}")
}

fn entry_ids(entries: &[Entry], skipped: &[Skipped]) -> Vec<EntryId> {
    entries
        .iter()
        .map(|e| e.id.clone())
        .chain(
            skipped
                .iter()
                .filter_map(|s| s.path.file_stem()?.to_str()?.parse().ok()),
        )
        .collect()
}

/// Read one issue file into the entry it becomes. Its id stays a
/// placeholder until the caller picks a free one.
fn convert(path: &Path, bytes: &[u8], tag: &TagName) -> Result<Converted> {
    let bad = |message: &str| Error::corrupt(path, message.to_owned());
    let (front, body) = split_frontmatter(path, bytes)?;
    let (_, value) = parse_frontmatter(path, front, ISSUE_TAG)?;
    let front: Front = yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?;
    let number = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.strip_prefix("issue-"))
        .and_then(|n| n.parse::<NonZeroU32>().ok())
        .ok_or_else(|| bad("an issue file is named issue-<n>.md"))?;
    if front.status == "forgotten" {
        return Ok(Converted {
            number,
            entry: None,
        });
    }
    if front.author.kind != AuthorKind::Agent {
        return Err(bad("an issue's author must be an agent"));
    }
    let (header_cause, header_fix, texts) = split_body(body).ok_or_else(|| bad("an issue body"))?;
    if texts.len() != front.occurrences.len() {
        return Err(bad("as many occurrences in the body as in the frontmatter"));
    }
    // Newest first in the file; the first occurrence holds the request
    // that opened the issue.
    let first_meta = front
        .occurrences
        .iter()
        .min_by_key(|o| o.n)
        .ok_or_else(|| bad("at least one occurrence"))?;
    let first_text = front
        .occurrences
        .iter()
        .zip(&texts)
        .min_by_key(|(o, _)| o.n)
        .map(|(_, t)| t)
        .ok_or_else(|| bad("at least one occurrence"))?;
    // The best-known cause and fix: the header's, else the newest
    // occurrence that found one.
    let known = |header: &str, missing: &str, pick: fn(&OccurrenceText) -> &str| {
        if header != missing {
            return Some(header.to_owned());
        }
        texts
            .iter()
            .map(pick)
            .find(|text| *text != missing)
            .map(str::to_owned)
    };
    let cause = known(&header_cause, CAUSE_NOT_FOUND, |t| &t.cause);
    let fix = known(&header_fix, FIX_NOT_FOUND, |t| &t.fix);
    let mut related = front.related;
    for session in front
        .occurrences
        .iter()
        .filter_map(|o| o.session.as_deref())
    {
        let link: Related = format!("session:{session}")
            .parse()
            .map_err(|e| Error::corrupt(path, e))?;
        if !related.contains(&link) {
            related.push(link);
        }
    }
    let mut content = front.title.trim().to_owned();
    content.push_str(&format!(
        "\nCause: {}\nFix: {}",
        cause.as_deref().unwrap_or(CAUSE_NOT_FOUND),
        fix.as_deref().unwrap_or(FIX_NOT_FOUND)
    ));
    let entry = Entry {
        id: EntryId::new(tag.clone(), NonZeroU64::MIN),
        tag: tag.clone(),
        status: EntryStatus::Active,
        project: first_meta.project.clone(),
        topics: front.topics,
        related,
        provider: Label::sanitized("cli", "cli"),
        host: first_meta.host.clone(),
        author: front.author,
        updated_by: None,
        created: front.created,
        updated: front.updated,
        inactive_since: None,
        said: String::new(),
        content_from: ContentFrom::Agent,
        cite: None,
        retirement_reason: None,
        superseded_by: None,
        review_after: None,
        quote: Some(first_text.request.clone()),
        window: None,
        evidence: None,
        as_of: None,
        approved_by: None,
        instruction: None,
        removal_instruction: None,
        stated_by: None,
        handoff_mode: None,
        outcome: None,
        content,
    };
    Ok(Converted {
        number,
        entry: Some(entry),
    })
}

/// The header's cause and fix and each occurrence's request, cause and
/// fix, in file order; `None` when the body is not an issue body.
fn split_body(body: &str) -> Option<(String, String, Vec<OccurrenceText>)> {
    let lines: Vec<&str> = body.split('\n').collect();
    let at = |heading: &str| lines.iter().position(|l| *l == heading);
    let (fix_at, occurrences_at) = (at("## Fix")?, at(OCCURRENCES)?);
    if lines.first() != Some(&"## Cause") || fix_at > occurrences_at {
        return None;
    }
    let cause = lines[1..fix_at].join("\n").trim().to_owned();
    let fix = lines[fix_at + 1..occurrences_at]
        .join("\n")
        .trim()
        .to_owned();
    let mut texts: Vec<OccurrenceText> = Vec::new();
    for line in &lines[occurrences_at + 1..] {
        if line.starts_with("### ") {
            texts.push(OccurrenceText {
                request: String::new(),
                cause: String::new(),
                fix: String::new(),
            });
            continue;
        }
        let current = texts.last_mut()?;
        if let Some(request) = ASKED
            .iter()
            .find_map(|asked| line.strip_prefix(asked))
            .and_then(|rest| rest.strip_suffix('"'))
        {
            request.clone_into(&mut current.request);
        } else if let Some(cause) = line.strip_prefix(OCCURRENCE_CAUSE) {
            cause.clone_into(&mut current.cause);
        } else if let Some(fix) = line.strip_prefix(OCCURRENCE_FIX) {
            fix.clone_into(&mut current.fix);
        }
    }
    texts
        .iter()
        .all(|t| !t.request.trim().is_empty())
        .then_some((cause, fix, texts))
}
