use crate::support::command::{Expectation, Input};

impl crate::support::Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["init"], expect)
    }
}

pub(super) fn install(
    fx: &crate::support::Fixture,
    provider: &str,
    target: &std::path::Path,
    path: &std::ffi::OsStr,
    json: bool,
) -> std::process::Output {
    let mut cmd = fx.cmd(&[
        "init",
        "--provider",
        provider,
        "--target",
        target.to_str().unwrap(),
    ]);
    if json {
        cmd.arg("--json");
    }
    fx.check(
        cmd.env("PATH", path),
        crate::support::command::Exit::Capture,
    )
}
