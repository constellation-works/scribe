//! The store through its public API, against the shared fixture store the
//! CLI's tests also read. Each test copies it to a fresh owner-only temp root.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use scribe_core::{
    EntryFilter, EntryStatus, Error, Redactor, SessionFilter, Store, TagName, verbatim,
};
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scribe/tests/fixtures/store")
}

#[cfg(unix)]
fn private(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn private(_: &Path, _: u32) {}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir(to).unwrap();
    private(to, 0o700);
    for item in fs::read_dir(from).unwrap() {
        let item = item.unwrap();
        let target = to.join(item.file_name());
        if item.file_type().unwrap().is_dir() {
            copy_tree(&item.path(), &target);
        } else {
            fs::copy(item.path(), &target).unwrap();
            private(&target, 0o600);
        }
    }
}

/// A temp dir holding a copy of the fixture at `<dir>/store`.
fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    copy_tree(&fixture(), &root);
    (dir, Store::open(root))
}

/// Every file under `root` with its bytes, for proving reads write nothing.
fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for item in fs::read_dir(&dir).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn the_fixture_reads_whole_and_reading_writes_nothing() {
    let (_dir, store) = store();
    let before = snapshot(store.root());
    let entries = store.list_entries(&EntryFilter::default()).unwrap();
    assert!(entries.skipped.is_empty(), "{:?}", entries.skipped);
    let ids: Vec<String> = entries
        .listing
        .items
        .iter()
        .map(|e| e.id.to_string())
        .collect();
    assert_eq!(
        ids,
        ["rituals-1", "preferences-1", "decisions-1", "decisions-2"]
    );
    let sessions = store.list_sessions(&SessionFilter::default()).unwrap();
    assert_eq!(sessions.listing.total, 2);
    assert_eq!(store.proposals().unwrap()[0].id.to_string(), "p-1");
    let tags = store.list_tags().unwrap();
    let rows: Vec<(String, usize, bool)> = tags
        .tags
        .iter()
        .map(|t| (t.name.to_string(), t.entries, t.def.is_some()))
        .collect();
    assert_eq!(
        rows,
        [
            (String::from("decisions"), 2, false),
            (String::from("idea"), 0, true),
            (String::from("preferences"), 1, true),
            (String::from("rituals"), 1, false),
            (String::from("standard"), 0, true),
        ]
    );
    store.entry(&"decisions-2".parse().unwrap()).unwrap();
    store
        .session(&"7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02".parse().unwrap())
        .unwrap();
    assert_eq!(snapshot(store.root()), before);
    assert!(!store.audit_path().exists());
}

#[test]
fn filters_apply_before_the_limit() {
    let (_dir, store) = store();
    let filter = EntryFilter {
        status: Some(EntryStatus::Active),
        limit: NonZeroUsize::new(1),
        ..EntryFilter::default()
    };
    let list = store.list_entries(&filter).unwrap();
    assert_eq!(list.listing.total, 2);
    assert!(list.listing.truncated());
    let by_tag = EntryFilter {
        tag: Some("decisions".parse::<TagName>().unwrap()),
        ..EntryFilter::default()
    };
    assert_eq!(store.list_entries(&by_tag).unwrap().listing.total, 2);
}

#[test]
fn a_stored_quote_passes_the_verbatim_check_against_its_entry() {
    let (_dir, store) = store();
    for proposal in store.proposals().unwrap() {
        for quote in &proposal.quotes {
            let entry = store.entry(&quote.entry).unwrap();
            assert!(
                verbatim::contains(&entry.content, &quote.text),
                "{} does not quote {}",
                proposal.id,
                quote.entry
            );
        }
    }
}

#[test]
fn a_record_from_a_newer_build_is_refused_not_rewritten() {
    let (_dir, store) = store();
    let path = store.entry_path(&"decisions-2".parse().unwrap());
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("format: 1", "format: 15");
    fs::write(&path, &text).unwrap();
    let err = store.list_entries(&EntryFilter::default()).unwrap_err();
    assert!(
        matches!(err, Error::NewerFormat { found: 15, .. }),
        "got {err:?}"
    );
    assert_eq!(err.code(), "store_too_new");
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
}

#[cfg(unix)]
#[test]
fn a_group_writable_root_or_a_linked_record_is_refused() {
    let (_dir, store) = store();
    private(store.root(), 0o770);
    let err = store.list_entries(&EntryFilter::default()).unwrap_err();
    assert_eq!(err.code(), "store_permissions", "got {err:?}");
    private(store.root(), 0o700);

    let path = store.entry_path(&"decisions-2".parse().unwrap());
    let real = store.root().join("decisions-2.md.real");
    fs::rename(&path, &real).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    let err = store.entry(&"decisions-2".parse().unwrap()).unwrap_err();
    assert_eq!(err.code(), "store_symlink", "got {err:?}");
}

#[test]
fn the_redactor_masks_a_value_from_the_store_config() {
    let (_dir, store) = store();
    let config = store.config().unwrap();
    let redactor = Redactor::from_config(&config, |name| {
        (name == "GITHUB_TOKEN").then(|| String::from("ghp_0123456789abcdef"))
    });
    let mut entry = store.entry(&"decisions-2".parse().unwrap()).unwrap();
    entry.content = String::from("token ghp_0123456789abcdef here");
    redactor.entry(&mut entry);
    assert_eq!(entry.content, "token [redacted:GITHUB_TOKEN] here");
}
