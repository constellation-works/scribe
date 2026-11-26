//! How the store uses `cw-fs`, the kit's file layer (durable writes, private
//! modes, capped reads, the advisory lock; STD-03 R4–R7, STD-05 R7–R9).
//!
//! The mechanisms are the kit's; the store calls them by their own names.
//! This module holds only what is scribe's own: the caps it states for a
//! read, and the one place its data directory differs from the kit's rule,
//! the root, which the operator may name through a symbolic link (the
//! decision "Store directories and files are owner-only"). The kit refuses a
//! link at the directory it is asked about, so the root is resolved first and
//! the kit checks the directory it points to. Nothing inside the root is ever
//! followed through a link.

use crate::error::{Error, Result};
use cw_fs::Cap;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The most bytes the store reads from one record, document, stamp, memory
/// file, config or spooled audit line. Far above anything the store writes;
/// it bounds a file that grew without limit (STD-02 R31).
const RECORD_CAP: Cap = Cap::bytes(64 * 1024 * 1024);

/// The most bytes the store reads from one file of a legacy archive: a
/// session transcript's record or a whole `audit.jsonl`, which grows with
/// every command.
const ARCHIVE_CAP: Cap = Cap::bytes(1024 * 1024 * 1024);

/// Read a store file that `cw_fs::inspect_file` found, within [`RECORD_CAP`].
pub(crate) fn read(path: &Path) -> Result<Vec<u8>> {
    cw_fs::read(path, RECORD_CAP).map_err(Error::from)
}

/// Read a file of a legacy archive, within [`ARCHIVE_CAP`].
pub(crate) fn read_archived(path: &Path) -> Result<Vec<u8>> {
    cw_fs::read(path, ARCHIVE_CAP).map_err(Error::from)
}

/// `root` with a link at its own path resolved: the root may be one, and
/// what is checked is the directory it names. `None` when there is nothing
/// there yet.
fn resolve_root(root: &Path) -> Result<Option<PathBuf>> {
    match std::fs::canonicalize(root) {
        Ok(real) => Ok(Some(real)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io("inspect", root, e)),
    }
}

/// Check the data directory; `Ok(false)` when it does not exist.
pub(crate) fn inspect_root(root: &Path) -> Result<bool> {
    match resolve_root(root)? {
        Some(real) => cw_fs::inspect_dir(&real).map_err(Error::from),
        None => Ok(false),
    }
}

/// The names in the data directory, sorted; none when it does not exist.
pub(crate) fn list_root(root: &Path) -> Result<Vec<String>> {
    match resolve_root(root)? {
        Some(real) => cw_fs::list(&real).map_err(Error::from),
        None => Ok(Vec::new()),
    }
}

/// Remove the store file at `path`, which lies under the data directory
/// `root`, and flush its directory. Only a link at `root` itself is
/// resolved. Every directory between the root and the file is checked
/// before the kit acts: a link at any of them is refused (`store_symlink`),
/// never followed, however high in the tree it sits, and a `..` is refused
/// so the path cannot leave the root. A `path` outside `root` is not
/// resolved at all.
pub(crate) fn remove(root: &Path, path: &Path) -> Result<()> {
    let Ok(rel) = path.strip_prefix(root) else {
        return cw_fs::remove(path).map_err(Error::from);
    };
    let Some(real) = resolve_root(root)? else {
        return Ok(());
    };
    let mut target = real;
    let mut names = rel.components().peekable();
    while let Some(component) = names.next() {
        let Component::Normal(name) = component else {
            let refusal = io::Error::new(
                io::ErrorKind::InvalidInput,
                "the path leaves the data directory",
            );
            return Err(Error::io("remove", path, refusal));
        };
        target.push(name);
        if names.peek().is_none() {
            break;
        }
        match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::Symlink { path: target });
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(Error::io("inspect", &target, e)),
        }
    }
    cw_fs::remove(&target).map_err(Error::from)
}
