//! `scribe group leave` (ORB-27707 step 3c): remove the calling session from
//! its seat.

use crate::SessionId;
use crate::cases::group::GroupVerbs;
use crate::runtime::Ctx;

/// Remove `session` from its seat; the group it left.
///
/// # Errors
///
/// A store error, or the session holds no seat.
pub fn run<E>(ctx: &Ctx<'_, E>, session: &SessionId) -> crate::Result<String> {
    ctx.store.leave_group(session)
}
