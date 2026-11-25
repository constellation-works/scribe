//! The session record's log check: the store refuses a record whose log is
//! malformed, on read (the codec) and on write, and reports it as
//! `store_corrupt` naming the file.

use crate::session::{
    REASON_CHARS, SEGMENT_HEADING_CHARS, SegmentOrigin, SegmentState, SessionRecord, SessionStatus,
};

/// Why a record's log is malformed. The store reports it as
/// `store_corrupt`, naming the file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LogError {
    /// Segment numbers are not 1, 2, 3, … in order.
    #[error("log item {index} has n {found}; segments are numbered from 1 without gaps")]
    Numbering {
        /// The position, from 1.
        index: usize,
        /// The number found.
        found: u32,
    },
    /// A segment other than the last is open, or a final record has one.
    #[error("segment {0} is open; only the last segment of an open record may be open")]
    Open(u32),
    /// A heading is empty, too long or more than one line.
    #[error("segment {0}'s heading must be one line of 1-{max} characters", max = SEGMENT_HEADING_CHARS)]
    Heading(u32),
    /// A segment not written by the upgrade has no message range.
    #[error("segment {0} has no first_message or last_message; only an upgraded segment may")]
    Messages(u32),
    /// `active_change.reason` is over its budget.
    #[error("active_change.reason has {0} characters; the limit is {max}", max = REASON_CHARS)]
    Reason(usize),
}

/// Check a record's log and activity metadata: segments numbered from 1
/// without gaps, only the last one open and none when `final`, headings one
/// line of at most [`SEGMENT_HEADING_CHARS`], message ids present unless
/// the segment came from the upgrade, and the change reason within budget.
///
/// # Errors
///
/// The first rule broken.
pub fn check_log(record: &SessionRecord) -> Result<(), LogError> {
    let last = record.log.len();
    for (i, segment) in record.log.iter().enumerate() {
        let n = segment.n.get();
        if usize::try_from(n).ok() != Some(i + 1) {
            return Err(LogError::Numbering {
                index: i + 1,
                found: n,
            });
        }
        let open_allowed = i + 1 == last && record.status == SessionStatus::Open;
        if segment.state == SegmentState::Open && !open_allowed {
            return Err(LogError::Open(n));
        }
        let chars = segment.heading.chars().count();
        if chars == 0 || chars > SEGMENT_HEADING_CHARS || segment.heading.contains('\n') {
            return Err(LogError::Heading(n));
        }
        if segment.origin != SegmentOrigin::Upgrade
            && (segment.first_message.is_none() || segment.last_message.is_none())
        {
            return Err(LogError::Messages(n));
        }
    }
    if let Some(change) = &record.active_change {
        let chars = change.reason.chars().count();
        if chars > REASON_CHARS {
            return Err(LogError::Reason(chars));
        }
    }
    Ok(())
}
