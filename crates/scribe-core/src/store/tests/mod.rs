//! Unit tests for the store, one file per source file (STD-02 R19). As a child
//! of `store` this module also reaches its private submodules, which nothing
//! outside `store` can.

mod archive;
mod audit;
mod db;
mod documents;
mod fsio;
mod memory;
mod migrated;
mod rebuild;
mod redact;
mod sessions;
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

/// Whether any file under `root` (`scribe.db` and its WAL included) holds
/// `needle`: what a forget or a redaction must leave nowhere.
fn on_disk(root: &std::path::Path, needle: &str) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for item in std::fs::read_dir(&dir).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::read(&path)
                .unwrap()
                .windows(needle.len())
                .any(|w| w == needle.as_bytes())
            {
                return true;
            }
        }
    }
    false
}

/// The records [`seed`] writes.
#[derive(Default)]
struct Seed {
    sessions: Vec<crate::SessionRecord>,
    control: crate::session::ControlState,
    entries: Vec<crate::Entry>,
    claims: Vec<crate::proposal::Proposal>,
    tags: Vec<crate::StoreTag>,
    marks: Vec<crate::annotation::MarkRecord>,
    pickups: Vec<crate::action::Pickup>,
    audit: Vec<(u32, super::audit::AuditEvent)>,
}

/// A store at `root` (created owner-only if missing) holding `seed`'s
/// records as `scribe.db` rows, written through a store write as the
/// commands write them: every kind lives in the database, as in a store
/// any write since phase 8 left.
fn seed(root: &std::path::Path, seed: &Seed) -> super::Store {
    use super::db;
    if !root.exists() {
        std::fs::create_dir(root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let store = super::Store::open(root);
    store
        .with_conn(|conn, path| {
            let fail = |e| db::db_error("seed", path, e);
            for record in &seed.sessions {
                db::sessions::put_record(conn, path, record)?;
            }
            let before = crate::session::ControlState::default();
            db::sessions::put_control(conn, path, &before, &seed.control)?;
            for entry in &seed.entries {
                db::docs::put_entry(conn, path, entry, true, fail)?;
            }
            db::docs::put_claims(conn, path, &[], &seed.claims, fail)?;
            db::docs::put_tags(conn, path, &seed.tags, fail)?;
            db::docs::put_marks(conn, path, &seed.marks, fail)?;
            db::docs::put_pickups(conn, path, &seed.pickups, fail)?;
            for (format, event) in &seed.audit {
                db::docs::put_audit(conn, path, *format, event, None, fail)?;
            }
            Ok(())
        })
        .unwrap();
    store
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
