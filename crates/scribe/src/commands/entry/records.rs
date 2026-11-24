//! CLI fields for decision and open-item writes; their checks are
//! the entry app's ([`scribe_core::apps::entry`]).

use clap::Args;
use scribe_core::apps::entry::RecordFields;
use scribe_core::{AnswerIn, AuthorName, EntryId};
#[cfg(test)]
use {
    crate::error::CliError, scribe_core::SessionId,
    scribe_core::apps::entry::verify_quotes_in_sessions, time::OffsetDateTime,
};

#[derive(Debug, Args)]
pub(super) struct RecordArgs {
    /// For a decision, user or the deciding agent's session name
    #[arg(long)]
    decided_by: Option<AuthorName>,
    /// For an agent decision, why the agent chose it
    #[arg(long)]
    rationale: Option<String>,
    /// For a user decision, their verbatim reason from a related session
    #[arg(long)]
    reason_quote: Option<String>,
    /// The active decision this decision reverses
    #[arg(long)]
    supersedes: Option<EntryId>,
    /// The open item this decision answers
    #[arg(long)]
    settles: Option<EntryId>,
    /// For an open item, the asking agent's session name
    #[arg(long)]
    asked_by: Option<AuthorName>,
    /// For an open item, what waits meanwhile (or 'nothing')
    #[arg(long)]
    blocked: Option<String>,
    /// Where to answer an open item: any (default), session:<id>, or hand
    #[arg(long)]
    answer_in: Option<AnswerIn>,
    /// An option shown in full, repeatable; supply zero or two to six
    #[arg(long = "option")]
    options: Vec<String>,
    /// The 1-based index of the recommended option
    #[arg(long)]
    recommended: Option<usize>,
    /// Why the recommended option is preferred for an open item
    #[arg(long)]
    why: Option<String>,
}

impl RecordArgs {
    /// The fields as `entry add` checks them.
    pub(super) fn into_fields(self) -> RecordFields {
        RecordFields {
            decided_by: self.decided_by,
            rationale: self.rationale,
            reason_quote: self.reason_quote,
            supersedes: self.supersedes,
            settles: self.settles,
            asked_by: self.asked_by,
            blocked: self.blocked,
            answer_in: self.answer_in,
            options: self.options,
            recommended: self.recommended,
            why: self.why,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_quotes_stop_before_a_later_read_error() {
        let id: SessionId = "8dc9df2b-5808-47cf-8be4-190c9c6e0711".parse().unwrap();
        let turns = vec![
            (OffsetDateTime::UNIX_EPOCH, String::from("yes")),
            (OffsetDateTime::UNIX_EPOCH, String::from("because")),
        ];
        let sessions = || {
            [
                Ok((id.clone(), turns.clone())),
                Err(CliError::Read {
                    what: String::from("later transcript"),
                    source: std::io::Error::other("injected read failure"),
                }),
            ]
            .into_iter()
        };
        assert!(verify_quotes_in_sessions(sessions(), "yes", Some("because")).is_ok());
        assert!(verify_quotes_in_sessions(sessions(), "yes", None).is_ok());
        assert!(matches!(
            verify_quotes_in_sessions(sessions(), "yes", Some("missing")),
            Err(CliError::Read { what, .. }) if what == "later transcript"
        ));
        assert!(matches!(
            verify_quotes_in_sessions(sessions(), "missing", Some("because")),
            Err(CliError::Read { what, .. }) if what == "later transcript"
        ));
    }
}
