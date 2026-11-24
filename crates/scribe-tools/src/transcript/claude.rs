//! Read only new Claude JSONL records from a provenance-checked handle.

use crate::fs::Metadata;
use scribe_domain::text::bytes_sha256;
use scribe_store::ClaudeCapturePosition;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;

/// Revalidate the file start and the prior complete record boundary with
/// fixed-size samples even when the transcript itself is very large.
const SAMPLE_BYTES: usize = 256;

pub(super) struct Window {
    pub(super) bytes: Vec<u8>,
    pub(super) resumed: bool,
    pub(super) position: ClaudeCapturePosition,
}

pub(super) fn read_window(
    file: &mut (impl Read + Seek),
    meta: &Metadata,
    prior: Option<&ClaudeCapturePosition>,
) -> io::Result<Option<Window>> {
    let file_id = format!("{}:{}", meta.dev(), meta.ino());
    let modified = format!("{}:{}", meta.mtime(), meta.mtime_nsec());
    let len = meta.len();
    let mut start = 0;
    if let Some(prior) = prior
        && let Ok(offset) = u64::try_from(prior.offset)
        && prior.file_id == file_id
        && (offset < len || (offset == len && prior.modified == modified))
        && matches_boundary(file, offset, len, &prior.boundary)?
    {
        start = offset;
    }
    if start == len {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(len - start).read_to_end(&mut bytes)?;
    let delimited = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let trailing = &bytes[delimited..];
    // A complete final JSON value is readable before its newline arrives.
    // Leave partial JSON for the next hook to read from the same boundary.
    if serde_json::from_slice::<serde::de::IgnoredAny>(trailing).is_err() {
        bytes.truncate(delimited);
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    let offset = start + bytes.len() as u64;
    let (boundary, _) = boundary(file, offset)?;
    Ok(Some(Window {
        bytes,
        resumed: start != 0,
        position: ClaudeCapturePosition {
            file_id,
            offset: i64::try_from(offset).map_err(io::Error::other)?,
            modified,
            boundary,
        },
    }))
}

fn matches_boundary(
    file: &mut (impl Read + Seek),
    offset: u64,
    len: u64,
    expected: &str,
) -> io::Result<bool> {
    let (hash, newline) = boundary(file, offset)?;
    if hash != expected {
        return Ok(false);
    }
    if newline || offset == len {
        return Ok(true);
    }
    // If the last complete JSON value had no newline at EOF, appending
    // must start by terminating its record. Otherwise replay the whole file.
    let mut next = [0];
    file.read_exact(&mut next)?;
    Ok(next == [b'\n'])
}

/// Sample the head and tail, leaving the file at `end`.
fn boundary(file: &mut (impl Read + Seek), end: u64) -> io::Result<(String, bool)> {
    let mut samples = [0; 2 * SAMPLE_BYTES];
    let available = usize::try_from(end).unwrap_or(usize::MAX);
    let head = available.min(SAMPLE_BYTES);
    let tail = (available - head).min(SAMPLE_BYTES);
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut samples[..head])?;
    file.seek(SeekFrom::Start(end - tail as u64))?;
    file.read_exact(&mut samples[head..head + tail])?;
    let samples = &samples[..head + tail];
    Ok((bytes_sha256(samples), samples.last() == Some(&b'\n')))
}
