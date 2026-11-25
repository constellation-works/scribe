//! The session app's rules. Pure functions over loaded records: no I/O
//! (STD-02 R5).
//!
//! - [`activity`]: whether a session record still describes something
//!   current, and who cataloged it;
//! - [`control`]: the `/scribe` switch transitions;
//! - [`message`]: the messages a pass keeps from its window;
//! - [`pass`]: a session pass, from a switched-on session's transcript to
//!   its distilled record;
//! - [`sessions`]: the session records' rules: a new link's check, the
//!   children and tree derived from links, and the cadence's due rule;
//! - here: the checks of a session write, a hand-set field, an activity
//!   change by an agent, and a rebuild while a pass owns the session.

pub mod activity;
pub mod control;
pub mod message;
pub mod pass;
pub mod sessions;

use crate::apps::entry::check_content;
use crate::apps::tag::check_agent_author;
use crate::store::refusal;
use crate::{Error, Result, WriteRefusal};

/// The routine's own routes carry a scribe author; an agent author here
/// is a CLI write verb, which may not use a reserved name.
pub fn check_active_author(request: &crate::activity::SetActive) -> Result<()> {
    let by = match request {
        crate::activity::SetActive::Deactivate { by, .. }
        | crate::activity::SetActive::Activate { by, .. } => by,
    };
    if by.kind == crate::AuthorKind::Agent {
        check_agent_author(&by.name)?;
    }
    Ok(())
}

/// Refuse a rebuild write while `id` is on and due: a pass owns it.
pub fn refuse_busy(control: &crate::ControlState, id: &crate::SessionId) -> Result<()> {
    if control.get(id).is_some_and(|c| c.on && c.due) {
        return Err(Error::SessionBusy(id.clone()));
    }
    Ok(())
}

/// A hand-set session field: content, and either a valid project or a
/// one-line title. `value` is already trimmed.
pub fn check_session_field(field: &str, value: &str) -> Result<crate::session::SessionField> {
    check_content(value)?;
    if field == "project" {
        value
            .parse::<crate::Project>()
            .map_err(|_| refusal(WriteRefusal::InvalidText, "invalid project"))?;
        return Ok(crate::session::SessionField::Project);
    } else if field != "title" || value.contains(['\n', '\r']) {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "invalid session field or title",
        ));
    }
    Ok(crate::session::SessionField::Title)
}
