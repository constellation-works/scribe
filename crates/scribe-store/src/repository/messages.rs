//! The message repository: stored session messages and their search.

use super::Messages;
use super::{Access, ReadWrite};
use crate::db;
use crate::error::Result;
use crate::message::{MessageSearch, NewMessage};
use crate::{Entry, Proposal, Redactor, SessionId, Store};

impl<A: Access> Messages<'_, A> {
    /// Stored messages matching `tokens`.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn search(&self, tokens: &[String]) -> Result<MessageSearch> {
        self.store.search_messages(tokens)
    }
}

/// The writes, on the repository a [`Tx`](super::Tx) gives: a
/// [`Snapshot`](super::Snapshot)'s cannot write.
impl Messages<'_, ReadWrite> {
    /// Store `messages`, masked by `redactor`; returns how many rows
    /// changed.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put(&self, messages: &[NewMessage], redactor: &Redactor) -> Result<usize> {
        self.store.write_messages(messages, redactor)
    }
}

impl Store {
    /// Keep a pass window's messages in `scribe.db` (2_design.md §15.7),
    /// each text redacted first, upserted on (session, message id) in one
    /// `BEGIN IMMEDIATE` transaction. The pass calls this before it writes
    /// the session file, whose cursor stays the truth: a window written
    /// again rewrites the same rows. Returns how many rows changed.
    ///
    /// # Errors
    ///
    /// [`Error::Locked`] when another writer holds the database past the
    /// lock wait; the privacy errors when the root or the database is a
    /// link, another user's or open to others; [`Error::NewerFormat`] for a
    /// database a newer build wrote.
    pub(crate) fn write_messages(
        &self,
        messages: &[crate::message::NewMessage],
        redactor: &Redactor,
    ) -> Result<usize> {
        if messages.is_empty() {
            return Ok(0);
        }
        self.prepare_dir(&self.root)?;
        db::upsert_messages(&self.root, self.lock_wait, messages, redactor)
    }

    /// Delete stored messages by (session, message id), for a forget,
    /// leaving no trace of their text in the database or its WAL. A missing
    /// database deletes nothing.
    pub(crate) fn forget_messages(&self, keys: &[(SessionId, String)]) -> Result<usize> {
        db::delete_messages(&self.root, self.lock_wait, keys)
    }

    /// Delete the stored messages a forgotten `entry` rests on: the turn it
    /// cites and the turns its candidate proposals cite.
    pub(crate) fn forget_cited_messages(
        &self,
        entry: &Entry,
        proposals: &[Proposal],
    ) -> Result<usize> {
        let keys: Vec<_> = entry
            .cite
            .iter()
            .chain(
                proposals
                    .iter()
                    .filter(|p| p.older == entry.id)
                    .filter_map(|p| p.candidate.as_ref().map(|c| &c.cite)),
            )
            .filter_map(|c| c.message.clone().map(|m| (c.session.clone(), m)))
            .collect();
        self.forget_messages(&keys)
    }

    /// The stored messages matching any of `tokens` (ranking tokens, as
    /// [`scribe_domain::text::tokens`] gives them) as a word prefix, best FTS5
    /// `bm25()` first. Read-only: no lock, no write (STD-01 R31). A missing
    /// root or database matches nothing.
    ///
    /// # Errors
    ///
    /// The privacy errors for the root or the database, [`Error::NewerFormat`]
    /// and [`Error::Database`].
    pub(crate) fn search_messages(
        &self,
        tokens: &[String],
    ) -> Result<crate::message::MessageSearch> {
        if !self.root_exists()? {
            return Ok(crate::message::MessageSearch::default());
        }
        db::search_messages(&self.root, tokens, None)
    }
}
