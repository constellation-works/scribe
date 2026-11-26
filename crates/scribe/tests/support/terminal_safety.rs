use crate::support::command::{Expectation, Input};
use crate::support::text;

impl crate::support::Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["entry"], expect)
    }
}

pub(super) fn no_control(label: &str, bytes: &[u8]) {
    let out = text(bytes);
    let bad: Vec<char> = out
        .chars()
        .filter(|c| c.is_control() && *c != '\n' && *c != '\t')
        .collect();
    assert!(bad.is_empty(), "{label} printed {bad:?}: {out}");
}
