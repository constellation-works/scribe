use crate::support::command::Exit::Success;
use crate::support::command::Input::Cli;
use crate::support::command::Json;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, make_private};
use serde_json::Value;

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["tag"], expect)
    }
}

pub(super) fn config(fx: &Fixture, toml: &str) -> std::path::PathBuf {
    let root = fx.root();
    if !root.exists() {
        fx.create_private_dir_all(&root);
    }
    let file = root.join("config.toml");
    std::fs::write(&file, toml).unwrap();
    make_private(&file);
    file
}

pub(super) fn show(fx: &Fixture, tag: &str) -> Value {
    fx.check(&["show", tag, "--json"], Json)
}

pub(super) const SHIPPED_TRIGGERS: [&str; 6] = [
    "always",
    "never",
    "should not",
    "from now on",
    "I prefer",
    "by default",
];

pub(super) fn listed(fx: &Fixture) -> Value {
    fx.check(&["list", "--json"], Json)
}

pub(super) fn reads(fx: &Fixture) {
    fx.check(&["list"], Success);
    fx.check(Cli(&["entry", "list", "--json"]), Success);
    fx.check(Cli(&["search", "preference", "--json"]), Success);
}
