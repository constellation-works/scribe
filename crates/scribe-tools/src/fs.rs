//! The crate's file-system access: the only module that names `std::fs`
//! (scripts/check-dependency-direction.sh). Everything else reaches a file
//! through these names, so how scribe opens a host file it does not own
//! (beneath a root, without following a link, without blocking on a pipe)
//! is decided here.

pub use std::fs::{File, Metadata, canonicalize, metadata, read_dir, symlink_metadata};
use std::io;
use std::path::{Component, Path};

/// Whether `e` is the refusal to open a link where a file or directory was
/// expected (`O_NOFOLLOW` on a link, or a directory component that is one).
pub fn is_link_refusal(e: &io::Error) -> bool {
    #[cfg(unix)]
    {
        use rustix::io::Errno;
        matches!(Errno::from_io_error(e), Some(Errno::LOOP | Errno::NOTDIR))
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

/// Open the file at `root` joined with `parts`, every component opened
/// beneath the one before without following a link; the last without
/// blocking on a pipe.
///
/// # Errors
///
/// When a component is missing, is a link, or cannot be opened.
#[cfg(unix)]
pub fn open_beneath(root: &Path, parts: &[Component<'_>]) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open, openat};
    let root = canonicalize(root)?;
    let mut dir = open(
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for (n, part) in parts.iter().enumerate() {
        let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        if n + 1 != parts.len() {
            flags |= OFlags::DIRECTORY;
        } else {
            // A pipe swapped in for the file must not block the open; the
            // type check on the handle refuses it.
            flags |= OFlags::NONBLOCK;
        }
        dir = openat(&dir, part.as_os_str(), flags, Mode::empty())?;
    }
    Ok(File::from(dir))
}

/// [`open_beneath`] off Unix: a plain open of the joined path.
///
/// # Errors
///
/// When the file cannot be opened.
#[cfg(not(unix))]
pub fn open_beneath(root: &Path, parts: &[Component<'_>]) -> io::Result<File> {
    let mut path = root.to_path_buf();
    path.extend(parts.iter().map(|part| part.as_os_str()));
    File::open(path)
}

/// Open `path` read-only. `NOFOLLOW` refuses a link at the last name, and
/// `NONBLOCK` keeps a FIFO from stalling the open; the caller checks the file
/// type on the open handle, so a pipe swapped in is refused, not read.
///
/// # Errors
///
/// When `path` is missing, is a link, or cannot be opened.
#[cfg(unix)]
pub fn open_no_follow(path: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open};
    let fd = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok(File::from(fd))
}

/// [`open_no_follow`] off Unix: a link at `path` is refused before the open.
///
/// # Errors
///
/// When `path` is missing, is a link, or cannot be opened.
#[cfg(not(unix))]
pub fn open_no_follow(path: &Path) -> io::Result<File> {
    if symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the Git metadata file is a link",
        ));
    }
    File::open(path)
}

/// What [`inspect_owned`] found at a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owned {
    /// Nothing is there.
    Missing,
    /// It has the asked-for type, belongs to this user and no other user
    /// can write it.
    Fits,
    /// It is a link, or not the asked-for type.
    WrongType,
    /// It belongs to another user or another user can write it.
    Foreign,
}

/// Inspect `path` without following a link: whether it is a directory
/// (`directory`) or a regular file of this user that no other user can
/// write. On non-unix hosts ownership is not checked.
///
/// This is the rule for a project's own files (`scribe init`), which others
/// may read; it is not the store's owner-only rule, which `cw-fs` applies.
///
/// # Errors
///
/// When the path cannot be inspected for a reason other than its absence.
pub fn inspect_owned(path: &Path, directory: bool) -> io::Result<Owned> {
    let meta = match symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Owned::Missing),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
    {
        return Ok(Owned::WrongType);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o022 != 0 {
            return Ok(Owned::Foreign);
        }
    }
    Ok(Owned::Fits)
}

/// Read the regular file at `path` beneath `root` without following a link
/// in any component, refusing one that is foreign-owned, writable by others
/// or larger than `limit` bytes. `None` when a component is missing.
///
/// A project's own file read beneath its root: `cw-fs` leaves
/// descriptor-relative reads out of the kit, and its capped read demands the
/// store's owner-only mode, which a project file does not have.
///
/// # Errors
///
/// When `path` is not beneath `root`, a component is a link, the file fails
/// the checks above, or it cannot be read.
#[cfg(unix)]
pub fn read_bounded_beneath(root: &Path, path: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    use std::io::Read as _;
    use std::os::unix::fs::MetadataExt;
    let relative = path.strip_prefix(root).map_err(io::Error::other)?;
    let parts: Vec<_> = relative.components().collect();
    if parts
        .iter()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(io::Error::other("non-normal path"));
    }
    let file = match open_beneath(root, &parts) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o022 != 0
    {
        return Err(io::Error::other(
            "opened file is foreign-owned, writable by others or not regular",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(io::Error::other(format!("file exceeds {limit} bytes")));
    }
    Ok(Some(bytes))
}

/// [`read_bounded_beneath`] off Unix: project initialization needs it.
///
/// # Errors
///
/// Always.
#[cfg(not(unix))]
pub fn read_bounded_beneath(
    _root: &Path,
    _path: &Path,
    _limit: u64,
) -> io::Result<Option<Vec<u8>>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe project initialization requires Unix",
    ))
}
