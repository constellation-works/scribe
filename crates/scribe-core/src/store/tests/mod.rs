//! Unit tests for the store, one file per source file (STD-02 R19). As a child
//! of `store` this module also reaches its private submodules, which nothing
//! outside `store` can.

mod audit;
mod db;
mod format;
mod fsio;
mod memory;
mod rebuild;
mod redact;
mod store;

/// A temporary directory that is owner-only whatever the umask, as a store
/// directory must be (STD-05 R9). `tempfile` leaves the mode to the umask.
fn private_tempdir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

/// A valid entry file for `preferences-1`, with `extra` frontmatter lines
/// inserted before the closing `---`.
fn entry_text(extra: &str) -> String {
    format!(
        "---
format: 1
kind: entry
id: preferences-1
tag: preferences
status: active
project: scribe
topics: [ci]
related: [\"repo:scribe\"]
provider: claude-code
host: host
created: 2026-11-01T10:00:00Z
updated: 2026-11-01T10:00:00Z
said: \"#scribe preferences: run the gate\"
content_from: {{kind: instruction}}
cite: {{session: s-1, message: null, at: 2026-11-01T10:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
superseded_by: null
review_after: null
{extra}---
run the gate before every push
",
        sha = "0".repeat(64)
    )
}

/// A valid session record for `s-1`, started in November 2026.
fn session_text(front_extra: &str, body: &str) -> String {
    format!(
        "---
format: 1
kind: session
id: s-1
status: open
project: scribe
topics: []
related: []
terms: [{{term: level 4, said: \"graduate to level 4\", message: m-1}}]
provider: claude-code
host: host
created: 2026-11-01T10:00:00Z
updated: 2026-11-01T10:00:00Z
title: a session
cwd: /work
started: 2026-11-01T09:00:00Z
on_since: 2026-11-01T09:00:00Z
scribed_through: null
passes: 1
next_open: 0
{front_extra}---
{body}"
    )
}

const SESSION_BODY: &str =
    "## Summary\nsum\n\n## Goals\ngoal\n\n## Outcome\nout\n\n## Next\n- one\n- two\n";
