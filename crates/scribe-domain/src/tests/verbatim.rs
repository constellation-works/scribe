use crate::verbatim::{MIN_QUOTE_CHARS, contains, normalize};

#[test]
fn whitespace_runs_and_line_endings_normalize_to_one_space() {
    assert_eq!(normalize("a\r\nb \t\n c"), "a b c");
    assert!(contains(
        "keep merge\r\n  commits for releases",
        "merge commits for"
    ));
}

#[test]
fn composed_and_decomposed_text_compare_equal() {
    // "café" with a precomposed é in the source and e + combining acute in
    // the quote.
    assert!(contains("le caf\u{e9} au lait", "cafe\u{301} au lait"));
}

#[test]
fn no_case_folding_and_no_punctuation_stripping() {
    let source = "squash-merge agent PRs; keep merge commits";
    assert!(contains(source, "agent PRs; keep"));
    assert!(!contains(source, "agent prs; keep"));
    assert!(!contains(source, "agent PRs keep"));
}

#[test]
fn a_quote_must_be_long_enough_and_present() {
    let source = "squash-merge agent PRs";
    assert!(!contains(source, ""));
    assert!(!contains(source, "   "));
    assert!(!contains(source, &source[..MIN_QUOTE_CHARS - 1]));
    assert!(contains(source, &source[..MIN_QUOTE_CHARS]));
    assert!(
        contains(source, " squash-merge agent \n"),
        "the quote's ends are trimmed"
    );
    assert!(!contains(source, "squash-merge agent PRs!"));
}
