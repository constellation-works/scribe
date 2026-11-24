//! `memory/`: the profile, `now` items and milestones
//! (docs/design/scribe/2_design.md §15.5, §15.6), written whole through
//! `write_atomic` and kept in step with `scribe.db` by the memory journal.
//!
//! - **Reads** take no lock and write nothing. With a journal left open by a
//!   crash they print the profile's committed revision from the database
//!   instead of the file and warn `memory_pending`.
//! - **Writes** run inside a store write's transaction and follow one
//!   order: read the files as the open journals leave them; stage the rows
//!   and a journal listing every file operation with its content. Once the
//!   transaction commits, every open journal's operations are applied and
//!   the journal closed, in a write of its own. A rollback leaves no file
//!   written; a crash after the commit leaves an open journal the next
//!   write applies.
//! - Format-24 `m-<n>.md` items are not read: the first store write
//!   archives them with the other files the file store left (§15.7,
//!   phase 8), and a read only counts them.

use super::db::{self, FileOp, HistoryRow, Journal, MemoryCommit, PendingRow, Revision, Writer};
use super::{RECORD_EXT, Skipped, Store, codec, fsio};
use crate::checks::refusal;
use crate::models::{MemoryContext, PendingContext, ProfileRequest, ProfileSession};
use crate::{Error, Result, SessionId, WriteRefusal};
use scribe_domain::memory::{
    self, Draft, Milestone, NowItem, PassChange, ProfileView, Refusal, Stem,
};
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
    /// Format-24 `m-<n>` items the next store write archives.
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

/// The memory files a write reads inside its transaction.
#[derive(Clone, Debug, Default)]
pub struct MemoryFiles {
    /// The `now` items, in stem order.
    pub now: Vec<NowItem>,
    /// The milestones, in stem order.
    pub milestones: Vec<Milestone>,
    /// `profile.md`'s text.
    pub profile: Option<String>,
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// Tests: fail the next memory write after its journal commits and
    /// before any file is touched, as a crash there would.
    pub static CRASH_AFTER_COMMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
        content: Some(codec::encode_now(path, item)?),
    })
}

fn milestone_op(path: &Path, item: &Milestone) -> Result<FileOp> {
    Ok(FileOp {
        path: format!("{MILESTONES_DIR}/{}{RECORD_EXT}", item.stem),
        content: Some(codec::encode_milestone(path, item)?),
    })
}

fn remove_op(dir: &str, stem: &Stem) -> FileOp {
    FileOp {
        path: format!("{dir}/{stem}{RECORD_EXT}"),
        content: None,
    }
}

/// `memory_not_found` for `input` under the data root `root`.
pub fn not_found(input: &str, root: &Path) -> Error {
    refusal(
        WriteRefusal::MemoryNotFound,
        format!(
            "no memory file {input} under {}; run 'scribe memory show'",
            root.join(MEMORY_DIR).display()
        ),
    )
}

