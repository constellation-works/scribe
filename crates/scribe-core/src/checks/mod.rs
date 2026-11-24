//! The write checks: pure rules a write must pass before it is stored, each
//! refusing with an [`Error::AgentWrite`] and its [`WriteRefusal`] code
//! (docs/design/scribe/2_design.md §4.6, §16, §18). They were inside the
//! store; the store still calls them, and step 2 of the four-crate refactor
//! moves each call into its use case's write closure. No I/O (STD-02 R5).

pub mod candidates;
pub mod entry;
pub mod group;
pub mod memory;
pub mod records;
pub mod tag;

use crate::{AuthorName, Error, Result, WriteRefusal};

pub(crate) fn refusal(kind: WriteRefusal, message: impl Into<String>) -> Error {
    Error::AgentWrite {
        kind,
        message: message.into(),
    }
}

/// Refuse a reserved name on an agent-channel write: `user` (and `daniel`,
/// its name before format 23), `scribe`, `consistency`, `session-pass` and
/// `tag-retired` name the user's and scribe's own channels, so a record
/// written with one could be mistaken for theirs.
///
/// # Errors
///
/// [`WriteRefusal::ReservedAuthor`] when `author` is reserved.
pub fn check_agent_author(author: &AuthorName) -> Result<()> {
    if author.is_reserved() {
        return Err(refusal(
            WriteRefusal::ReservedAuthor,
            format!(
                "'{author}' is reserved for the user's and scribe's own channels; pass your own crew or model name as --author"
            ),
        ));
    }
    Ok(())
}

/// Refuse blank free text: an entry's content or a proposal's text.
///
/// # Errors
///
/// [`WriteRefusal::InvalidText`] when `text` is empty or only whitespace.
pub fn check_content(text: &str) -> Result<()> {
    required(text)
}

/// Refuse a blank reason, or one over 300 characters.
///
/// # Errors
///
/// [`WriteRefusal::InvalidText`] for either.
pub fn check_reason(reason: &str) -> Result<()> {
    required_reason(reason)
}

pub(crate) fn unknown_tag() -> Error {
    refusal(
        WriteRefusal::TagNotFound,
        "unknown tag; run 'scribe tag list'",
    )
}

pub(crate) fn required(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        return Err(refusal(WriteRefusal::InvalidText, "text must not be empty"));
    }
    Ok(())
}

pub(crate) fn required_reason(reason: &str) -> Result<()> {
    required(reason)?;
    if reason.chars().count() > 300 {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "reason must contain at most 300 characters",
        ));
    }
    Ok(())
}

/// Refuse an instruction without the message id of the turn that backs
/// it: every record of it names that turn, and a store with a blank one
/// does not read.
pub(crate) fn required_message(instructed: &crate::instruction::Instructed) -> Result<()> {
    if instructed.message.trim().is_empty() {
        return Err(refusal(
            WriteRefusal::InstructionNotFound,
            "the instruction's turn has no message id; ask the user again",
        ));
    }
    Ok(())
}

/// The refusal of a write to or on behalf of a forgotten session
/// (`session_forgotten`); `what` says what is refused.
pub(crate) fn forgotten_refusal(id: &crate::SessionId, what: &str) -> Error {
    refusal(
        WriteRefusal::SessionForgotten,
        format!("session {id} was forgotten (#scribe forget); {what}"),
    )
}

/// The routine's own routes carry a scribe author; an agent author here
/// is a CLI write verb, which may not use a reserved name.
pub(crate) fn check_active_author(request: &crate::activity::SetActive) -> Result<()> {
    let by = match request {
        crate::activity::SetActive::Deactivate { by, .. }
        | crate::activity::SetActive::Activate { by, .. } => by,
    };
    if by.kind == crate::AuthorKind::Agent {
        crate::checks::check_agent_author(&by.name)?;
    }
    Ok(())
}

/// Refuse a rebuild write while `id` is on and due: a pass owns it.
pub(crate) fn refuse_busy(control: &crate::ControlState, id: &crate::SessionId) -> Result<()> {
    if control.get(id).is_some_and(|c| c.on && c.due) {
        return Err(Error::SessionBusy(id.clone()));
    }
    Ok(())
}

/// A hand-set session field: content, and either a valid project or a
/// one-line title. `value` is already trimmed.
pub(crate) fn check_session_field(
    field: &str,
    value: &str,
) -> Result<crate::session::SessionField> {
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
