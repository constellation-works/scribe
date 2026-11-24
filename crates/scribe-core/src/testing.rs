//! Test support: a shim that keeps the tests' call syntax
//! (`store.session(id)`, `store.control()`, …) after ORB-27707 step 2c-4c
//! narrowed `Store` to its lifecycle, so their bodies stay as they were.
//! It is not API: production code reads and writes through
//! [`Store::read`] and [`Store::write`]. Compiled for tests and the
//! `test-support` feature only. Step 4 moves it with the tests.

use crate::group::Group;
use crate::message::{MessageSearch, NewMessage};
use crate::{
    AuditEvent, Config, ControlState, Entry, EntryFilter, EntryId, EntryList, Proposal, Redactor,
    Result, SessionFilter, SessionId, SessionList, SessionView, Store, TagList,
};
use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::PathBuf;

/// The store reads and writes the tests call by their pre-2c-4c names.
/// Each is one delegation to a repository.
pub trait StoreTestReads {
    /// [`crate::repo::Sessions::get`].
    fn session(&self, id: &SessionId) -> Result<SessionView>;
    /// [`crate::repo::Sessions::control`].
    fn control(&self) -> Result<ControlState>;
    /// [`crate::repo::Sessions::change_control`] in a [`Store::write`].
    fn update_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T>;
    /// [`crate::repo::Sessions::list`].
    fn list_sessions(&self, filter: &SessionFilter) -> Result<SessionList>;
    /// [`crate::repo::Sessions::numbers`].
    fn session_numbers(&self) -> Result<BTreeMap<SessionId, NonZeroU64>>;
    /// [`crate::repo::Entries::get`].
    fn entry(&self, id: &EntryId) -> Result<Entry>;
    /// [`crate::repo::Entries::list`].
    fn list_entries(&self, filter: &EntryFilter) -> Result<EntryList>;
    /// [`crate::repo::Proposals::all`].
    fn proposals(&self) -> Result<Vec<Proposal>>;
    /// [`crate::repo::Tags::config`].
    fn config(&self) -> Result<Config>;
    /// [`crate::repo::Tags::list`].
    fn list_tags(&self) -> Result<TagList>;
    /// [`crate::repo::Groups::all`].
    fn groups(&self) -> Result<Vec<Group>>;
    /// [`crate::repo::Groups::get`].
    fn group(&self, id: &str) -> Result<Group>;
    /// [`crate::repo::Groups::search_messages`].
    fn search_group_messages(&self, group: &str, tokens: &[String]) -> Result<MessageSearch>;
    /// [`crate::repo::Messages::search`].
    fn search_messages(&self, tokens: &[String]) -> Result<MessageSearch>;
    /// [`crate::repo::Messages::put`] in a [`Store::write`].
    fn write_messages(&self, messages: &[NewMessage], redactor: &Redactor) -> Result<usize>;
    /// [`crate::repo::Audit::events`].
    fn audit_events(&self) -> Result<(Vec<AuditEvent>, usize)>;
    /// [`crate::repo::Audit::path`].
    fn audit_path(&self) -> PathBuf;
}

impl StoreTestReads for Store {
    fn session(&self, id: &SessionId) -> Result<SessionView> {
        self.read().sessions().get(id)
    }
    fn control(&self) -> Result<ControlState> {
        self.read().sessions().control()
    }
    fn update_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T> {
        self.write(|tx| tx.sessions().change_control(change))
    }
    fn list_sessions(&self, filter: &SessionFilter) -> Result<SessionList> {
        self.read().sessions().list(filter)
    }
    fn session_numbers(&self) -> Result<BTreeMap<SessionId, NonZeroU64>> {
        self.read().sessions().numbers()
    }
    fn entry(&self, id: &EntryId) -> Result<Entry> {
        self.read().entries().get(id)
    }
    fn list_entries(&self, filter: &EntryFilter) -> Result<EntryList> {
        self.read().entries().list(filter)
    }
    fn proposals(&self) -> Result<Vec<Proposal>> {
        self.read().proposals().all()
    }
    fn config(&self) -> Result<Config> {
        self.read().tags().config()
    }
    fn list_tags(&self) -> Result<TagList> {
        self.read().tags().list()
    }
    fn groups(&self) -> Result<Vec<Group>> {
        self.read().groups().all()
    }
    fn group(&self, id: &str) -> Result<Group> {
        self.read().groups().get(id)
    }
    fn search_group_messages(&self, group: &str, tokens: &[String]) -> Result<MessageSearch> {
        self.read().groups().search_messages(group, tokens)
    }
    fn search_messages(&self, tokens: &[String]) -> Result<MessageSearch> {
        self.read().messages().search(tokens)
    }
    fn write_messages(&self, messages: &[NewMessage], redactor: &Redactor) -> Result<usize> {
        self.write(|tx| tx.messages().put(messages, redactor))
    }
    fn audit_events(&self) -> Result<(Vec<AuditEvent>, usize)> {
        self.read().audit().events()
    }
    fn audit_path(&self) -> PathBuf {
        self.read().audit().path()
    }
}
