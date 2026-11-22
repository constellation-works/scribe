//! Shared fixture for the integration tests: a temporary data root and a
//! command builder that runs the built binary against it.
//!
//! Every child gets a cleared environment with `HOME` and `SCRIBE_ROOT`
//! pointing into the fixture, so no test can read or write the real home or
//! inherit the developer's `SCRIBE_*`, `NO_COLOR` or `TERM` (STD-03 R20).

// Each integration-test binary compiles this module and uses a different
// subset of it.
#![allow(dead_code)]

pub mod replay;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};

/// The sentence a test's claim gets after its words to make it a context
/// paragraph of two sentences (2_design.md §15.3).
pub const CONTEXT_TAIL: &str = "The user and the agent discussed it in this session.";

/// The context paragraph a test claim or annotation stating `said`
/// carries: `said`, then [`CONTEXT_TAIL`].
pub fn context_of(said: &str) -> String {
    let said = said.trim();
    let end = if said.ends_with(['.', '!', '?']) {
        ""
    } else {
        "."
    };
    format!("{said}{end} {CONTEXT_TAIL}")
}

/// Annotation answers as the current schema returns them: a mark a test
/// wrote with `statement` (its short form) carries `context` instead,
/// [`context_of`] it, or `""` for a `none` mark.
pub fn with_mark_contexts(marks: &serde_json::Value) -> serde_json::Value {
    let mut marks = marks.clone();
    for mark in marks
        .as_array_mut()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_object_mut)
    {
        if let Some(said) = mark.remove("statement") {
            let said = said.as_str().unwrap_or_default();
            let context = if said.is_empty() {
                String::new()
            } else {
                context_of(said)
            };
            mark.entry("context").or_insert(serde_json::json!(context));
        }
    }
    marks
}

/// The binary under test, exactly as cargo built it (STD-01 R24).
pub const BIN: &str = env!("CARGO_BIN_EXE_scribe");

/// A temporary home and data root, deleted on drop.
pub struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        std::fs::create_dir(dir.path().join("home")).expect("create fake home");
        Self { dir }
    }

    /// The data directory the binary is pointed at.
    pub fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    /// A path inside the fixture but outside both the home and the data root.
    pub fn elsewhere(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Write `toml` as the data root's `config.toml`, owner-only.
    pub fn config(&self, toml: &str) {
        let root = self.root();
        self.create_private_dir_all(&root);
        let file = root.join("config.toml");
        std::fs::write(&file, toml).expect("write fixture config");
        make_private(&file);
    }

    /// Create a directory tree inside the fixture with owner-only permissions.
    pub fn create_private_dir_all(&self, path: &Path) {
        assert!(
            path.starts_with(self.dir.path()),
            "directory must be inside fixture"
        );
        std::fs::create_dir_all(path).expect("create fixture directory tree");
        let mut current = path;
        loop {
            make_private(current);
            if current == self.dir.path() {
                break;
            }
            current = current.parent().expect("fixture path has a parent");
        }
    }

    /// A command with a hermetic environment.
    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("SCRIBE_ROOT", self.root());
        cmd
    }

    /// Run to completion and capture everything.
    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().expect("run binary")
    }

    /// Run and require exit 0.
    pub fn ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// Write an executable script `bin/<name>` in the fixture, outside the
    /// home and the data root, and return a `PATH` that finds it first.
    #[cfg(unix)]
    pub fn fake_program(&self, name: &str, script: &str) -> std::ffi::OsString {
        use std::os::unix::fs::PermissionsExt;
        let bin = self.elsewhere("bin");
        std::fs::create_dir_all(&bin).expect("create fake bin");
        let path = bin.join(name);
        std::fs::write(&path, script).expect("write fake program");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("make fake program executable");
        let mut var = bin.into_os_string();
        var.push(":/usr/bin:/bin");
        var
    }

    /// Copy the fixture store (`tests/fixtures/store`) to the data root,
    /// owner-only, for the output goldens and most tests.
    pub fn seed(&self) {
        copy_private(&fixture_store(), &self.root());
    }
}

/// The checked-in fixture store.
pub fn fixture_store() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/store")
}

