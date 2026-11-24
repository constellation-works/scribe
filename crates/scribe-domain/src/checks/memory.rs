//! The check a pass window's memory drafts pass before they are planned
//! (§15.5).

use crate::Redactor;
use crate::memory::{Draft, Refusal};

/// Split a pass window's `drafts` into those the planner may use and a
/// [`Refusal::Sensitive`] for each one whose title, text or quote redaction
/// would change, in draft order. Pure: no store read.
pub fn check_sensitive_drafts(drafts: &[Draft], redactor: &Redactor) -> (Vec<Draft>, Vec<Refusal>) {
    let mut kept = Vec::new();
    let mut refused = Vec::new();
    for draft in drafts {
        let e = &draft.checked;
        if redactor.mask(&e.title) != e.title
            || redactor.mask(&e.text) != e.text
            || redactor.mask(&e.quote) != e.quote
        {
            refused.push(Refusal::Sensitive);
        } else {
            kept.push(draft.clone());
        }
    }
    (kept, refused)
}