/// `input` without a `now/` or `milestones/` prefix or `.md` suffix.
pub fn bare(input: &str) -> &str {
    let input = input.trim();
    let input = input
        .strip_prefix("now/")
        .or_else(|| input.strip_prefix("milestones/"))
        .unwrap_or(input);
    input.strip_suffix(RECORD_EXT).unwrap_or(input)
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
                .and_then(|stem| stem.strip_prefix("m-"))
                .is_some_and(|n| {
                    !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
                });
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
        self.write_step(|| {
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
        #[cfg(any(test, feature = "test-support"))]
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

    /// How many format-24 `memory/m-<n>.md` items the next store write
    /// archives. Reads only.
    fn legacy_count(&self) -> Result<usize> {
        let dir = self.memory_dir();
        if !fsio::inspect_subdir(&dir)? {
            return Ok(0);
        }
        Ok(super::visible(fsio::list_dir(&dir)?)
            .filter(|name| {
                name.strip_suffix(RECORD_EXT)
                    .is_some_and(|stem| stem.starts_with("m-"))
            })
            .count())
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
    fn files_for_write(&self) -> Result<MemoryFiles> {
        let (now, a) = self.read_items(NOW_DIR, codec::decode_now)?;
        let (milestones, b) = self.read_items(MILESTONES_DIR, codec::decode_milestone)?;
        if let Some(bad) = a.into_iter().chain(b).next() {
            return Err(bad.error);
        }
        Ok(MemoryFiles {
            now,
            milestones,
            profile: self.profile_text()?,
        })
    }

    /// Memory as the block prints it (§15.4). Reads only: no lock, no
    /// replay, no migration.
    ///
    /// # Errors
    ///
    /// Anything but a file that does not parse: a link, loose modes, a
    /// newer format, an I/O or database error.
    pub(crate) fn memory_view(&self) -> Result<MemoryView> {
        if !self.root_exists()? {
            return Ok(MemoryView::default());
        }
        let (mut now, mut skipped) = self.read_items(NOW_DIR, codec::decode_now)?;
        let (mut milestones, more) = self.read_items(MILESTONES_DIR, codec::decode_milestone)?;
        skipped.extend(more);
        let legacy = self.legacy_count()?;
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
            unmigrated: legacy,
        })
    }

    /// The file `input` names: `profile`, or a `now` or milestone stem
    /// (with or without its directory). Reads only.
    ///
    /// # Errors
    ///
    /// `memory_not_found`, a file that does not parse, and store failures.
    pub(crate) fn memory_file(&self, input: &str) -> Result<MemoryShown> {
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
    pub(crate) fn profile_revision(&self, n: i64) -> Result<ProfileRevision> {
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
    pub(crate) fn memory_context(
        &self,
        session: &SessionId,
    ) -> Result<(Vec<MemoryContext>, Vec<PendingContext>)> {
        if !self.root_exists()? {
            return Ok((Vec::new(), Vec::new()));
        }
        let (now, _) = self.read_items(NOW_DIR, codec::decode_now)?;
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
    /// journal, then `f`'s commit, its files applied
    /// and its journal closed once the write commits.
    fn memory_write<T>(
        &self,
        now: OffsetDateTime,
        f: impl FnOnce(&mut Writer<'_>, &MemoryFiles) -> Result<(MemoryCommit, T)>,
    ) -> Result<(T, Option<Journal>)> {
        self.prepare_subdir(&self.memory_dir())?;
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        let mut writer = Writer::new(&conn, &self.root);
        let files = self.prepare_memory(&mut writer)?;
        let (commit, out) = f(&mut writer, &files)?;
        let journal = self.commit_memory(&mut writer, &commit, now)?;
        drop(writer);
        drop(conn);
        self.stamp_format()?;
        Ok((out, journal))
    }

    /// The start of every memory write, inside the caller's write: replay
    /// any open journal (one an earlier step of the same command left is
    /// applied here, so this step reads the files it wrote), then read the
    /// files as they now stand.
    fn prepare_memory(&self, writer: &mut Writer<'_>) -> Result<MemoryFiles> {
        self.prepare_subdir(&self.memory_dir())?;
        self.files_after_journals(writer)
    }

    /// The memory files as the open journals leave them: inside a write,
    /// the files on disk with every open journal's operations laid over
    /// them in order, and the journals applied once the write commits, so
    /// a rollback never leaves a file the database does not record. With no
    /// write under way, the journals are replayed now.
    fn files_after_journals(&self, writer: &mut Writer<'_>) -> Result<MemoryFiles> {
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
    fn overlay(&self, files: &mut MemoryFiles, op: &FileOp) -> Result<()> {
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
                    .push(codec::decode_now(&path, content.as_bytes(), stem)?);
                files
                    .now
                    .sort_by(|a, b| a.stem.as_str().cmp(b.stem.as_str()));
            }
        } else if let Some(stem) = stem.strip_prefix("milestones/") {
            files.milestones.retain(|m| m.stem.as_str() != stem);
            if let Some(content) = &op.content {
                files
                    .milestones
                    .push(codec::decode_milestone(&path, content.as_bytes(), stem)?);
                files
                    .milestones
                    .sort_by(|a, b| a.stem.as_str().cmp(b.stem.as_str()));
            }
        }
        // A format-24 `m-<n>.md` operation a journal from before phase 8
        // left is applied but not read: the next store write archives the file.
        Ok(())
    }

    /// Plan a pass window's memory under the lock the caller holds, for
    /// the session `record` (§15.5), after any open journal is replayed:
    /// each draft's quote checked again
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
        sensitive: &[Refusal],
        record: &crate::SessionRecord,
    ) -> Result<(MemoryCommit, MemoryWritten)> {
        if pass.drafts.is_empty()
            && sensitive.is_empty()
            && pass.signal.is_none()
            && !pass.final_pass
        {
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
        let files = self.prepare_memory(writer)?;
        let files = &files;
        let planned = (|| -> Result<(MemoryCommit, MemoryWritten)> {
            let mut written = MemoryWritten {
                refused: sensitive.to_vec(),
                ..MemoryWritten::default()
            };
            let drafts = &pass.drafts;
            let pending = writer.pending(&session)?;
            let (changes, refused) =
                memory::plan(drafts, &files.now, &files.milestones, &pending, &session);
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
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn write_pass_memory(
        &self,
        pass: &PassMemory,
        redactor: &super::Redactor,
        record: &crate::SessionRecord,
    ) -> Result<MemoryWritten> {
        self.prepare_dir(&self.root)?;
        self.prepare_subdir(&self.memory_dir())?;
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        let mut writer = Writer::new(&conn, &self.root);
        let (drafts, sensitive) =
            crate::checks::memory::check_sensitive_drafts(&pass.drafts, redactor);
        let pass = PassMemory {
            drafts,
            ..pass.clone()
        };
        let (commit, written) = self.plan_pass_memory(&mut writer, &pass, &sensitive, record)?;
        self.commit_memory(&mut writer, &commit, record.updated)?;
        drop(writer);
        drop(conn);
        self.stamp_format()?;
        Ok(written)
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
        let changed = writer.forget_session(session)?;
        let journal = self.commit_memory(&mut writer, &commit, now)?;
        let scrubbed = writer.scrub_journal(journal.as_ref(), |op| {
            let (Some(content), Some(stem)) = (&op.content, op.path.strip_suffix(RECORD_EXT))
            else {
                return Ok(false);
            };
            let path = self.op_path(&op.path)?;
            if let Some(stem) = stem.strip_prefix("now/") {
                let item = codec::decode_now(&path, content.as_bytes(), stem)?;
                return Ok(item.sources.contains(session)
                    || item.raised.as_ref().is_some_and(|r| &r.session == session));
            }
            if let Some(stem) = stem.strip_prefix("milestones/") {
                return Ok(codec::decode_milestone(&path, content.as_bytes(), stem)?
                    .sources
                    .contains(session));
            }
            if stem.starts_with("m-") {
                return Ok(content.contains(session.as_str()));
            }
            Ok(false)
        })?;
        Ok(changed || scrubbed || journal.is_some())
    }

    /// One memory write inside the write under way, with its rows and the
    /// files as the open journals leave them ([`Store::memory_write`]).
    pub(super) fn memory_commit<T>(
        &self,
        now: OffsetDateTime,
        f: impl FnOnce(&MemoryRows<'_>, &MemoryFiles) -> Result<(MemoryCommit, T)>,
    ) -> Result<(T, MemoryJournal)> {
        let dir = self.memory_dir();
        let (out, journal) = self.memory_write(now, |writer, files| {
            f(
                &MemoryRows {
                    writer: &*writer,
                    dir: &dir,
                },
                files,
            )
        })?;
        Ok((out, MemoryJournal(journal)))
    }

    /// Discard every copy of the files `journal` removed from the journals,
    /// closed ones included, keeping its own removal, and truncate the log
    /// once the write commits.
    pub(super) fn scrub_copies(&self, journal: &MemoryJournal) -> Result<()> {
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        let writer = Writer::new(&conn, &self.root);
        writer.scrub_journal(journal.0.as_ref(), |_| Ok(false))?;
        db::scope::Scope::truncate_later(&self.root);
        Ok(())
    }
}

/// A committed memory write's journal ([`crate::repo::Memory::commit`]).
#[derive(Debug)]
pub struct MemoryJournal(Option<Journal>);

impl MemoryJournal {
    /// The profile revision the write committed, if any.
    #[must_use]
    pub fn revision(&self) -> Option<i64> {
        self.0.as_ref().and_then(|j| j.revision)
    }
}

/// The rows a memory write reads, and the file operations it stages
/// ([`crate::repo::Memory::commit`]).
pub struct MemoryRows<'w> {
    writer: &'w Writer<'w>,
    dir: &'w Path,
}

impl MemoryRows<'_> {
    /// The newest profile revision and the newest whose journal closed.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn revisions(&self) -> Result<(Option<Revision>, Option<Revision>)> {
        self.writer.revisions()
    }

    /// Profile revision `n`.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn revision(&self, n: i64) -> Result<Option<Revision>> {
        self.writer.revision(n)
    }

    /// The profile job's cursor: the last session read, by time and id.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn cursor(&self) -> Result<Option<(OffsetDateTime, String)>> {
        self.writer.cursor()
    }

    /// Each session's pass signal.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn signals(&self) -> Result<std::collections::BTreeMap<String, i64>> {
        self.writer.signals()
    }

    /// A session's stored user text.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn user_text(&self, session: &str) -> Result<String> {
        self.writer.user_text(session)
    }

    /// The history rows of these kinds.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn history(&self, kinds: &[&str]) -> Result<Vec<HistoryRow>> {
        self.writer.history(kinds)
    }

    /// The operation that writes `item`'s `now` file.
    ///
    /// # Errors
    ///
    /// An item that does not encode.
    pub fn now_op(&self, item: &NowItem) -> Result<FileOp> {
        now_op(self.dir, item)
    }

    /// The operation that writes `item`'s milestone file.
    ///
    /// # Errors
    ///
    /// An item that does not encode.
    pub fn milestone_op(&self, item: &Milestone) -> Result<FileOp> {
        milestone_op(self.dir, item)
    }

    /// The operation that removes the `now` file `stem`.
    #[must_use]
    pub fn remove_now(&self, stem: &Stem) -> FileOp {
        remove_op(NOW_DIR, stem)
    }

    /// The operation that removes the milestone file `stem`.
    #[must_use]
    pub fn remove_milestone(&self, stem: &Stem) -> FileOp {
        remove_op(MILESTONES_DIR, stem)
    }

    /// The operation that writes `profile.md`.
    #[must_use]
    pub fn profile_op(&self, text: &str) -> FileOp {
        FileOp {
            path: PROFILE_FILE.to_owned(),
            content: Some(text.to_owned()),
        }
    }

    /// The sha256 the journal records for a profile file with `text`.
    #[must_use]
    pub fn digest(&self, text: &str) -> String {
        sha256(text)
    }
}
