//! `scribe hook session-end` (ORB-27707 step 3c): the session ended, so its
//! last pass is due and final (docs/design/scribe/2_design.md §5.2).
//!
//! Model-free and cheap: it reads `control.json` and, only for a session
//! that is on, sets `due` and `final` under the store lock. A session that
//! was never switched on, or is off, is left alone: nothing is read beyond
//! `control.json`, nothing is written and nothing is audited. The pass
//! itself runs later from `scribe session pass`. It prints nothing.

use super::reply::HookReply;
use crate::apps::group::HookError;
use crate::runtime::Ctx;
use crate::runtime::audit::HookOutcome;
use crate::{SessionId, control};

/// The hook event this runs for.
const EVENT: &str = "SessionEnd";

/// Run the hook for `session`.
///
/// # Errors
///
/// [`HookError::Core`]: the store could not be read or written.
pub fn run<E>(ctx: &Ctx<'_, E>, session: &SessionId) -> Result<HookReply, HookError<E>> {
    let on = ctx
        .store
        .read()
        .sessions()
        .control()?
        .get(session)
        .is_some_and(|c| c.on);
    let marked = on
        && ctx.store.write(|tx| {
            tx.sessions()
                .change_control(|state| control::mark_final(state, session))
        })?;
    let outcomes = if marked {
        vec![HookOutcome {
            target: Some(format!("session:{session}")),
            error_code: None,
        }]
    } else {
        Vec::new()
    };
    Ok(HookReply {
        event: EVENT,
        message: None,
        context: None,
        block: None,
        outcomes,
    })
}
