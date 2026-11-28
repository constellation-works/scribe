//! Compatibility with a store migrated by the phase 6–7 implementation.

use crate::store::{ExportFormat, Store};
use crate::testing::private_tempdir;
use std::path::Path;

fn compare_tree(expected: &Path, actual: &Path) {
    let mut names: Vec<_> = std::fs::read_dir(expected)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    let mut actual_names: Vec<_> = std::fs::read_dir(actual)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    actual_names.sort();
    assert_eq!(names, actual_names, "{}", actual.display());
    for name in names {
        let expected = expected.join(&name);
        let actual = actual.join(&name);
        if expected.is_dir() {
            compare_tree(&expected, &actual);
        } else {
            assert_eq!(
                std::fs::read(&expected).unwrap(),
                std::fs::read(&actual).unwrap(),
                "{}",
                expected.display()
            );
        }
    }
}

#[test]
fn phase_seven_store_loads_and_exports_unchanged() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/migrated");
    let dir = private_tempdir();
    let root = dir.path().join("store");
    std::fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::copy(fixture.join("scribe.db"), root.join("scribe.db")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join("scribe.db"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let store = Store::open(&root);
    assert_eq!(store.load_sessions().unwrap().records.len(), 1);
    assert_eq!(store.load_entries().unwrap().0.len(), 2);
    assert_eq!(store.proposals().unwrap().len(), 1);
    for (name, format) in [
        ("markdown", ExportFormat::Markdown),
        ("jsonl", ExportFormat::Jsonl),
    ] {
        let target = dir.path().join(name);
        let exported = store.export(&target, format).unwrap();
        assert_eq!(
            (
                exported.sessions,
                exported.entries,
                exported.claims,
                exported.audit_lines
            ),
            (1, 2, 1, 5)
        );
        compare_tree(&fixture.join(name), &target);
    }
    // Writing to the pre-change fixture migrates it cleanly through cw-sqlite
    // with no manual step and preserves existing records.
    store
        .write(|tx| {
            tx.messages().put(
                &[crate::NewMessage {
                    session: "s-1".parse().unwrap(),
                    message: String::from("m-post-cw-sqlite"),
                    role: crate::entry::Speaker::User,
                    at: crate::fixtures::T0,
                    text: String::from("post-migration message"),
                }],
                &crate::Redactor::default(),
            )
        })
        .unwrap();
    assert_eq!(store.load_sessions().unwrap().records.len(), 1);
    assert_eq!(store.load_entries().unwrap().0.len(), 2);
    assert_eq!(store.proposals().unwrap().len(), 1);
    assert_eq!(
        store
            .read()
            .messages()
            .search(&[String::from("post")])
            .unwrap()
            .matches
            .len(),
        1
    );
}
