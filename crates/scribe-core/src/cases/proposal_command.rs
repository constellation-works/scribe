//! The settling verbs of a typed prompt (form 4; docs/design/scribe/2_design.md
//! §3, §4.2, §7): `#scribe approve <p-id>`, `#scribe reject <p-id>`,
//! `#scribe retire <entry-id>`, `#scribe forget <entry-id>` and `#scribe
//! forget <session-id>`.
//!
//! [`crate::cases::user_prompt_submit`] hands each such line here after the transcript
//! check, in prompt order. One locked store call per line applies it
//! ([`crate::ProposalVerbs::settle`]) and the line gets one echo:
//!
//! - approve: the proposal is `approved` and its older entry `superseded`
//!   by the newer (or `retired`, for a stale proposal);
//! - reject: the proposal is `rejected`, and its pair is never proposed
//!   again;
//! - retire: the entry is hidden from answers and stays on disk;
//! - forget: entry text and evidence, proposal quotes of it, and related
//!   candidate quotes, evidence and reasons are cleared; session records stay;
//! - forget a session (`s-<n>`, its provider id or a unique prefix of it, as a
//!   table prints it; 2_design.md §15.7): its
//!   scribe's stored session data is erased and the id is kept as a tombstone.
//!   The provider transcript stays in place.
//!
//! A change is one audit line whose target is the proposal, entry or session; a
//! refusal is one line with its code; an instruction already applied (a
//! replayed hook) changes nothing and writes none. There is no other way to
//! settle: no CLI verb and no Orbit tool reaches this code (4_decisions.md,
//! "Proposals are settled only by #scribe in a typed turn").

use crate::EntryStatus;
use crate::capture::{Settling, Turn};
use crate::cases::hook::{HookOutcome, chain, quote, refusal};
use crate::cases::proposal::ProposalVerbs;
use crate::consistency::Settled;
use crate::runtime::Ctx;

/// The echo and the audit outcome of one settling line.
#[derive(Debug)]
pub struct Reply {
    /// The line shown to the user and the agent.
    pub echo: String,
    /// What the audit log records; `None` when nothing changed.
    pub outcome: Option<HookOutcome>,
}

/// Apply `settling`, typed in `turn`.
#[must_use]
pub fn run<E>(ctx: &Ctx<'_, E>, settling: &Settling, turn: &Turn) -> Reply {
    let cite = turn.cite(settling.line);
    let settled = match ctx.store.settle(&settling.action, &cite, ctx.clock.now()) {
        Ok(settled) => settled,
        Err(err) => {
            return Reply {
                echo: format!(
                    "scribe: could not apply \"{}\" — {}",
                    quote(&settling.said),
                    chain(&err)
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
                    "scribe: approved {proposal} — recorded {entry} as the user's words with the original turn's cite"
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
            format!(
                "scribe: forgot {entry} — entry text and evidence, proposal quotes of it, and related candidate quotes, evidence and reasons are cleared; session records stay"
            ),
        ),
        Settled::SessionForgotten(forgot) => {
            let name = forgot
                .number
                .map_or_else(|| forgot.id.to_string(), |n| format!("s-{n}"));
            // Whatever was typed, the line names the full id it erased.
            let shown = match forgot.number {
                Some(_) => format!("{name} ({})", forgot.id),
                None => name.clone(),
            };
            let off = if forgot.was_on {
                "; it is switched off for good"
            } else {
                ""
            };
            done(
                name.clone(),
                format!(
                    "scribe: forgot {shown} — the session record and {} stored messages are erased{off}; the provider transcript was left untouched",
                    forgot.messages
                ),
            )
        }
        Settled::SessionForgetRepaired { id, number } => {
            let name = number.map_or_else(|| id.to_string(), |n| format!("s-{n}"));
            let shown = match number {
                Some(_) => format!("{name} ({id})"),
                None => name.clone(),
            };
            done(
                name.clone(),
                format!(
                    "scribe: {shown} is already forgotten — cleared remaining scribe data, including any stale checkpoint; the provider transcript was left untouched"
                ),
            )
        }
        Settled::Already(what) => Reply {
            echo: format!("scribe: {what} — nothing changed"),
            outcome: None,
        },
        Settled::Refused(why) => Reply {
            echo: format!("scribe: refused \"{}\" — {why}", quote(&settling.said)),
            outcome: Some(refusal(&turn.session, why.code())),
        },
    }
}