/// Copy the tree at `from` to `to` (which must not exist), every directory
/// and file owner-only, as the binary requires of store state.
pub fn copy_private(from: &Path, to: &Path) {
    std::fs::create_dir(to).expect("create copy");
    make_private(to);
    let mut items: Vec<_> = std::fs::read_dir(from)
        .expect("read fixture")
        .map(|item| item.expect("fixture entry"))
        .collect();
    items.sort_by_key(std::fs::DirEntry::file_name);
    for item in items {
        let target = to.join(item.file_name());
        if item.file_type().expect("file type").is_dir() {
            copy_private(&item.path(), &target);
        } else {
            std::fs::copy(item.path(), &target).expect("copy fixture file");
            make_private(&target);
        }
    }
}

/// Every file under `root` with its bytes and mode, sorted, for proving a
/// command wrote nothing.
pub fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>, u32)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for item in std::fs::read_dir(&dir).expect("read dir") {
            let path = item.expect("dir entry").path();
            let meta = std::fs::symlink_metadata(&path).expect("metadata");
            #[cfg(unix)]
            let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions());
            #[cfg(not(unix))]
            let mode = 0;
            if meta.is_dir() {
                out.push((path.clone(), Vec::new(), mode));
                stack.push(path);
            } else {
                out.push((path.clone(), std::fs::read(&path).expect("read file"), mode));
            }
        }
    }
    out.sort();
    out
}

pub fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("utf-8 output")
}

/// A spawned child that is killed and reaped if the test ends early, panics
/// included (STD-03 R18).
pub struct ChildGuard(Option<Child>);

impl ChildGuard {
    pub fn new(child: Child) -> Self {
        Self(Some(child))
    }

    pub fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("child not yet finished")
    }

    /// Wait for the child and collect what is left of its output.
    pub fn finish(mut self) -> Output {
        let child = self.0.take().expect("child not yet finished");
        child.wait_with_output().expect("wait for child")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Make `path` owner-only (`0700` for a directory, `0600` for a file), as
/// the binary itself creates store state (STD-05 R8).
pub fn make_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("set owner-only mode");
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// One valid entry file's text, `preferences-<n>`, with `content` as its body.
pub fn entry_text(n: usize, content: &str) -> String {
    format!(
        "---
format: 1
kind: entry
id: preferences-{n}
tag: preferences
status: active
project: scribe
topics: []
related: []
provider: claude-code
host: host
created: 2026-01-02T03:04:05Z
updated: 2026-01-02T03:04:05Z
said: \"#scribe preferences: {content}\"
content_from: {{kind: instruction}}
cite: {{session: s-1, message: null, at: 2026-01-02T03:04:05Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
superseded_by: null
review_after: null
---
{content}
",
        sha = "0".repeat(64)
    )
}

/// Write a store of `count` decision entries directly, for tests that need
/// more data than the fixture holds.
pub fn write_entries(root: &Path, count: usize) {
    let dir = root.join("entries").join("preferences");
    std::fs::create_dir_all(&dir).expect("create entries");
    for d in [root.to_path_buf(), root.join("entries"), dir.clone()] {
        make_private(&d);
    }
    for n in 1..=count {
        let path = dir.join(format!("preferences-{n}.md"));
        let content =
            format!("decision number {n} with content long enough to fill the pipe quickly");
        std::fs::write(&path, entry_text(n, &content)).expect("write entry");
        make_private(&path);
    }
}

/// The `idea` catcher that shipped before phase 2 (2_design.md §15.9),
/// now a config example: tests that catch ideas declare it.
pub const IDEA_CATCHER: &str = r#"[tags.idea]
action = true
description = "a future consideration worth revisiting"
[tags.idea.capture]
from = "user"
route = "write"
triggers = ["have * an idea", "had an idea", "here's an idea", "what do you think about * idea", "what if we"]
criteria = "The user proposes something that could be built, changed or tried, and the session does not carry it out."
not = "A direction to act now; a preference; a question of fact; an idea the session goes on to carry out."
"#;

/// [`IDEA_CATCHER`] on `route` instead of `write`.
pub fn idea_catcher_on(route: &str) -> String {
    IDEA_CATCHER.replace("route = \"write\"", &format!("route = \"{route}\""))
}
