//! The settling verbs of a typed prompt (form 4; docs/design/scribe/2_design.md
//! §3, §4.2, §7): `#scribe approve <p-id>`, `#scribe reject <p-id>`,
//! `#scribe retire <entry-id>` and `#scribe forget <entry-id>`.
//!
//! `user_prompt_submit.rs` hands each such line here after the transcript
//! check, in prompt order. One locked store call per line applies it
//! ([`scribe_core::Store::settle`]) and the line gets one echo:
//!
//! - approve: the proposal is `approved` and its older entry `superseded`
//!   by the newer (or `retired`, for a stale proposal);
//! - reject: the proposal is `rejected`, and its pair is never proposed
//!   again;
//! - retire: the entry is hidden from answers and stays on disk;
//! - forget: the entry's `said` and `content`, and the proposal quotes of
//!   it, are blanked for good; its id, cite and status stay.
//!
//! A change is one audit line whose target is the proposal or entry; a
//! refusal is one line with its code; an instruction already applied (a
//! replayed hook) changes nothing and writes none. There is no other way to
//! settle: no CLI verb and no Orbit tool reaches this code (4_decisions.md,
//! "Proposals are settled only by #scribe in a typed turn").

use crate::app::Context;
use crate::output::HookOutcome;
use scribe_core::capture::{Settling, Turn};
use scribe_core::consistency::Settled;
use scribe_core::{EntryStatus, SessionId};

/// The echo and the audit outcome of one settling line.
pub(super) struct Reply {
    /// The line shown to Daniel and the agent.
    pub(super) echo: String,
    /// What the audit log records; `None` when nothing changed.
    pub(super) outcome: Option<HookOutcome>,
}

/// Apply `settling`, typed in `turn`.
pub(super) fn run(settling: &Settling, turn: &Turn, ctx: &Context) -> Reply {
    let cite = turn.cite(settling.line);
    let settled = match ctx.store().settle(&settling.action, &cite, ctx.now()) {
        Ok(settled) => settled,
        Err(err) => {
            return Reply {
                echo: format!(
                    "scribe: could not apply \"{}\" — {}",
                    super::user_prompt_submit::quote(&settling.said),
                    super::user_prompt_submit::chain(&err)
                ),
                outcome: Some(refusal(&turn.session, err.code())),
            };
        }
    };
    let done = |target: String, echo: String| Reply {
        echo,
        outcome: Some(HookOutcome {
            target: Some(target),
            error_code: None,
        }),
    };
    match settled {
        Settled::Recorded {
            proposal,
            entry,
            agent_authored,
        } => done(
            proposal.to_string(),
            if agent_authored {
                format!(
                    "scribe: approved {proposal} — recorded {entry} as the agent's entry with the assistant turn's cite (cancel: #scribe retire {entry})"
                )
            } else {
                format!(
                    "scribe: approved {proposal} — recorded {entry} as Daniel's words with the original turn's cite"
                )
            },
        ),
        Settled::Approved {
            proposal,
            entry,
            status,
            by,
        } => {
            let what = match (status, by) {
                (EntryStatus::Superseded, Some(by)) => format!("{entry} is superseded by {by}"),
                (status, _) => format!("{entry} is {status}"),
            };
            done(
                proposal.to_string(),
                format!("scribe: approved {proposal} — {what}; ask leaves it out of answers"),
            )
        }
        Settled::Covered { proposal, by } => done(
            proposal.to_string(),
            format!("scribe: approved {proposal} — already covered by {by}; no new entry"),
        ),
        Settled::Rejected { proposal } => done(
            proposal.to_string(),
            format!(
                "scribe: rejected {proposal} — its entries stay as they are, and it is not proposed again"
            ),
        ),
        Settled::Retired(entry) => done(
            entry.to_string(),
            format!(
                "scribe: retired {entry} — ask leaves it out of answers; scribe entry show {entry} still has it"
            ),
        ),
        Settled::Forgotten(entry) => done(
            entry.to_string(),
            format!("scribe: forgot {entry} — its text is blanked for good; the id and cite stay"),
        ),
        Settled::Already(what) => Reply {
            echo: format!("scribe: {what} — nothing changed"),
            outcome: None,
        },
        Settled::Refused(why) => Reply {
            echo: format!(
                "scribe: refused \"{}\" — {why}",
                super::user_prompt_submit::quote(&settling.said)
            ),
            outcome: Some(refusal(&turn.session, why.code())),
        },
    }
}

fn refusal(session: &SessionId, code: &'static str) -> HookOutcome {
    HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: Some(code),
    }
}
