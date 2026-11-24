//! Which typed turns are kept in `scribe.db`.

use crate::message;

#[test]
fn typed_text_drops_fenced_code_and_the_blank_lines_it_leaves() {
    assert_eq!(
        message::typed_text("Keep this.\n\n```rust\nfn x() {}\n```\n\nAnd this."),
        "Keep this.\n\nAnd this."
    );
    assert_eq!(
        message::typed_text("~~~~\ncode\n~~~\nstill code\n~~~~\nafter"),
        "after"
    );
    assert_eq!(message::typed_text("before\n```\nnever closed"), "before");
    assert_eq!(message::typed_text("```\nonly code\n```"), "");
    assert_eq!(
        message::typed_text("inline `code` stays"),
        "inline `code` stays"
    );
}

#[test]
fn a_turn_with_a_finance_or_sensitive_event_word_is_not_kept() {
    assert!(message::keeps("keep the merge queue"));
    assert!(!message::keeps(""));
    assert!(!message::keeps("my salary went up"));
    assert!(!message::keeps("my grandmother died last week"));
}
