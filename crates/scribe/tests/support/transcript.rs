use crate::support::command::{Expectation, Input};

impl crate::support::Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["transcript"], expect)
    }
}
