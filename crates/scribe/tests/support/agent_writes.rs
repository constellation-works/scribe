use crate::support;
use crate::support::Fixture;
use crate::support::command::Exit::Capture;
use crate::support::command::{Expectation, Input};

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

/// Files (relative to `root`) whose bytes or mode differ between two snapshots.
pub(super) fn changed(
    before: &[(std::path::PathBuf, Vec<u8>, u32)],
    after: &[(std::path::PathBuf, Vec<u8>, u32)],
) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for (p, b, m) in before {
        if !after.iter().any(|(q, c, n)| q == p && c == b && n == m) {
            out.push(p.clone());
        }
    }
    for (p, _, _) in after {
        if !before.iter().any(|(q, _, _)| q == p) {
            out.push(p.clone());
        }
    }
    out
}

/// Every exported record document, for "no record changed" checks that
/// tolerate scribe.db and its WAL moving bytes.
pub(super) fn records(root: &std::path::Path) -> Vec<Option<String>> {
    [
        "entries/preferences/preferences-1.md",
        "proposals.json",
        "tags.json",
        "marks.json",
        "pickups.json",
    ]
    .iter()
    .map(|rel| support::exported(root, rel))
    .collect()
}

pub(super) fn write_finding_config(fx: &Fixture) {
    fx.create_private_dir_all(&fx.root());
    let config = fx.root().join("config.toml");
    std::fs::write(
        &config,
        "[tags.preferences]\n[tags.finding]\nrequire_evidence = true\n",
    )
    .unwrap();
    support::make_private(&config);
}

#[cfg(unix)]
pub(super) fn catalog_in_terminal(fx: &Fixture, id: &str) -> std::process::Output {
    let command = format!(
        "{} session catalog {id} --author daniel --json",
        support::BIN
    );
    let mut script = std::process::Command::new("script");
    if cfg!(target_os = "macos") {
        script.args(["-q", "/dev/null", "/bin/sh", "-c", &command]);
    } else {
        script.args(["-q", "-e", "-c", &command, "/dev/null"]);
    }
    fx.check(
        script
            .env_clear()
            .env("HOME", fx.home())
            .env("SCRIBE_ROOT", fx.root())
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/sh")
            .stdin(std::process::Stdio::null()),
        Capture,
    )
}
