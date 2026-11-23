//! `memory/`: the profile, `now` items and milestones
//! (docs/design/scribe/2_design.md §15.5, §15.6), written whole through
//! `write_atomic` and kept in step with `scribe.db` by the memory journal.
//!
//! - **Reads** take no lock and write nothing. With a journal left open by a
//!   crash they print the profile's committed revision from the database
//!   instead of the file and warn `memory_pending`.
//! - **Writes** run inside a store write's transaction and follow one
//!   order: read the files as the open journals leave them; migrate
//!   format-24 `m-<n>` items on the first write (§15.11); stage the rows
//!   and a journal listing every file operation with its content. Once the
//!   transaction commits, every open journal's operations are applied and
//!   the journal closed, in a write of its own. A rollback leaves no file
//!   written; a crash after the commit leaves an open journal the next
//!   write applies.

use super::agent::{check_agent_author, refusal};
use super::db::{self, FileOp, HistoryRow, Journal, MemoryCommit, PendingRow, Writer};
use super::{RECORD_EXT, Redactor, Skipped, Store, format, fsio};
use crate::entry::EntryStatus;
use crate::memory::legacy::MemoryItem;
use crate::memory::{
    self, Draft, Milestone, NowItem, PassChange, ProfileView, Refusal, Stem, migrate, profile,
};
use crate::models::{MemoryContext, PendingContext, ProfileAnswer, ProfileRequest, ProfileSession};
use crate::{AuthorName, Error, Result, SessionId, WriteRefusal};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use time::{Date, OffsetDateTime};

/// The memory directory's name under the data directory.
pub const MEMORY_DIR: &str = "memory";
/// The `now` items' directory under [`MEMORY_DIR`].
pub const NOW_DIR: &str = "now";
/// The milestones' directory under [`MEMORY_DIR`].
pub const MILESTONES_DIR: &str = "milestones";
/// The profile's file under [`MEMORY_DIR`].
pub const PROFILE_FILE: &str = "profile.md";

/// The most sessions one refresh reads.
pub const REFRESH_SESSIONS: usize = 40;
/// Each session's summary in a refresh, in characters.
pub const REFRESH_SUMMARY_CHARS: usize = 1500;
/// How many sessions a refresh reads verbatim.
pub const REFRESH_VERBATIM_SESSIONS: usize = 3;
/// Each verbatim session's user text, in characters.
pub const REFRESH_VERBATIM_CHARS: usize = 12_000;

/// Memory as a read sees it.
#[derive(Debug, Default)]
pub struct MemoryView {
    /// The profile, when there is one.
    pub profile: Option<ProfileView>,
    /// Every `now` item, in stem order.
    pub now: Vec<NowItem>,
    /// Every milestone, in stem order.
    pub milestones: Vec<Milestone>,
    /// Files that did not parse.
    pub skipped: Vec<Skipped>,
    /// A journal is open (`memory_pending`): the profile shown is the
    /// committed revision, and the files may lag it until the next write.
    pub pending: bool,
    /// The database was opened as a read-only copy.
    pub read_only_copy: bool,
    /// Format-24 `m-<n>` items the first memory write migrates.
    pub unmigrated: usize,
}

/// One memory file `memory show <id>` prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryShown {
    /// `profile`.
    Profile(ProfileView),
    /// A `now` item.
    Now(NowItem),
    /// A milestone.
    Milestone(Milestone),
}

/// One profile revision, as `memory show --revision` prints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileRevision {
    /// Its number.
    pub n: i64,
    /// When it was committed.
    pub created: OffsetDateTime,
    /// The profile's text.
    pub content: String,
}

/// What a pass window's memory write did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryWritten {
    /// Each file written, as `memory:now/<stem>` or
    /// `memory:milestones/<stem>`.
    pub written: Vec<String>,
    /// Undated candidates kept.
    pub pending: usize,
    /// Drafts refused under the lock, by code.
    pub refused: Vec<Refusal>,
}

/// The memory a pass window writes with its record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassMemory {
    /// The checked elements, tied to their turns.
    pub drafts: Vec<Draft>,
    /// The window's signal: its last message id and 0–3.
    pub signal: Option<(String, u8)>,
    /// The session's last pass: its pending candidates are dropped.
    pub final_pass: bool,
}

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
    tags: Vec<String>,
    known: BTreeSet<String>,
    cursor_to: Option<(OffsetDateTime, SessionId)>,
    basis: Basis,
}

/// What a refresh's write checks has not changed since its inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Basis {
    latest: Option<i64>,
    file: Option<String>,
    cursor: Option<(OffsetDateTime, String)>,
    now: Vec<Stem>,
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

/// The memory files a write reads inside its transaction.
#[derive(Clone, Debug, Default)]
struct Files {
    now: Vec<NowItem>,
    milestones: Vec<Milestone>,
    profile: Option<String>,
    legacy: Vec<MemoryItem>,
}

