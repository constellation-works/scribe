use crate::support;
use crate::support::Fixture;
use crate::support::command::Exit::Success;
use crate::support::command::Input::Cli;
use crate::support::command::{Expectation, Input};
use serde_json::Value;

pub(super) const ON: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

pub(super) const ARCHIVE: &str = "pre-sqlite-2026-10-01";

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["store"], expect)
    }
}

pub(super) fn json(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Leave the archive a phase-7 move left: every record of the store, as
/// `scribe export --as markdown` writes it, under `pre-sqlite-2026-10-01/`.
pub(super) fn leave_archive(fx: &Fixture) {
    let dir = fx.root().join(ARCHIVE);
    fx.check(Cli(&["export", dir.to_str().unwrap(), "--json"]), Success);
}

/// Switched off by hand, as `/scribe off` would leave it.
pub(super) fn switch_off(fx: &Fixture) {
    support::set_switch(&fx.root(), ON, "on", "0");
}

pub(super) fn archive_rows(fx: &Fixture) -> i64 {
    let conn = rusqlite::Connection::open(fx.root().join("scribe.db")).unwrap();
    conn.query_row("SELECT count(*) FROM archive_records", [], |r| r.get(0))
        .unwrap()
}
