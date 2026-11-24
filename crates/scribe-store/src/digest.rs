//! The SHA-256 hashes a cite and a transcript boundary carry, lowercase hex
//! (ORB-27921 ruling (h)). Pure functions; no I/O.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// SHA-256 of a prompt's text, lowercase hex: the cite's `prompt_sha256`,
/// which the `Stop` hook matches against the transcript, and a trigger
/// fire's key in the audit log (2_design.md §11.4).
#[must_use]
pub fn prompt_sha256(prompt: &str) -> String {
    bytes_sha256(prompt.as_bytes())
}

/// SHA-256 of raw bytes, lowercase hex, for checks on a transcript boundary.
#[must_use]
pub fn bytes_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
