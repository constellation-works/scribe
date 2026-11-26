use crate::support;
use crate::support::command::Exit::{Capture, Success};
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, make_private, text};
use serde_json::Value;

pub(super) const SESSION: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";

pub(super) const INACTIVE: &str = "3d5e7f90-1a2b-4c3d-8e9f-0a1b2c3d4e05";

pub(super) const LEGACY: &str = "0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01";

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input
            .into()
            .run_with(self, &[], expect, |fixture, command| {
                command.env("PATH", fixture.elsewhere("empty-path"));
                command.env("GITHUB_TOKEN", "fixture-token-value-1234");
            })
    }
}

pub(super) fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|e| panic!("{e}: {}", text(bytes)))
}

pub(super) fn ok_json(fx: &Fixture, args: &[&str]) -> Value {
    let mut all = args.to_vec();
    all.push("--json");
    json(&fx.check(&all, Success).stdout)
}

pub(super) fn usage_error(fx: &Fixture, args: &[&str]) -> Value {
    let mut all = args.to_vec();
    all.push("--json");
    let out = fx.check(&all, Capture);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{args:?}: {}",
        text(&out.stderr)
    );
    json(&out.stderr)
}

pub(super) fn ids(list: &Value, key: &str, field: &str) -> Vec<String> {
    list[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item[field].as_str().unwrap().to_owned())
        .collect()
}

/// Copy the record of `SESSION` under another id, so two sessions share a
/// prefix.
pub(super) fn copy_session(fx: &Fixture, id: &str) {
    // Before the first session write the record is still a file; after
    // it, a row.
    let dir = fx.root().join("sessions/2026/11");
    let file = dir.join(format!("{SESSION}.md"));
    if file.exists() {
        let original = std::fs::read_to_string(&file).unwrap();
        let path = dir.join(format!("{id}.md"));
        std::fs::write(
            &path,
            original.replace(&format!("id: {SESSION}"), &format!("id: {id}")),
        )
        .unwrap();
        make_private(&path);
        return;
    }
    support::sql(
        &fx.root(),
        &format!(
            "CREATE TEMP TABLE c AS SELECT * FROM sessions WHERE provider_session = '{SESSION}';
             UPDATE c SET id = NULL, provider_session = '{id}';
             INSERT INTO sessions SELECT * FROM c;
             CREATE TEMP TABLE g AS SELECT * FROM segments WHERE session =
                 (SELECT id FROM sessions WHERE provider_session = '{SESSION}');
             UPDATE g SET session = (SELECT id FROM sessions WHERE provider_session = '{id}');
             INSERT INTO segments SELECT * FROM g;"
        ),
    );
}
