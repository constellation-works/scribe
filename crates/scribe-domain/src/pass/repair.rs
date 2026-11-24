//! Deterministic recovery after the model's one correction attempt.

use super::{BUDGETS, Distilled, Invalid, SEGMENT_CHARS, check, check_with_rules};
use crate::models::{
    Budgets, NewSegmentAnswer, PassMessage, SegmentAnswer, SessionAnswer, SessionRequest,
};

/// Repair only header lengths, segment text lengths and starts outside the
/// window or at/before the previous open segment's last message. Other shape
/// defects still refuse the whole answer, including defects beyond a cut.
/// Call only after the existing model retry; the result is fully checked.
pub fn repair(
    mut answer: SessionAnswer,
    conversation: &[PassMessage],
    request: &SessionRequest,
) -> Result<Distilled, Invalid> {
    check_with_rules(
        answer.clone(),
        conversation,
        request,
        Budgets {
            title: usize::MAX,
            summary: usize::MAX,
            goals: usize::MAX,
            next: usize::MAX,
        },
        true,
    )?;
    let mut repairs = Vec::new();
    for (field, text, max) in [
        ("title", &mut answer.title, BUDGETS.title),
        ("summary", &mut answer.summary, BUDGETS.summary),
        ("goals", &mut answer.goals, BUDGETS.goals),
    ] {
        cut_field(field, text, max, &mut repairs);
    }
    let items = super::next_items(&answer.next);
    let next_chars =
        items.iter().map(|s| s.chars().count() + 2).sum::<usize>() + items.len().saturating_sub(1);
    if next_chars > BUDGETS.next {
        let mut kept = Vec::new();
        let mut chars = 0;
        let mut boundary = "bullet";
        for item in items {
            let size = item.chars().count() + 2 + usize::from(!kept.is_empty());
            if chars + size > BUDGETS.next {
                if kept.is_empty() {
                    kept.push(cut(&item, BUDGETS.next - 2, false).0);
                    boundary = "word";
                }
                break;
            }
            chars += size;
            kept.push(item);
        }
        repairs.push(format!(
            "repaired next: cut at {boundary} boundary (budget {} characters)",
            BUDGETS.next
        ));
        answer.next = kept;
    }

    let expected = request.previous.as_ref().and_then(|p| p.open.as_ref());
    // Match check's tolerant unexpected-open conversion before repairing text
    // and ranges. Its segment count was checked above.
    if expected.is_none()
        && let Some(open) = answer.open.take()
    {
        let first = request.messages.first().ok_or_else(|| {
            Invalid::Segment(String::from("cannot repair starts_at in an empty window"))
        })?;
        answer.new.insert(
            0,
            NewSegmentAnswer {
                starts_at: first.uuid.clone(),
                heading: open.heading,
                text: open.text,
                topics: open.topics,
                related: open.related,
            },
        );
    }
    let over_open = answer
        .open
        .as_ref()
        .is_some_and(|s| s.text.trim().chars().count() > SEGMENT_CHARS);
    if let Some(open) = &mut answer.open {
        cut_field(
            "open segment text",
            &mut open.text,
            SEGMENT_CHARS,
            &mut repairs,
        );
    }
    for (i, segment) in answer.new.iter_mut().enumerate() {
        cut_field(
            &format!("new segment {} text", i + 1),
            &mut segment.text,
            SEGMENT_CHARS,
            &mut repairs,
        );
    }

    let last_open = expected
        .and_then(|s| s.last_message.as_ref())
        .and_then(|id| conversation.iter().position(|m| &m.uuid == id));
    let position = |uuid: &str| conversation.iter().position(|m| m.uuid == uuid);
    let window_index = |uuid: &str| request.messages.iter().position(|m| m.uuid == uuid);
    let follows =
        |uuid: &str| last_open.is_none_or(|last| position(uuid).is_some_and(|i| i > last));
    // Starts the final check accepts as they stand; a relocated segment must
    // land before them, so the transcript order the check enforces holds.
    let earliest_kept = answer
        .new
        .iter()
        .filter(|s| follows(&s.starts_at))
        .filter_map(|s| window_index(&s.starts_at))
        .min();
    let mut kept = Vec::new();
    let mut relocated: Vec<NewSegmentAnswer> = Vec::new();
    let mut last_anchor = None;
    let mut forced_seal = false;
    for (i, mut segment) in std::mem::take(&mut answer.new).into_iter().enumerate() {
        if window_index(&segment.starts_at).is_some() && follows(&segment.starts_at) {
            kept.push(segment);
            continue;
        }
        if let Some(previous) = expected {
            let open = answer.open.get_or_insert_with(|| SegmentAnswer {
                heading: previous.heading.clone(),
                text: previous.text.clone(),
                topics: previous
                    .topics
                    .as_slice()
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                related: previous.related.iter().map(ToString::to_string).collect(),
            });
            let continuation = format!("{}\n\n{}", open.text.trim(), segment.text.trim());
            if continuation.chars().count() <= SEGMENT_CHARS {
                merge_into(open, segment, continuation);
                repairs.push(format!(
                    "repaired new segment {} starts_at: continued the open segment",
                    i + 1
                ));
                continue;
            }
            // The first window message after the open segment's last message
            // that the check can place before every start it keeps.
            let anchor = request.messages.iter().enumerate().find(|(at, m)| {
                last_anchor.is_none_or(|last| *at > last)
                    && earliest_kept.is_none_or(|kept| *at < kept)
                    && follows(&m.uuid)
            });
            let Some((at, first)) = anchor else {
                // No start can hold this content: it ends the open segment at
                // the window's end instead, cut to the segment budget.
                let (text, boundary) = cut(&continuation, SEGMENT_CHARS, true);
                merge_into(open, segment, text);
                forced_seal = true;
                repairs.push(format!(
                    "repaired new segment {} starts_at: continued the open segment, cut at {boundary} boundary (budget {SEGMENT_CHARS} characters)",
                    i + 1
                ));
                continue;
            };
            segment.starts_at.clone_from(&first.uuid);
            last_anchor = Some(at);
            relocated.push(segment);
            repairs.push(format!(
                "repaired new segment {} starts_at: opened at the first window message after the open segment",
                i + 1
            ));
            continue;
        }
        // No open segment: the first new segment starts the window.
        let first = request.messages.first().ok_or_else(|| {
            Invalid::Segment(String::from("cannot repair starts_at in an empty window"))
        })?;
        segment.starts_at.clone_from(&first.uuid);
        // Keep the content, ahead of valid later starts. The final check still
        // refuses collisions; never discard another new segment to make room.
        relocated.push(segment);
        repairs.push(format!(
            "repaired new segment {} starts_at: opened at the window's first message",
            i + 1
        ));
    }
    let mut new = relocated;
    new.extend(kept);
    answer.new = new;
    let seal_open = (over_open || forced_seal) && answer.new.is_empty();
    if seal_open {
        repairs.push(String::from(
            "repaired open segment: sealed at window end after text cut",
        ));
    }
    let mut distilled = check(answer, conversation, request)?;
    distilled.repairs = repairs;
    distilled.seal_open = seal_open;
    Ok(distilled)
}

