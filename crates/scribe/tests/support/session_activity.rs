use crate::support;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, text};
use serde_json::Value;

pub(super) const SESSION: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

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

pub(super) fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|e| panic!("{e}: {}", text(bytes)))
}

pub(super) fn audit(fx: &Fixture) -> Vec<Value> {
    support::audit_lines(&fx.root())
}

/// The record as its file reads (`scribe export`).
pub(super) fn record_bytes(fx: &Fixture) -> Vec<u8> {
    support::record_text(&fx.root(), SESSION)
        .expect("the record")
        .into_bytes()
}
