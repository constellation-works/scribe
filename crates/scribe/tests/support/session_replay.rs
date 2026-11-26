use crate::support;
use crate::support::command::{Expectation, Input};
use crate::support::replay;
use crate::support::{Fixture, copy_private};
use std::path::{Path, PathBuf};

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["session", "show"], expect)
    }
}

pub(super) fn synthetic() -> PathBuf {
    replay::recordings_dir().join("synthetic-rebuild")
}

pub(super) fn recordings() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(replay::recordings_dir())
        .unwrap()
        .map(|item| item.unwrap().path())
        .filter(|path| path.join("fixture.json").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// A writable copy of `recording` in `fx`.
pub(super) fn copy(fx: &Fixture, recording: &Path) -> PathBuf {
    let to = fx.elsewhere("recording");
    copy_private(recording, &to);
    to
}

/// Every file under `root`, relative, with its bytes.
pub(super) fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    support::snapshot(root)
        .into_iter()
        .filter(|(path, _, _)| path.is_file())
        .map(|(path, bytes, _)| (path.strip_prefix(root).unwrap().to_path_buf(), bytes))
        .collect()
}

/// The redaction key: one line on stdin, which `scripts/redact-recording.sh`
/// writes. It is never in argv or the environment (STD-05 R12).
pub(super) fn redaction_key_from_stdin() -> String {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .expect("read the redaction key from stdin");
    line.trim_end_matches(['\r', '\n']).to_owned()
}
