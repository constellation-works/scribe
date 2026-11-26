use crate::error::Error;
use std::time::Duration;

#[test]
fn a_second_lock_times_out_and_names_the_holder() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".lock");
    // The holder is another thread: a thread that asks again for a lock it
    // holds is refused at once (`store_lock_reentrant`), not timed out.
    let (held, release) = std::sync::mpsc::channel::<()>();
    let (ready, taken) = std::sync::mpsc::channel::<()>();
    let holder_path = path.clone();
    let holder = std::thread::spawn(move || {
        let _held =
            cw_fs::Lock::acquire(&holder_path, "first writer", Duration::from_secs(1)).unwrap();
        ready.send(()).unwrap();
        release.recv().unwrap();
    });
    taken.recv().unwrap();
    let err: Error = cw_fs::Lock::acquire(&path, "second writer", Duration::from_millis(60))
        .unwrap_err()
        .into();
    held.send(()).unwrap();
    holder.join().unwrap();
    let Error::Locked { holder, .. } = err else {
        panic!("expected Locked, got {err:?}");
    };
    assert!(
        holder.contains("first writer") && holder.contains(&std::process::id().to_string()),
        "holder should name the label and pid, got {holder:?}"
    );
}

#[test]
fn a_released_lock_names_no_holder_to_a_later_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".lock");
    drop(cw_fs::Lock::acquire(&path, "first writer", Duration::from_secs(1)).unwrap());
    // A holder that has not written its record yet.
    let other = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    other.try_lock().unwrap();
    let err: Error = cw_fs::Lock::acquire(&path, "second writer", Duration::from_millis(60))
        .unwrap_err()
        .into();
    let Error::Locked { holder, .. } = err else {
        panic!("expected Locked, got {err:?}");
    };
    assert_eq!(holder, "an unknown process");
}

#[test]
fn the_kit_s_refusals_keep_the_store_s_codes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("record.json");
    std::fs::write(&path, vec![b'x'; 8]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let too_large: Error = cw_fs::read(&path, cw_fs::Cap::bytes(4)).unwrap_err().into();
    assert_eq!(too_large.code(), "store_too_large", "{too_large:?}");
    let not_regular: Error = cw_fs::inspect_file(dir.path()).unwrap_err().into();
    assert_eq!(not_regular.code(), "store_not_regular", "{not_regular:?}");
    let lock = dir.path().join(".lock");
    let _held = cw_fs::Lock::acquire(&lock, "first", Duration::from_secs(1)).unwrap();
    let again: Error = cw_fs::Lock::acquire(&lock, "again", Duration::from_secs(1))
        .unwrap_err()
        .into();
    assert_eq!(again.code(), "store_lock_reentrant", "{again:?}");
    let missing: Error = cw_fs::read(&dir.path().join("none"), cw_fs::Cap::bytes(4))
        .unwrap_err()
        .into();
    assert_eq!(missing.code(), "io_error", "{missing:?}");
}

