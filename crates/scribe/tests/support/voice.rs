use crate::support::command::{Expectation, Input};

impl crate::support::Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["tag"], expect)
    }
}

pub(super) fn named_user(name: &str) -> crate::support::Fixture {
    use std::io::Write;
    let fx = crate::support::Fixture::seeded();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(fx.root().join("config.toml"))
        .unwrap();
    writeln!(file, "\n[user]\nname = {name:?}").unwrap();
    fx
}