/// Append a new segment's content to the open segment. Each input's topics
/// passed validation; keep their order and the first five distinct labels.
fn merge_into(open: &mut SegmentAnswer, segment: NewSegmentAnswer, text: String) {
    open.text = text;
    for topic in segment.topics {
        if !open.topics.contains(&topic) && open.topics.len() < 5 {
            open.topics.push(topic);
        }
    }
    for related in segment.related {
        if !open.related.contains(&related) {
            open.related.push(related);
        }
    }
}

fn cut_field(field: &str, text: &mut String, max: usize, repairs: &mut Vec<String>) {
    if text.trim().chars().count() > max {
        let (bounded, boundary) = cut(text.trim(), max, true);
        *text = bounded;
        repairs.push(format!(
            "repaired {field}: cut at {boundary} boundary (budget {max} characters)"
        ));
    }
}

/// Sentence punctuation must end a word; decimals and punctuation inside a
/// token do not count. A token with no word boundary is cut by Unicode char.
fn cut(text: &str, max: usize, sentence: bool) -> (String, &'static str) {
    let prefix: String = text.chars().take(max).collect();
    if sentence {
        let mut end = None;
        for (i, c) in prefix.char_indices() {
            if matches!(c, '.' | '!' | '?' | '。' | '！' | '？') {
                let mut after = i + c.len_utf8();
                for closing in text[after..].chars() {
                    if !matches!(closing, '"' | '\'' | '”' | '’' | ')' | ']' | '}') {
                        break;
                    }
                    after += closing.len_utf8();
                }
                if after <= prefix.len()
                    && (matches!(c, '。' | '！' | '？')
                        || text[after..].chars().next().is_none_or(char::is_whitespace))
                {
                    end = Some(after);
                }
            }
        }
        if let Some(end) = end {
            return (prefix[..end].trim_end().to_owned(), "sentence");
        }
    }
    let prefix: String = text.chars().take(max.saturating_sub(1)).collect();
    let boundary = if text[prefix.len()..].starts_with(char::is_whitespace) {
        Some(prefix.len())
    } else {
        prefix.rfind(char::is_whitespace)
    };
    let word = boundary
        .map_or(prefix.as_str(), |end| &prefix[..end])
        .trim_end();
    (format!("{word}…"), "word")
}
