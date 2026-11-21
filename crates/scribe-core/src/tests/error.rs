//! Error formatting and source-chain contracts (`crates/scribe-core/src/error.rs`).

use crate::{AfterPersistError, Error};
use std::error::Error as _;
use std::io;
use std::path::PathBuf;

/// Fold an error and its `std::error::Error::source` chain the way the CLI's
/// `output::report_error` does.
fn format_chain(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[test]
fn after_persist_walks_source_chain_without_repeating_the_inner_error() {
    let io_inner = Error::Io {
        action: "write",
        path: PathBuf::from("/tmp/store/control.json"),
        source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
    };
    let expected_io = format_chain(&io_inner);
    let after_io = Error::AfterPersist(Box::new(AfterPersistError {
        closed: vec![String::from("p-1")],
        source: io_inner,
    }));
    assert_eq!(after_io.code(), "io_error");
    assert_eq!(format_chain(&after_io), expected_io);
    assert!(
        after_io.source().is_some(),
        "underlying io::Error stays on the source chain"
    );

    let symlink_inner = Error::Symlink {
        path: PathBuf::from("/tmp/store/control.json"),
    };
    let expected_symlink = format_chain(&symlink_inner);
    let after_symlink = Error::AfterPersist(Box::new(AfterPersistError {
        closed: vec![String::from("p-1")],
        source: symlink_inner,
    }));
    assert_eq!(after_symlink.code(), "store_symlink");
    assert_eq!(format_chain(&after_symlink), expected_symlink);
    assert!(
        after_symlink.source().is_none(),
        "an inner error with no source exposes no duplicate source"
    );
}
