use crate::support::command::Exit::Capture;
use crate::support::command::with_env;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, make_private, text};
use serde_json::Value;

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["entry"], expect)
    }
}

pub(super) const ISSUE: &str = include_str!("../fixtures/issue-store/issue-1.md");

pub(super) const SESSION: &str = "session:7e2d4c1a-9b3f-4e8d-a6c5-1f0e2d3c4b5a";

pub(super) fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

/// Leave `pre-issue-2026-11-22/issue-1.md`, the archive an issue
/// conversion left before phase 8.
pub(super) fn place_archive(fx: &Fixture) -> std::path::PathBuf {
    let dir = fx.root().join("pre-issue-2026-11-22");
    fx.create_private_dir_all(&dir);
    let file = dir.join("issue-1.md");
    std::fs::write(&file, ISSUE).unwrap();
    make_private(&file);
    dir
}

pub(super) fn a_write(fx: &Fixture, words: &str) {
    // The fixture's config redacts `GITHUB_TOKEN`, so a write needs a value.
    let out = fx.check(
        with_env(
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                words,
            ],
            "GITHUB_TOKEN",
            "test-github-token-1234",
        ),
        Capture,
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
}