#[cfg(unix)]
mod private_state {
    //! STD-05 R7–R9 through the kit: the store's codes and sentences, and
    //! the root that may be a link.
    use crate::db::files;
    use crate::error::Error;
    use std::fs::{self, Permissions};
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn a_file_open_to_others_is_refused_with_its_chmod_fix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");
        fs::write(&path, b"{}").unwrap();
        for loose in [0o664, 0o644, 0o640] {
            fs::set_permissions(&path, Permissions::from_mode(loose)).unwrap();
            let err: Error = cw_fs::inspect_file(&path).unwrap_err().into();
            let Error::Permissions { mode, fix, .. } = err else {
                panic!("expected Permissions for {loose:o}, got {err:?}");
            };
            assert_eq!((mode, fix), (loose, "600"));
            let read: Error = files::read(&path).unwrap_err();
            assert_eq!(read.code(), "store_permissions", "{read:?}");
        }
        fs::set_permissions(&path, Permissions::from_mode(0o600)).unwrap();
        assert!(
            cw_fs::inspect_file(&path).unwrap(),
            "owner-only is accepted"
        );
    }

    #[test]
    fn the_root_may_be_a_link_but_nothing_inside_it_may() {
        let dir = crate::testing::private_tempdir();
        let real = dir.path().join("real");
        cw_fs::create_private_dir(&real).unwrap();
        let root = dir.path().join("root");
        symlink(&real, &root).unwrap();
        assert!(files::inspect_root(&root).unwrap(), "the root is followed");
        fs::write(real.join("a.json"), b"{}").unwrap();
        fs::set_permissions(real.join("a.json"), Permissions::from_mode(0o600)).unwrap();
        assert_eq!(files::list_root(&root).unwrap(), ["a.json"]);
        files::remove(&root, &root.join("a.json")).unwrap();
        assert!(!real.join("a.json").exists());

        let elsewhere = dir.path().join("elsewhere");
        fs::write(&elsewhere, b"precious").unwrap();
        let inner = real.join("data.json");
        symlink(&elsewhere, &inner).unwrap();
        let err: Error = cw_fs::inspect_file(&inner).unwrap_err().into();
        assert!(matches!(err, Error::Symlink { .. }), "got {err:?}");
        let sub = real.join("sub");
        symlink(&elsewhere, &sub).unwrap();
        let err: Error = cw_fs::inspect_dir(&sub).unwrap_err().into();
        assert!(matches!(err, Error::Symlink { .. }), "got {err:?}");
    }

    #[test]
    fn removal_through_a_linked_subdirectory_is_refused() {
        let dir = crate::testing::private_tempdir();
        let real = dir.path().join("real");
        cw_fs::create_private_dir(&real).unwrap();
        let root = dir.path().join("root");
        symlink(&real, &root).unwrap();

        // A directory outside the store, owner-only, holding a file the
        // store must never remove.
        let outside = dir.path().join("outside");
        cw_fs::create_private_dir(&outside).unwrap();
        let target = outside.join("keep.md");
        fs::write(&target, b"precious").unwrap();
        fs::set_permissions(&target, Permissions::from_mode(0o600)).unwrap();

        // `memory/now` inside the store is a link to it.
        let memory = real.join("memory");
        cw_fs::create_private_dir(&memory).unwrap();
        symlink(&outside, memory.join("now")).unwrap();

        let err = files::remove(&root, &root.join("memory/now/keep.md")).unwrap_err();
        assert!(matches!(err, Error::Symlink { .. }), "got {err:?}");
        assert_eq!(err.code(), "store_symlink");
        assert_eq!(fs::read(&target).unwrap(), b"precious");

        // The same with a root that is not a link.
        let err = files::remove(&real, &real.join("memory/now/keep.md")).unwrap_err();
        assert!(matches!(err, Error::Symlink { .. }), "got {err:?}");
        assert_eq!(fs::read(&target).unwrap(), b"precious");
    }

    #[test]
    fn removal_through_a_linked_ancestor_is_refused() {
        // `memory` inside the store is itself a link to an outside
        // owner-only directory holding a real `now` and a file. The file's
        // immediate parent is a real directory, so only a check of every
        // component below the root refuses the removal.
        let dir = crate::testing::private_tempdir();
        let real = dir.path().join("real");
        cw_fs::create_private_dir(&real).unwrap();
        let root = dir.path().join("root");
        symlink(&real, &root).unwrap();

        let outside = dir.path().join("outside");
        cw_fs::create_private_dir(&outside).unwrap();
        cw_fs::create_private_dir(&outside.join("now")).unwrap();
        let target = outside.join("now").join("keep.md");
        fs::write(&target, b"precious").unwrap();
        fs::set_permissions(&target, Permissions::from_mode(0o600)).unwrap();
        symlink(&outside, real.join("memory")).unwrap();

        let err = files::remove(&root, &root.join("memory/now/keep.md")).unwrap_err();
        assert!(matches!(err, Error::Symlink { .. }), "got {err:?}");
        assert_eq!(err.code(), "store_symlink");
        assert_eq!(fs::read(&target).unwrap(), b"precious");

        // The same with a root that is not a link.
        let err = files::remove(&real, &real.join("memory/now/keep.md")).unwrap_err();
        assert!(matches!(err, Error::Symlink { .. }), "got {err:?}");
        assert_eq!(fs::read(&target).unwrap(), b"precious");

        // A `..` below the root would leave it through a real directory;
        // it is refused, and the outside file stays.
        let escape = real.join("../outside/now/keep.md");
        assert!(files::remove(&real, &escape).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"precious");
    }

    #[test]
    fn removal_through_real_directories_still_removes() {
        let dir = crate::testing::private_tempdir();
        let real = dir.path().join("real");
        cw_fs::create_private_dir(&real).unwrap();
        let root = dir.path().join("root");
        symlink(&real, &root).unwrap();
        cw_fs::create_private_dir(&real.join("memory")).unwrap();
        cw_fs::create_private_dir(&real.join("memory").join("now")).unwrap();

        for (store_root, name) in [(&root, "linked.md"), (&real, "plain.md")] {
            let file = real.join("memory").join("now").join(name);
            fs::write(&file, b"{}").unwrap();
            fs::set_permissions(&file, Permissions::from_mode(0o600)).unwrap();
            files::remove(store_root, &store_root.join("memory/now").join(name)).unwrap();
            assert!(!file.exists(), "{name} should be removed");
        }
    }

    #[test]
    fn a_missing_root_is_absent() {
        let dir = crate::testing::private_tempdir();
        let root = dir.path().join("none");
        assert!(!files::inspect_root(&root).unwrap());
        assert!(files::list_root(&root).unwrap().is_empty());
    }
}
