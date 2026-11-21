//! `#scribe forget <issue-id>` and `#scribe forget <issue-id>#<n>` in a
//! typed prompt (docs/design/scribe/2_design.md §11.4): Daniel removes an
//! issue an agent recorded, or one occurrence of it.
//!
//! `user_prompt_submit.rs` hands each such line here after the transcript
//! check. One locked store call ([`scribe_core::Store::forget_issue`])
//! blanks the issue's words, or drops the occurrence, and the line gets one
//! echo. A change is one audit line naming the issue, the occurrence and the
//! session the line was typed in; a refusal is one line with its code; a
//! forget already applied changes nothing and writes none.

use crate::app::Context;
use crate::output::HookOutcome;
use scribe_core::capture::{ForgetIssue, Turn};
use scribe_core::{Author, IssueAudit};

/// The echo and the audit outcome of one forget line.
pub(super) struct Reply {
    /// The line shown to Daniel and the agent.
    pub(super) echo: String,
    /// What the audit log records; `None` when nothing changed.
    pub(super) outcome: Option<HookOutcome>,
}

/// Apply `forget`, typed in `turn`.
pub(super) fn run(forget: &ForgetIssue, turn: &Turn, ctx: &Context) -> Reply {
    let target = forget.target;
    let audit = IssueAudit {
        verb: String::from("forget"),
        id: Some(target.issue.to_string()),
        occurrence: target.occurrence.map(std::num::NonZeroU32::get),
        session: Some(turn.session.to_string()),
        request_message: None,
        moved_to: None,
    };
    match ctx
        .store()
        .forget_issue(target, Author::daniel(), ctx.now())
    {
        Ok((_, true)) => Reply {
            echo: match target.occurrence {
                Some(_) => format!(
                    "scribe: forgot {target} — the occurrence's words are gone from the store; the issue stays"
                ),
                None => format!(
                    "scribe: forgot {target} — its words and every occurrence are gone from the store; the id is not reused"
                ),
            },
            outcome: Some(HookOutcome {
                target: Some(target.issue.to_string()),
                error_code: None,
                issue: Some(audit),
            }),
        },
        Ok((_, false)) => Reply {
            echo: format!("scribe: {target} is already forgotten — nothing changed"),
            outcome: None,
        },
        Err(err) => Reply {
            echo: format!(
                "scribe: could not apply \"{}\" — {}",
                super::user_prompt_submit::quote(&forget.said),
                super::user_prompt_submit::chain(&err)
            ),
            outcome: Some(HookOutcome {
                target: Some(format!("session:{}", turn.session)),
                error_code: Some(err.code()),
                issue: Some(audit),
            }),
        },
    }
}
