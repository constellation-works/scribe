//! Read-only store export to a new or empty directory.

use crate::db::{self, codec};
use crate::error::{Error, Result, refusal};
use crate::{CONTROL_FILE, RECORD_EXT, SESSIONS_DIR, Store, WriteRefusal};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The format of `scribe export`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// One file per session, the shape of today's record files, and
    /// `control.json`.
    Markdown,
    /// One line per row, one file per table.
    Jsonl,
}

/// What `scribe export` wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportCounts {
    /// The files written, relative to the target directory, in order.
    pub files: Vec<String>,
    /// Sessions written.
    pub sessions: usize,
    /// Entries written.
    pub entries: usize,
    /// Claims written.
    pub claims: usize,
    /// Audit lines written.
    pub audit_lines: usize,
    /// Records that did not read, so were not written.
    pub skipped: Vec<PathBuf>,
}

impl Store {
    /// `scribe export`: write every record to `target`, a directory that is
    /// missing or empty, created owner-only. Reads the store only: no lock,
    /// no store write (STD-01 R31).
    ///
    /// # Errors
    ///
    /// `export_target_exists` for a directory with anything in it;
    /// `store_corrupt` for a file store whose records never moved into
    /// `scribe.db`; I/O and store failures.
    pub fn export(&self, target: &Path, as_format: ExportFormat) -> Result<ExportCounts> {
        let mut out = ExportCounts::default();
        let rows = match as_format {
            ExportFormat::Markdown => None,
            ExportFormat::Jsonl if !self.root_exists()? => Some(Vec::new()),
            ExportFormat::Jsonl => {
                let db = db::read_export(&self.root, SESSION_EXPORT_TABLES, DOC_EXPORT_TABLES)?;
                let (spooled, _) = self.spooled_audit()?;
                if !db.sessions_moved || !db.docs_moved {
                    self.refuse_file_store()?;
                }
                self.check_stamp()?;
                let mut rows = db.rows;
                if !spooled.is_empty() {
                    let drained: BTreeSet<&str> = rows
                        .iter()
                        .filter(|(table, _)| *table == "audit")
                        .filter_map(|(_, row)| row.get("spool").and_then(serde_json::Value::as_str))
                        .collect();
                    let spool_dir = self.root.join(crate::db::write::AUDIT_SPOOL_DIR);
                    let mut pending = Vec::with_capacity(spooled.len());
                    for (name, format, event) in &spooled {
                        if !drained.contains(name.as_str()) {
                            pending.push((
                                "audit",
                                crate::db::docs::audit_export_row(
                                    &spool_dir.join(name),
                                    *format,
                                    event,
                                    name,
                                )?,
                            ));
                        }
                    }
                    rows.extend(pending);
                }
                Some(rows)
            }
        };
        prepare_target(target)?;
        let mut files = Vec::new();
        let mut write = |rel: String, bytes: &[u8]| -> Result<()> {
            let path = target.join(&rel);
            let mut dir = target.to_path_buf();
            for part in Path::new(&rel)
                .parent()
                .into_iter()
                .flat_map(Path::components)
            {
                dir.push(part);
                if !cw_fs::inspect_dir(&dir)? {
                    cw_fs::create_private_dir(&dir)?;
                }
            }
            cw_fs::write_atomic(&path, bytes)?;
            files.push(rel);
            Ok(())
        };
        match rows {
            Some(rows) => {
                let mut tables: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
                for (table, row) in rows {
                    let line = tables.entry(table).or_default();
                    serde_json::to_writer(&mut *line, &row).map_err(|e| {
                        Error::corrupt(target, format!("a row does not encode: {e}"))
                    })?;
                    line.push(b'\n');
                }
                let lines = |name: &str| {
                    tables
                        .get(name)
                        .map_or(0, |b| b.iter().filter(|c| **c == b'\n').count())
                };
                out.sessions = lines("sessions");
                out.entries = lines("entries");
                out.claims = lines("claims");
                out.audit_lines = lines("audit");
                for (table, bytes) in tables {
                    write(format!("{table}.jsonl"), &bytes)?;
                }
            }
            None => {
                let loaded = self.load_sessions()?;
                let mut records = loaded.records;
                records.sort_by(|a, b| a.started.cmp(&b.started).then_with(|| a.id.cmp(&b.id)));
                for record in &records {
                    let rel = format!(
                        "{SESSIONS_DIR}/{:04}/{:02}/{}{RECORD_EXT}",
                        record.started.year(),
                        u8::from(record.started.month()),
                        record.id
                    );
                    let bytes = codec::encode_session(&target.join(&rel), record)?;
                    write(rel, &bytes)?;
                }
                let path = target.join(CONTROL_FILE);
                write(
                    CONTROL_FILE.to_owned(),
                    &codec::encode_control(&path, &loaded.control)?,
                )?;
                out.sessions = records.len();
                out.skipped = loaded.skipped.into_iter().map(|(_, s)| s.path).collect();
                self.export_documents(target, &mut write, &mut out)?;
            }
        }
        out.files = files;
        Ok(out)
    }
}

/// Every session table `scribe export --as jsonl` writes (phase 6), with
/// its row order.
const SESSION_EXPORT_TABLES: &[(&str, &str)] = &[
    ("sessions", "id"),
    ("segments", "session, n"),
    ("session_links", "session, position"),
    ("session_terms", "session, position"),
    ("codex_cursors", "session"),
    ("rebuild_checkpoints", "session"),
];

/// Every document table `scribe export --as jsonl` writes (phase 7), with
/// its row order.
const DOC_EXPORT_TABLES: &[(&str, &str)] = &[
    ("entries", "tag, n"),
    ("claims", "n"),
    ("agent_tags", "name"),
    ("marks", "position"),
    ("pickups", "position"),
    ("audit", "id"),
];

/// Create `target` owner-only, or accept it empty.
///
/// A non-empty or non-directory target is refused here
/// (`export_target_exists`), not in the admin app: it is the precondition
/// of the store's own filesystem write, and only the store touches the
/// filesystem. It is one of the store's documented exceptions to holding
/// no rule (ARCHITECTURE.md).
fn prepare_target(target: &Path) -> Result<()> {
    match std::fs::symlink_metadata(target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
            }
            Ok(cw_fs::create_private_dir(target)?)
        }
        Err(e) => Err(Error::io("inspect", target, e)),
        Ok(meta) if meta.is_dir() => {
            if std::fs::read_dir(target)
                .map_err(|e| Error::io("list", target, e))?
                .next()
                .is_some()
            {
                return Err(refusal(
                    WriteRefusal::ExportTargetExists,
                    format!(
                        "{} is not empty; export into a new or empty directory",
                        target.display()
                    ),
                ));
            }
            Ok(())
        }
        Ok(_) => Err(refusal(
            WriteRefusal::ExportTargetExists,
            format!("{} exists and is not a directory", target.display()),
        )),
    }
}
