use crate::output::common::printable::Printable;
use std::io::Write;

fn through(chunks: &[&[u8]]) -> String {
    let mut out = Vec::new();
    {
        let mut p = Printable::new(&mut out);
        for chunk in chunks {
            p.write_all(chunk).unwrap();
        }
        p.flush().unwrap();
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn control_characters_become_replacement_characters() {
    assert_eq!(
        through(&[b"a\x1b[31mb\x07c\rd\x7fe\x00f"]),
        "a\u{FFFD}[31mb\u{FFFD}c\u{FFFD}d\u{FFFD}e\u{FFFD}f"
    );
}

#[test]
fn line_breaks_tabs_and_ordinary_text_pass() {
    assert_eq!(through(&["é日\t1\n".as_bytes()]), "é日\t1\n");
}

#[test]
fn a_c1_control_is_caught_across_a_write_split() {
    assert_eq!(through(&[b"x\xc2", b"\x9bq"]), "x\u{FFFD}q");
    assert_eq!(
        through(&[b"x\xc2", "\u{a9}".as_bytes()[1..].as_ref()]),
        "x\u{a9}"
    );
    assert_eq!(through(&[b"x\xc2\x85y"]), "x\u{FFFD}y");
}

#[test]
fn an_escape_sequence_never_passes() {
    assert_eq!(
        through(&[b"\x1b[33mid\x1b[0m"]),
        "\u{FFFD}[33mid\u{FFFD}[0m"
    );
}