#[cfg(test)]
thread_local! {
    /// Tests: fail the next memory write after its journal commits and
    /// before any file is touched, as a crash there would.
    pub(super) static CRASH_AFTER_COMMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Record that `item` cites `cites`, so a forget of it deletes them.
fn link(commit: &mut MemoryCommit, item: &str, cites: &[(SessionId, String)]) {
    for (session, message) in cites {
        commit
            .links
            .push((item.to_owned(), session.to_string(), message.clone()));
    }
}

fn sha256(text: &str) -> String {
    crate::capture::prompt_sha256(text)
}

fn now_op(path: &Path, item: &NowItem) -> Result<FileOp> {
    Ok(FileOp {
        path: format!("{NOW_DIR}/{}{RECORD_EXT}", item.stem),
        content: Some(format::encode_now(path, item)?),
    })
}

fn milestone_op(path: &Path, item: &Milestone) -> Result<FileOp> {
    Ok(FileOp {
        path: format!("{MILESTONES_DIR}/{}{RECORD_EXT}", item.stem),
        content: Some(format::encode_milestone(path, item)?),
    })
}

fn remove_op(dir: &str, stem: &Stem) -> FileOp {
    FileOp {
        path: format!("{dir}/{stem}{RECORD_EXT}"),
        content: None,
    }
}

fn not_found(input: &str, root: &Path) -> Error {
    refusal(
        WriteRefusal::MemoryNotFound,
        format!(
            "no memory file {input} under {}; run 'scribe memory show'",
            root.join(MEMORY_DIR).display()
        ),
    )
}

/// `input` without a `now/` or `milestones/` prefix or `.md` suffix.
fn bare(input: &str) -> &str {
    let input = input.trim();
    let input = input
        .strip_prefix("now/")
        .or_else(|| input.strip_prefix("milestones/"))
        .unwrap_or(input);
    input.strip_suffix(RECORD_EXT).unwrap_or(input)
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

impl Store {
    fn memory_dir(&self) -> PathBuf {
        self.root.join(MEMORY_DIR)
    }

    /// A journal path, checked: `profile.md`, `now/<stem>.md`,
    /// `milestones/<stem>.md` or a format-24 `m-<n>.md`.
    fn op_path(&self, rel: &str) -> Result<PathBuf> {
        let bad = || {
            Error::corrupt(
                self.root.join(super::db::DB_FILE),
                format!("the memory journal names a path outside memory/: {rel}"),
            )
        };
        let valid = rel == PROFILE_FILE
            || rel
                .strip_suffix(RECORD_EXT)
                .and_then(|stem| {
                    stem.strip_prefix("now/")
                        .or_else(|| stem.strip_prefix("milestones/"))
                })
                .is_some_and(|stem| stem.parse::<Stem>().is_ok())
            || rel
                .strip_suffix(RECORD_EXT)
                .and_then(|stem| stem.parse::<memory::legacy::MemoryId>().ok())
                .is_some_and(|id| !rel.contains('/') && id.as_str().starts_with("m-"));
        if valid {
            Ok(self.memory_dir().join(rel))
        } else {
            Err(bad())
        }
    }

    /// Apply a journal's operations in order; each leaves the same file
    /// when applied again. Returns the sha256 of `profile.md` when one
    /// wrote it.
    fn apply_ops(&self, ops: &[FileOp]) -> Result<Option<String>> {
        let mut profile_sha = None;
        for op in ops {
            let path = self.op_path(&op.path)?;
            match &op.content {
                Some(content) => {
                    if let Some(parent) = path.parent() {
                        self.prepare_subdir(parent)?;
                    }
                    fsio::write_atomic(&path, content.as_bytes())?;
                    if op.path == PROFILE_FILE {
                        profile_sha = Some(sha256(content));
                    }
                }
                None => fsio::remove_private(&path)?,
            }
        }
        Ok(profile_sha)
    }

    /// Replay every open journal: apply its operations, then close it.
    fn replay(&self, writer: &mut Writer<'_>) -> Result<usize> {
        let open = writer.open_journals()?;
        for journal in &open {
            let sha = self.apply_ops(&journal.ops)?;
            writer.close(journal, sha.as_deref())?;
        }
        Ok(open.len())
    }

    /// Commit `commit`, apply its file operations and close its journal.
    /// Nothing for an empty commit.
    fn commit_memory(
        &self,
        writer: &mut Writer<'_>,
        commit: &MemoryCommit,
        now: OffsetDateTime,
    ) -> Result<Option<Journal>> {
        if commit.is_empty() {
            return Ok(None);
        }
        let journal = writer.commit(commit, now)?;
        self.finish_journal(writer, journal.as_ref())?;
        Ok(journal)
    }

    /// Apply a committed journal's file operations, then close it: after
    /// the write's commit when one is under way, so a rollback never leaves
    /// files the database does not record.
    pub(super) fn finish_journal(
        &self,
        writer: &mut Writer<'_>,
        journal: Option<&Journal>,
    ) -> Result<()> {
        let Some(journal) = journal else {
            return Ok(());
        };
        if self.defer_journals() {
            return Ok(());
        }
        Self::crash_after_commit(&self.memory_dir())?;
        let sha = self.apply_ops(&journal.ops)?;
        writer.close(journal, sha.as_deref())
    }

    /// Ask the pending write to apply the open journals after it commits;
    /// `false` when no write is under way.
    pub(super) fn defer_journals(&self) -> bool {
        self.after(|after| after.journals = true)
    }

    /// After a commit: apply and close every open journal, in a write of
    /// its own so two processes never replay one at once.
    pub(super) fn apply_open_journals(&self) -> Result<()> {
        Self::crash_after_commit(&self.memory_dir())?;
        self.write(|| {
            self.prepare_subdir(&self.memory_dir())?;
            let conn = db::open_writer(&self.root, self.lock_wait)?;
            let mut writer = Writer::new(&conn, &self.root);
            self.replay(&mut writer)?;
            Ok(())
        })
    }

    /// Tests: the crash [`CRASH_AFTER_COMMIT`] injects, once.
    #[allow(clippy::unnecessary_wraps, unused_variables)]
    fn crash_after_commit(dir: &Path) -> Result<()> {
        #[cfg(test)]
        if CRASH_AFTER_COMMIT.with(|c| c.replace(false)) {
            return Err(Error::io(
                "apply the memory journal in",
                dir.to_path_buf(),
                std::io::Error::other("a crash injected by a test"),
            ));
        }
        Ok(())
    }

    /// Every item file under `memory/<sub>/`, in stem order, and the files
    /// that did not parse. Reads only.
    fn read_items<T>(
        &self,
        sub: &str,
        decode: fn(&Path, &[u8], &str) -> Result<T>,
    ) -> Result<(Vec<T>, Vec<Skipped>)> {
        let mut items = Vec::new();
        let mut skipped = Vec::new();
        let dir = self.memory_dir().join(sub);
        if !fsio::inspect_subdir(&self.memory_dir())? || !fsio::inspect_subdir(&dir)? {
            return Ok((items, skipped));
        }
        let mut names: Vec<String> = super::visible(fsio::list_dir(&dir)?).collect();
        names.sort();
        for name in names {
            let Some(stem) = name.strip_suffix(RECORD_EXT) else {
                continue;
            };
            let path = dir.join(&name);
            if !fsio::inspect_file(&path)? {
                continue;
            }
            match decode(&path, &fsio::read(&path)?, stem) {
                Ok(item) => items.push(item),
                Err(error @ Error::Corrupt { .. }) => skipped.push(Skipped { path, error }),
                Err(other) => return Err(other),
            }
        }
        Ok((items, skipped))
    }

    /// The format-24 `memory/m-<n>.md` items, in id order. Reads only.
    fn legacy_items(&self) -> Result<(Vec<MemoryItem>, Vec<Skipped>)> {
        let mut items = Vec::new();
        let mut skipped = Vec::new();
        let dir = self.memory_dir();
        if !fsio::inspect_subdir(&dir)? {
            return Ok((items, skipped));
        }
        for name in super::visible(fsio::list_dir(&dir)?) {
            let Some(stem) = name.strip_suffix(RECORD_EXT) else {
                continue;
            };
            if !stem.starts_with("m-") {
                continue;
            }
            let path = dir.join(&name);
            if !fsio::inspect_file(&path)? {
                continue;
            }
            match format::decode_memory(&path, &fsio::read(&path)?, stem) {
                Ok(item) => items.push(item),
                Err(error @ Error::Corrupt { .. }) => skipped.push(Skipped { path, error }),
                Err(other) => return Err(other),
            }
        }
        items.sort_by_key(|i| i.id.number());
        Ok((items, skipped))
    }

    fn profile_text(&self) -> Result<Option<String>> {
        let path = self.memory_dir().join(PROFILE_FILE);
        if !fsio::inspect_subdir(&self.memory_dir())? || !fsio::inspect_file(&path)? {
            return Ok(None);
        }
        String::from_utf8(fsio::read(&path)?)
            .map(Some)
            .map_err(|e| Error::corrupt(&path, e))
    }

    /// Every memory file a write needs, refusing one that does not parse:
    /// without every item, an update could open a second file for one event.
    fn files_for_write(&self) -> Result<Files> {
        let (now, a) = self.read_items(NOW_DIR, format::decode_now)?;
        let (milestones, b) = self.read_items(MILESTONES_DIR, format::decode_milestone)?;
        let (legacy, c) = self.legacy_items()?;
        if let Some(bad) = a.into_iter().chain(b).chain(c).next() {
            return Err(bad.error);
        }
        Ok(Files {
            now,
            milestones,
            profile: self.profile_text()?,
            legacy,
        })
    }

    /// Memory as the block prints it (§15.4). Reads only: no lock, no
    /// replay, no migration.
    ///
    /// # Errors
    ///
    /// Anything but a file that does not parse: a link, loose modes, a
    /// newer format, an I/O or database error.
    pub fn memory_view(&self) -> Result<MemoryView> {
        if !self.root_exists()? {
            return Ok(MemoryView::default());
        }
        let (mut now, mut skipped) = self.read_items(NOW_DIR, format::decode_now)?;
        let (mut milestones, more) = self.read_items(MILESTONES_DIR, format::decode_milestone)?;
        skipped.extend(more);
        let (legacy, more) = self.legacy_items()?;
        skipped.extend(more);
        if let Some(forgotten) = super::db::sessions::read_forgotten(&self.root)? {
            let live = |id: &SessionId| !forgotten.contains_key(id);
            now.retain_mut(|item| {
                let sourced = !item.sources.is_empty();
                item.sources.retain(live);
                if item
                    .raised
                    .as_ref()
                    .is_some_and(|raised| !live(&raised.session))
                {
                    item.raised = None;
                }
                !sourced || !item.sources.is_empty()
            });
            milestones.retain_mut(|item| {
                let sourced = !item.sources.is_empty();
                item.sources.retain(live);
                !sourced || !item.sources.is_empty()
            });
        }
        let dbv = db::memory_view(&self.root)?;
        let file = self.profile_text()?;
        let profile = if dbv.open_journal {
            dbv.latest.as_ref().map(|r| ProfileView {
                text: r.content.clone(),
                as_of: Some(r.created.date()),
            })
        } else {
            file.map(|text| ProfileView {
                text,
                as_of: dbv.closed.as_ref().map(|r| r.created.date()),
            })
        };
        Ok(MemoryView {
            profile,
            now,
            milestones,
            skipped,
            pending: dbv.open_journal,
            read_only_copy: dbv.read_only_copy,
            unmigrated: legacy.len(),
        })
    }

    /// The file `input` names: `profile`, or a `now` or milestone stem
    /// (with or without its directory). Reads only.
    ///
    /// # Errors
    ///
    /// `memory_not_found`, a file that does not parse, and store failures.
    pub fn memory_file(&self, input: &str) -> Result<MemoryShown> {
        let view = self.memory_view()?;
        if input.trim() == "profile" {
            return view
                .profile
                .map(MemoryShown::Profile)
                .ok_or_else(|| not_found(input, &self.root));
        }
        let stem = bare(input);
        if let Some(item) = view.now.into_iter().find(|n| n.stem.as_str() == stem) {
            return Ok(MemoryShown::Now(item));
        }
        if let Some(item) = view
            .milestones
            .into_iter()
            .find(|m| m.stem.as_str() == stem)
        {
            return Ok(MemoryShown::Milestone(item));
        }
        if let Some(bad) = view.skipped.into_iter().find(|s| {
            s.path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s == stem)
        }) {
            return Err(bad.error);
        }
        Err(not_found(input, &self.root))
    }

    /// Profile revision `n`. Reads only.
    ///
    /// # Errors
    ///
    /// `revision_not_found`, and store failures.
    pub fn profile_revision(&self, n: i64) -> Result<ProfileRevision> {
        db::revision_at(&self.root, n)?
            .map(|r| ProfileRevision {
                n: r.n,
                created: r.created,
                content: r.content,
            })
            .ok_or_else(|| {
                refusal(
                    WriteRefusal::RevisionNotFound,
                    format!("no profile revision {n}; the first is 1"),
                )
            })
    }

    /// What the session pass reads before its model call: the `now` items
    /// and `session`'s pending candidates. Reads only.
    ///
    /// # Errors
    ///
    /// Store failures; a `now` file that does not parse is left out.
    pub fn memory_context(
        &self,
        session: &SessionId,
    ) -> Result<(Vec<MemoryContext>, Vec<PendingContext>)> {
        if !self.root_exists()? {
            return Ok((Vec::new(), Vec::new()));
        }
        let (now, _) = self.read_items(NOW_DIR, format::decode_now)?;
        let forgotten = super::db::sessions::read_forgotten(&self.root)?.unwrap_or_default();
        let pending = db::pending_read(&self.root, session)?;
        Ok((
            now.iter()
                .filter(|item| {
                    item.sources.is_empty()
                        || item.sources.iter().any(|id| !forgotten.contains_key(id))
                })
                .map(|n| MemoryContext {
                    stem: n.stem.to_string(),
                    date: crate::record::nullable_date::format(n.date),
                    title: n.title.clone(),
                })
                .collect(),
            pending
                .iter()
                .map(|p| PendingContext {
                    id: format!("pending-{}", p.id),
                    subject: p.subject.clone(),
                })
                .collect(),
        ))
    }

    /// One memory write inside the write the caller runs: replay any open
    /// journal, migrate `m-<n>` items, then `f`'s commit, its files applied
    /// and its journal closed once the write commits.
    fn memory_write<T>(
        &self,
        redactor: &Redactor,
        now: OffsetDateTime,
        f: impl FnOnce(&mut Writer<'_>, &Files) -> Result<(MemoryCommit, T)>,
    ) -> Result<(T, Option<Journal>)> {
        self.prepare_subdir(&self.memory_dir())?;
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        let mut writer = Writer::new(&conn, &self.root);
        let files = self.prepare_memory(&mut writer, redactor, now)?;
        let (commit, out) = f(&mut writer, &files)?;
        let journal = self.commit_memory(&mut writer, &commit, now)?;
        drop(writer);
        drop(conn);
        self.stamp_format()?;
        Ok((out, journal))
    }

    /// The start of every memory write, inside the caller's write: replay
    /// any open journal (one an earlier step of the same command left is
    /// applied here, so this step reads the files it wrote) and migrate `m-<n>` items, then read the
    /// files as they now stand.
    fn prepare_memory(
        &self,
        writer: &mut Writer<'_>,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<Files> {
        self.prepare_subdir(&self.memory_dir())?;
        let mut files = self.files_after_journals(writer)?;
        if !files.legacy.is_empty() {
            self.migrate_memory(writer, &files, redactor, now)?;
            files = self.files_after_journals(writer)?;
        }
        Ok(files)
    }

    /// The memory files as the open journals leave them: inside a write,
    /// the files on disk with every open journal's operations laid over
    /// them in order, and the journals applied once the write commits, so
    /// a rollback never leaves a file the database does not record. With no
    /// write under way, the journals are replayed now.
    fn files_after_journals(&self, writer: &mut Writer<'_>) -> Result<Files> {
        let open = writer.open_journals()?;
        if open.is_empty() || !self.defer_journals() {
            self.replay(writer)?;
            return self.files_for_write();
        }
        let mut files = self.files_for_write()?;
        for op in open.iter().flat_map(|j| &j.ops) {
            self.overlay(&mut files, op)?;
        }
        Ok(files)
    }

    /// Lay one journal operation over `files`, as applying it would.
    fn overlay(&self, files: &mut Files, op: &FileOp) -> Result<()> {
        let path = self.op_path(&op.path)?;
        if op.path == PROFILE_FILE {
            files.profile.clone_from(&op.content);
            return Ok(());
        }
        let stem = op.path.strip_suffix(RECORD_EXT).unwrap_or(&op.path);
        if let Some(stem) = stem.strip_prefix("now/") {
            files.now.retain(|n| n.stem.as_str() != stem);
            if let Some(content) = &op.content {
                files
                    .now
                    .push(format::decode_now(&path, content.as_bytes(), stem)?);
                files
                    .now
                    .sort_by(|a, b| a.stem.as_str().cmp(b.stem.as_str()));
            }
        } else if let Some(stem) = stem.strip_prefix("milestones/") {
            files.milestones.retain(|m| m.stem.as_str() != stem);
            if let Some(content) = &op.content {
                files
                    .milestones
                    .push(format::decode_milestone(&path, content.as_bytes(), stem)?);
                files
                    .milestones
                    .sort_by(|a, b| a.stem.as_str().cmp(b.stem.as_str()));
            }
        } else {
            files.legacy.retain(|i| i.id.as_str() != stem);
            if let Some(content) = &op.content {
                files
                    .legacy
                    .push(format::decode_memory(&path, content.as_bytes(), stem)?);
                files.legacy.sort_by_key(|i| i.id.number());
            }
        }
        Ok(())
    }

    /// Migrate format-24 items (§15.11) through one journal: `now` and
    /// milestone files written, every item's text to history, the `m-<n>`
    /// files removed. An active `working` item whose quote fires the
    /// `preferences` capture trigger opens a claim first.
    fn migrate_memory(
        &self,
        writer: &mut Writer<'_>,
        files: &Files,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<()> {
        let plan = migrate::plan(&files.legacy, now.date());
        self.open_migrated_claims(&plan.working, redactor, now)?;
        let path = self.memory_dir();
        let mut commit = MemoryCommit::default();
        for item in &plan.now {
            if !files.now.iter().any(|n| n.stem == item.stem) {
                commit.ops.push(now_op(&path, item)?);
            }
        }
        for item in &plan.milestones {
            if !files.milestones.iter().any(|m| m.stem == item.stem) {
                commit.ops.push(milestone_op(&path, item)?);
            }
        }
        for (item, kind, text, sources) in &plan.history {
            commit.history.push(HistoryRow {
                item: item.clone(),
                kind: (*kind).to_owned(),
                text: redactor.mask(text),
                sources: sources.clone(),
                removed_at: now,
            });
        }
        commit.links.clone_from(&plan.links);
        for id in &plan.remove {
            commit.ops.push(FileOp {
                path: format!("{id}{RECORD_EXT}"),
                content: None,
            });
        }
        self.commit_memory(writer, &commit, now)?;
        Ok(())
    }

    /// Plan a pass window's memory under the lock the caller holds, for
    /// the session `record` (§15.5), after any open journal is replayed and
    /// `m-<n>` items migrated: each draft's quote checked again
    /// against its turn and its texts against the redactor, then planned
    /// against the current files and this session's pending candidates
    /// ([`memory::plan`]). A file whose content would not change is not
    /// written, so a replayed window writes nothing. The final pass drops
    /// the session's pending candidates. The pass commits the returned rows
    /// and journal in its own transaction, with the record.
    pub(super) fn plan_pass_memory(
        &self,
        writer: &mut Writer<'_>,
        pass: &PassMemory,
        redactor: &Redactor,
        record: &crate::SessionRecord,
    ) -> Result<(MemoryCommit, MemoryWritten)> {
        if pass.drafts.is_empty() && pass.signal.is_none() && !pass.final_pass {
            // Nothing to plan, but a journal a crash left open is still
            // applied: every pass window is a write.
            if !writer.open_journals()?.is_empty() && !self.defer_journals() {
                self.prepare_subdir(&self.memory_dir())?;
                self.replay(writer)?;
            }
            return Ok((MemoryCommit::default(), MemoryWritten::default()));
        }
        let session = record.id.clone();
        let path = self.memory_dir();
        let files = self.prepare_memory(writer, redactor, record.updated)?;
        let files = &files;
        let planned = (|| -> Result<(MemoryCommit, MemoryWritten)> {
            let mut written = MemoryWritten::default();
            let mut drafts = Vec::new();
            for draft in &pass.drafts {
                let e = &draft.checked;
                if redactor.mask(&e.title) != e.title
                    || redactor.mask(&e.text) != e.text
                    || redactor.mask(&e.quote) != e.quote
                {
                    written.refused.push(Refusal::Sensitive);
                } else {
                    drafts.push(draft.clone());
                }
            }
            let pending = writer.pending(&session)?;
            let (changes, refused) =
                memory::plan(&drafts, &files.now, &files.milestones, &pending, &session);
            written.refused.extend(refused);
            let mut commit = MemoryCommit::default();
            for change in changes {
                match change {
                    PassChange::Now(item, cites) => {
                        link(&mut commit, item.stem.as_str(), &cites);
                        if !files.now.contains(&item) {
                            written
                                .written
                                .push(format!("memory:{NOW_DIR}/{}", item.stem));
                            commit.ops.push(now_op(&path, &item)?);
                        }
                    }
                    PassChange::Milestone(item, cites) => {
                        link(&mut commit, item.stem.as_str(), &cites);
                        if !files.milestones.contains(&item) {
                            written
                                .written
                                .push(format!("memory:{MILESTONES_DIR}/{}", item.stem));
                            commit.ops.push(milestone_op(&path, &item)?);
                        }
                    }
                    PassChange::AddPending {
                        message,
                        subject,
                        text,
                    } => {
                        // The last pass keeps nothing it would drop at once.
                        if !pass.final_pass {
                            written.pending += 1;
                            commit.pending_add.push(PendingRow {
                                session: session.clone(),
                                message,
                                subject,
                                text,
                                created: record.updated,
                            });
                        }
                    }
                    PassChange::DropPending(id) => commit.pending_drop.push(id),
                }
            }
            if pass.final_pass && (!pending.is_empty() || !commit.pending_drop.is_empty()) {
                commit.pending_clear = Some(session.clone());
            }
            if let Some((through, signal)) = &pass.signal {
                commit
                    .signals
                    .push((session.clone(), through.clone(), (*signal).min(3)));
            }
            Ok((commit, written))
        })()?;
        Ok(planned)
    }

    /// A pass window's memory written on its own, for tests: planned and
    /// committed through one journal, as the pass's transaction does.
    #[cfg(test)]
    pub(super) fn write_pass_memory(
        &self,
        pass: &PassMemory,
        redactor: &Redactor,
        record: &crate::SessionRecord,
    ) -> Result<MemoryWritten> {
        self.prepare_dir(&self.root)?;
        self.prepare_subdir(&self.memory_dir())?;
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        let mut writer = Writer::new(&conn, &self.root);
        let (commit, written) = self.plan_pass_memory(&mut writer, pass, redactor, record)?;
        self.commit_memory(&mut writer, &commit, record.updated)?;
        drop(writer);
        drop(conn);
        self.stamp_format()?;
        Ok(written)
    }

    /// Mark a `now` item raised in `session` by the agent `author`
    /// (`memory ack`, §15.4): it limits unprompted mentions. The same
    /// session again changes nothing.
    ///
    /// # Errors
    ///
    /// `reserved_author`, `memory_not_found`, `memory_not_now` (a
    /// milestone), and store failures.
    pub fn ack_memory(
        &self,
        id: &str,
        session: SessionId,
        author: AuthorName,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<(NowItem, bool)> {
        check_agent_author(&author)?;
        self.prepare_dir(&self.root)?;
        self.write(|| {
            let path = self.memory_dir();
            let stem = bare(id).to_owned();
            let ((item, changed), _) = self.memory_write(redactor, now, |_, files| {
                let Some(mut item) = files.now.iter().find(|n| n.stem.as_str() == stem).cloned()
                else {
                    if files.milestones.iter().any(|m| m.stem.as_str() == stem) {
                        return Err(refusal(
                            WriteRefusal::MemoryNotNow,
                            format!("{stem} is a milestone; only now items are acknowledged"),
                        ));
                    }
                    return Err(not_found(id, &self.root));
                };
                if item.raised.as_ref().is_some_and(|r| r.session == session) {
                    return Ok((MemoryCommit::default(), (item, false)));
                }
                item.raised = Some(memory::Raised {
                    at: now,
                    session: session.clone(),
                });
                let commit = MemoryCommit {
                    ops: vec![now_op(&path, &item)?],
                    ..MemoryCommit::default()
                };
                Ok((commit, (item, true)))
            })?;
            Ok((item, changed))
        })
    }

    /// Remove session occurrences and retire items without any other source.
    /// The existing journal keeps the file changes in step with the audit.
    pub(super) fn forget_session_memory(
        &self,
        session: &SessionId,
        now: OffsetDateTime,
    ) -> Result<bool> {
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        let mut writer = Writer::new(&conn, &self.root);
        let mut files = self.files_after_journals(&mut writer)?;
        let mut commit = MemoryCommit::default();
        let dir = self.memory_dir();
        for item in &mut files.now {
            let had_source = item.sources.contains(session);
            let was_raised = item.raised.as_ref().is_some_and(|r| &r.session == session);
            if had_source || was_raised {
                item.sources.retain(|id| id != session);
                if was_raised {
                    item.raised = None;
                }
                let path = dir.join(NOW_DIR).join(format!("{}.md", item.stem));
                commit.ops.push(if had_source && item.sources.is_empty() {
                    remove_op(NOW_DIR, &item.stem)
                } else {
                    now_op(&path, item)?
                });
            }
        }
        for item in &mut files.milestones {
            if item.sources.contains(session) {
                item.sources.retain(|id| id != session);
                let path = dir.join(MILESTONES_DIR).join(format!("{}.md", item.stem));
                commit.ops.push(if item.sources.is_empty() {
                    remove_op(MILESTONES_DIR, &item.stem)
                } else {
                    milestone_op(&path, item)?
                });
            }
        }
        for item in &mut files.legacy {
            let before = item.occurrences.len();
            item.occurrences.retain(|o| &o.session != session);
            let source_removed = item.occurrences.len() != before;
            let acknowledged = item
                .acknowledged
                .as_ref()
                .is_some_and(|a| &a.session == session);
            if source_removed || acknowledged {
                if acknowledged {
                    item.acknowledged = None;
                }
                if source_removed && item.occurrences.is_empty() {
                    item.status = crate::memory::legacy::MemoryStatus::Retired;
                    item.statement.clear();
                    item.retirement_reason = None;
                }
                let rel = format!("{}.md", item.id);
                let path = dir.join(&rel);
                let bytes = format::encode_memory(&path, item)?;
                let content = String::from_utf8(bytes).map_err(|e| Error::corrupt(&path, e))?;
                commit.ops.push(FileOp {
                    path: rel,
                    content: Some(content),
                });
            }
        }
        let changed = writer.forget_session(session)?;
        let journal = self.commit_memory(&mut writer, &commit, now)?;
        let scrubbed = writer.scrub_journal(journal.as_ref(), |op| {
            let (Some(content), Some(stem)) = (&op.content, op.path.strip_suffix(RECORD_EXT))
            else {
                return Ok(false);
            };
            let path = self.op_path(&op.path)?;
            if let Some(stem) = stem.strip_prefix("now/") {
                let item = format::decode_now(&path, content.as_bytes(), stem)?;
                return Ok(item.sources.contains(session)
                    || item.raised.as_ref().is_some_and(|r| &r.session == session));
            }
            if let Some(stem) = stem.strip_prefix("milestones/") {
                return Ok(format::decode_milestone(&path, content.as_bytes(), stem)?
                    .sources
                    .contains(session));
            }
            if stem.starts_with("m-") {
                let item = format::decode_memory(&path, content.as_bytes(), stem)?;
                return Ok(item.occurrences.iter().any(|o| &o.session == session)
                    || item
                        .acknowledged
                        .as_ref()
                        .is_some_and(|a| &a.session == session));
            }
            Ok(false)
        })?;
        Ok(changed || scrubbed || journal.is_some())
    }

    /// Forget a `now` item or milestone for good for the agent `author`
    /// (§15.5): its file removed and its history rows deleted. The profile
    /// is corrected by editing it, not forgotten.
    ///
    /// # Errors
    ///
    /// `reserved_author`, `usage_error` for `profile`, `memory_not_found`,
    /// and store failures.
    pub fn forget_memory(
        &self,
        id: &str,
        author: AuthorName,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<MemoryShown> {
        check_agent_author(&author)?;
        if id.trim() == "profile" {
            return Err(refusal(
                WriteRefusal::InvalidId,
                "the profile is not forgotten: edit memory/profile.md outside Pinned, and the next refresh takes the edit as a correction",
            ));
        }
        self.prepare_dir(&self.root)?;
        self.write(|| {
            let stem = bare(id).to_owned();
            let (shown, _) = self.memory_write(redactor, now, |_, files| {
                let (shown, op) = if let Some(item) =
                    files.now.iter().find(|n| n.stem.as_str() == stem)
                {
                    (
                        MemoryShown::Now(item.clone()),
                        remove_op(NOW_DIR, &item.stem),
                    )
                } else if let Some(item) = files.milestones.iter().find(|m| m.stem.as_str() == stem)
                {
                    (
                        MemoryShown::Milestone(item.clone()),
                        remove_op(MILESTONES_DIR, &item.stem),
                    )
                } else {
                    return Err(not_found(id, &self.root));
                };
                let commit = MemoryCommit {
                    ops: vec![op],
                    forget_history: vec![stem.clone()],
                    forget_messages: vec![stem.clone()],
                    ..MemoryCommit::default()
                };
                Ok((commit, shown))
            })?;
            Ok(shown)
        })
    }

    /// Write profile revision `n` back as a new revision (`memory
    /// restore`). Restoring the revision the profile already is changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// `revision_not_found`, and store failures.
    pub fn restore_profile(
        &self,
        n: i64,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<Option<i64>> {
        self.prepare_dir(&self.root)?;
        self.write(|| {
            let (_, journal) = self.memory_write(redactor, now, |writer, files| {
                let revision = writer.revision(n)?.ok_or_else(|| {
                    refusal(
                        WriteRefusal::RevisionNotFound,
                        format!("no profile revision {n}; the first is 1"),
                    )
                })?;
                let (latest, _) = writer.revisions()?;
                let same = latest.is_some_and(|l| l.content == revision.content)
                    && files.profile.as_deref() == Some(revision.content.as_str());
                if same {
                    return Ok((MemoryCommit::default(), ()));
                }
                let commit = MemoryCommit {
                    ops: vec![FileOp {
                        path: PROFILE_FILE.to_owned(),
                        content: Some(revision.content.clone()),
                    }],
                    revision: Some(revision.content),
                    ..MemoryCommit::default()
                };
                Ok((commit, ()))
            })?;
            Ok(journal.and_then(|j| j.revision))
        })
    }

    /// What `memory refresh` sends the model (§15.6), read under the lock
    /// after any replay and migration, which it may write.
    ///
    /// # Errors
    ///
    /// A session or entry file that does not parse, and store failures.
    pub fn profile_inputs(
        &self,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<ProfileInputs> {
        self.prepare_dir(&self.root)?;
        self.write(|| {
            let config = self.config()?;
            let (entries, skipped) = self.load_entries()?;
            if let Some(bad) = skipped.into_iter().next() {
                return Err(bad.error);
            }
            let mut loaded = self.load_sessions()?;
            if let Some((_, bad)) = loaded.skipped.pop() {
                return Err(bad.error);
            }
            let records = loaded.records;
            let today = now.date();
            let (inputs, _) = self.memory_write(redactor, now, |writer, files| {
                let (latest, closed) = writer.revisions()?;
                let cursor = writer.cursor()?;
                let file_sha = files.profile.as_deref().map(sha256);
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
                    writer
                        .history(&["profile", "projects"])?
                        .into_iter()
                        .map(|h| {
                            let first = h.text.lines().next().unwrap_or_default().to_owned();
                            format!("{} ({}): {first}", h.kind, h.item)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let after = |r: &crate::SessionRecord| {
                    cursor
                        .as_ref()
                        .is_none_or(|(at, id)| (r.updated, r.id.as_str()) > (*at, id.as_str()))
                };
                let mut due: Vec<&crate::SessionRecord> =
                    records.iter().filter(|r| r.active && after(r)).collect();
                due.sort_by(|a, b| (a.updated, a.id.as_str()).cmp(&(b.updated, b.id.as_str())));
                let backlog = due.len().saturating_sub(REFRESH_SESSIONS);
                due.truncate(REFRESH_SESSIONS);
                let signals = writer.signals()?;
                let mut texts = Vec::new();
                for r in &due {
                    texts.push(writer.user_text(r.id.as_str())?);
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

    /// Check the model's `answer` to `inputs` and write it (§15.6): the
    /// profile as a new revision when it changed, each promoted milestone,
    /// the passed `now` items removed (to history unless promoted), and the
    /// cursor moved through the sessions read. A failed check writes
    /// nothing and keeps the cursor.
    ///
    /// # Errors
    ///
    /// `memory_changed` when memory changed since `inputs`; a `profile_*`
    /// refusal; store failures.
    pub fn write_refresh(
        &self,
        inputs: &ProfileInputs,
        answer: &ProfileAnswer,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<RefreshWritten> {
        self.prepare_dir(&self.root)?;
        self.write(|| {
        if let Some(forgotten) = super::db::sessions::read_forgotten(&self.root)?
            && inputs.sessions.iter().any(|session| forgotten.keys().any(|id| id.as_str() == session.id))
        {
            return Err(refusal(
                WriteRefusal::MemoryChanged,
                "a session was forgotten while the profile was rebuilt; run 'scribe memory refresh' again",
            ));
        }

        let path = self.memory_dir();
        let masks = |text: &str| redactor.mask(text) != text;
        let (written, journal) = self.memory_write(redactor, now, |writer, files| {
            let (latest, closed) = writer.revisions()?;
            let basis = Basis {
                latest: latest.as_ref().map(|r| r.n),
                file: files.profile.as_deref().map(sha256),
                cursor: writer.cursor()?,
                now: files.now.iter().map(|n| n.stem.clone()).collect(),
            };
            if basis != inputs.basis {
                return Err(refusal(
                    WriteRefusal::MemoryChanged,
                    "memory changed while the profile was rebuilt; nothing was written, run 'scribe memory refresh' again",
                ));
            }
            let text = if answer.profile.ends_with('\n') {
                answer.profile.clone()
            } else {
                format!("{}\n", answer.profile)
            };
            profile::check(&text, &inputs.pinned, &inputs.tags, &inputs.known, &masks).map_err(
                |r| {
                    let kind = match r {
                        profile::ProfileRefusal::Shape => WriteRefusal::ProfileShape,
                        profile::ProfileRefusal::Pinned => WriteRefusal::ProfilePinned,
                        profile::ProfileRefusal::TooLong => WriteRefusal::ProfileTooLong,
                        profile::ProfileRefusal::UnknownLink => WriteRefusal::ProfileUnknownLink,
                        profile::ProfileRefusal::Sensitive => WriteRefusal::ProfileSensitive,
                    };
                    refusal(kind, format!("the rebuilt profile was refused ({}); nothing was written", r.code()))
                },
            )?;
            let sources: Vec<SessionId> = inputs
                .sessions
                .iter()
                .filter_map(|s| s.id.parse().ok())
                .collect();
            let mut commit = MemoryCommit::default();
            let mut out = RefreshWritten {
                sessions: inputs.sessions.len(),
                backlog: inputs.backlog,
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
                if masks(&title) || masks(&body) || memory::finance_word(&title) || memory::finance_word(&body) {
                    return Err(refusal(
                        WriteRefusal::ProfileSensitive,
                        "a promoted milestone holds a credential or a finance word; nothing was written",
                    ));
                }
                let from = match m.now.as_deref().map(bare).filter(|s| !s.is_empty()) {
                    Some(stem) => Some(
                        inputs
                            .passed
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
                    commit.ops.push(milestone_op(&path, &stone)?);
                }
            }
            for item in &inputs.passed {
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
                commit.ops.push(remove_op(NOW_DIR, &item.stem));
            }
            let changed = latest.as_ref().is_none_or(|l| l.content != text)
                || files.profile.as_deref() != Some(text.as_str())
                || closed.is_none_or(|c| c.file_sha256.as_deref() != Some(sha256(&text).as_str()));
            if changed {
                commit.ops.insert(
                    0,
                    FileOp {
                        path: PROFILE_FILE.to_owned(),
                        content: Some(text.clone()),
                    },
                );
                commit.revision = Some(text);
            }
            commit.cursor.clone_from(&inputs.cursor_to);
            Ok((commit, out))
        })?;
        let mut written = written;
        written.revision = journal.and_then(|j| j.revision);
        Ok(written)
            })
    }

    /// Open the `preferences` claims migrated `working` items earn
    /// (§15.11): an active item whose quote fires that tag's capture
    /// trigger is a standing rule, offered with its quote for the user to
    /// settle. A claim already opened for the item (a crash before its
    /// files were removed) is not opened again.
    fn open_migrated_claims(
        &self,
        working: &[MemoryItem],
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<()> {
        let config = self.config()?;
        let Some(tag) = config
            .tags
            .tags()
            .iter()
            .find(|t| t.name.as_str() == "preferences")
        else {
            return Ok(());
        };
        let mut candidates = Vec::new();
        for item in working {
            let Some(occurrence) = item.latest_occurrence() else {
                continue;
            };
            let Some(fired) = crate::pass::fire(tag, &occurrence.quote) else {
                continue;
            };
            candidates.push((
                item.id.to_string(),
                occurrence.clone(),
                item.statement.clone(),
                fired,
            ));
        }
        if candidates.is_empty() {
            return Ok(());
        }
        self.open_memory_claims(&tag.name, &candidates, redactor, now)
    }
}
