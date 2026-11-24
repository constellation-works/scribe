//! Restore a Claude session's handoff and switch from one store snapshot.

use super::Store;
use crate::{
    Entry, EntryStatus, LinkType, Provider, Result, SessionId, SessionLink, WriteRefusal, control,
    handoff,
};
use time::OffsetDateTime;

/// How a continuation identifies its handoff.
#[derive(Clone, Copy)]
pub enum HandoffSelection<'a> {
    /// The newest active handoff for the session the transcript names.
    Predecessor(&'a SessionId),
    /// The handoff id or unique prefix the user typed.
    Named(&'a str),
}

impl Store {
    fn continuation_entry(&self, selection: HandoffSelection<'_>) -> Result<Option<Entry>> {
        match selection {
            HandoffSelection::Predecessor(parent) => Ok(self
                .handoffs_for_session(parent)?
                .into_iter()
                .next()
                .filter(|e| e.handoff_mode == Some(handoff::Mode::Continue))),
            HandoffSelection::Named(id) => {
                let entry = self.find_entry(id)?;
                if entry.status != EntryStatus::Active {
                    return Err(crate::checks::refusal(
                        WriteRefusal::EntryNotActive,
                        "the handoff is no longer active",
                    ));
                }
                if entry.tag.as_str() != handoff::HANDOFF_TAG
                    || entry.handoff_mode != Some(handoff::Mode::Continue)
                {
                    return Err(crate::checks::refusal(
                        WriteRefusal::HandoffSession,
                        "choose an active handoff with mode continue",
                    ));
                }
                Ok(Some(entry))
            }
        }
    }

    /// Return one checked handoff body and whether the child's link was written.
    /// An ordinary start takes no write lock. Recheck selection inside the write
    /// so retirement, a newer handoff, or a switch change cannot race injection.
    pub fn continue_handoff(
        &self,
        child: &SessionId,
        selection: HandoffSelection<'_>,
        now: OffsetDateTime,
    ) -> Result<Option<(String, bool)>> {
        if self.continuation_entry(selection)?.is_none() {
            return Ok(None);
        }
        self.write_step(|| {
            let Some(entry) = self.continuation_entry(selection)? else {
                return Ok(None);
            };
            crate::checks::entry::check_handoff(&entry.tag, &entry.content)?;
            let parent = handoff::check_related(&entry.related).map_err(|err| {
                crate::checks::refusal(WriteRefusal::HandoffSession, err.to_string())
            })?;
            let link = SessionLink {
                kind: LinkType::Continues,
                session: parent,
            };
            let linked = self.apply_session_link(
                child,
                &link,
                control::LinkSwitch::Inherit,
                now,
                Provider::ClaudeCode,
            )?;
            Ok(Some((entry.content, linked.added)))
        })
    }
}
