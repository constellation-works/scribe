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
