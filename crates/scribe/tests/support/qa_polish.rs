use crate::support::command::Exit::Capture;
use crate::support::command::Input::Cli;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, text};
use serde_json::Value;

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run_with(self, &[], expect, |_, command| {
            command.env("GITHUB_TOKEN", "fixture-token-value-1234");
        })
    }
}

pub(super) const SESSION: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

pub(super) fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|e| panic!("{e}: {}", text(bytes)))
}

/// Run `args` with `--json`, require `exit`, and return the error object.
pub(super) fn failure(fx: &Fixture, args: &[&str], exit: i32) -> Value {
    let mut all = args.to_vec();
    all.push("--json");
    let out = fx.check(&all, Capture);
    assert_eq!(
        out.status.code(),
        Some(exit),
        "{args:?}: {}",
        text(&out.stderr)
    );
    assert!(out.stdout.is_empty(), "{args:?}: stdout is payload only");
    json(&out.stderr)
}

/// The fixture config names `GITHUB_TOKEN` in `[redact] env`; running without
/// it set is the unresolved-name case.
pub(super) fn without_secret(fx: &Fixture, args: &[&str]) -> Value {
    let mut all = args.to_vec();
    all.push("--json");
    let out = fx.check(Cli(&all), Capture);
    assert!(out.stdout.is_empty(), "{args:?}: {}", text(&out.stdout));
    // The refusal is logged on stderr before the error object, its last line.
    let stderr = text(&out.stderr);
    let last = stderr.lines().last().unwrap_or_default();
    serde_json::from_str(last).unwrap_or_else(|e| panic!("{e}: {stderr}"))
}
