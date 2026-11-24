//! Test support, behind the `test-support` feature: the CLI's integration
//! tests stage records straight into `scribe.db`, as the store's own unit
//! tests do, since no build reads a file store any more (2_design.md §15.9,
//! phase 8). A shipped build never compiles this.

use super::Store;
use super::db::{self, codec};
use crate::error::{Error, Result};
use crate::proposal::Proposal;
use crate::session::{SessionControl, SessionId};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// A current-format JSON document a test stages: the proposals, or the
/// switches and Codex cursors, in the shape an export writes them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: u32,
    #[serde(default)]
    proposals: Option<Vec<Proposal>>,
    #[serde(default)]
    sessions: Option<BTreeMap<SessionId, SessionControl>>,
    #[serde(default)]
    codex_cursors: Option<BTreeMap<SessionId, String>>,
}

impl Store {
    /// Write each current-format record in `records` into `scribe.db`, as
    /// `scribe export --as markdown` writes them: an entry or a session
    /// record as Markdown (replacing a row with the same id), or a JSON
    /// document of proposals (added to those stored) or of switches and
    /// Codex cursors (merged into those stored). One write: every record
    /// lands or none does.
    ///
    /// # Errors
    ///
    /// [`Error::Corrupt`] when a record is not one of those, at this build's
    /// format, and every error a store write returns.
    pub fn seed_records(&self, records: &[&str]) -> Result<()> {
        self.with_conn(|conn, path| {
            for text in records {
                if text.trim_start().starts_with('{') {
                    seed_document(conn, path, text)?;
                } else if text.contains("\nkind: session\n") {
                    let id = text
                        .lines()
                        .find_map(|line| line.strip_prefix("id: "))
                        .ok_or_else(|| Error::corrupt(path, "a session record without an id"))?;
                    let record = codec::decode_session(path, text.as_bytes(), id.trim())?;
                    db::sessions::put_record(conn, path, &record)?;
                } else {
                    let entry = codec::decode_entry(path, text.as_bytes())?;
                    let fail = |e| db::db_error("seed", path, e);
                    db::docs::put_entry(conn, path, &entry, false, fail)?;
                }
            }
            Ok(())
        })
    }
}

fn seed_document(conn: &db::Conn, path: &Path, text: &str) -> Result<()> {
    let doc: Document = serde_json::from_str(text).map_err(|e| Error::corrupt(path, e))?;
    if doc.format != codec::FORMAT {
        return Err(codec::shape(
            path,
            format!(
                "a document has format {}, not {}",
                codec::FORMAT,
                doc.format
            ),
        ));
    }
    if let Some(proposals) = doc.proposals {
        let before = db::docs::claims(conn, path)?;
        let mut after = before.clone();
        after.extend(proposals);
        codec::check_proposals(path, &after)?;
        let fail = |e| db::db_error("seed", path, e);
        db::docs::put_claims(conn, path, &before, &after, fail)?;
    }
    if doc.sessions.is_some() || doc.codex_cursors.is_some() {
        let before = db::sessions::control(conn, path)?;
        let mut after = before.clone();
        after.sessions.extend(doc.sessions.unwrap_or_default());
        after
            .codex_cursors
            .extend(doc.codex_cursors.unwrap_or_default());
        db::sessions::put_control(conn, path, &before, &after)?;
    }
    Ok(())
}
