//! The error path's stderr: a usage error reported as JSON can quote text
//! the command line or a file carried, including ESC, BEL and CR, so it
//! passes through [`Printable`] on its way to stderr (STD-01 R9): each
//! control character other than a line break or a tab becomes U+FFFD. Payloads
//! and notices are the kit sink's, which escapes them itself.

use std::io::{self, Write};

/// What a neutralized control character prints as.
const REPLACEMENT: &[u8] = "\u{FFFD}".as_bytes();

/// The UTF-8 lead byte of C1 controls (U+0080 to U+009F).
const C1_LEAD: u8 = 0xC2;

/// A writer that neutralizes control characters in what passes through it.
pub(crate) struct Printable<W: Write> {
    inner: W,
    /// A C1 lead byte that ended the previous write: whether it starts a
    /// control depends on the next byte.
    pending_lead: bool,
}

impl<W: Write> Printable<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self {
            inner,
            pending_lead: false,
        }
    }
}

impl<W: Write> Write for Printable<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut out = Vec::with_capacity(buf.len());
        let mut i = 0;
        if self.pending_lead {
            self.pending_lead = false;
            match buf.first() {
                Some(0x80..=0x9F) => {
                    out.extend_from_slice(REPLACEMENT);
                    i = 1;
                }
                _ => out.push(C1_LEAD),
            }
        }
        while i < buf.len() {
            let byte = buf[i];
            match byte {
                b'\n' | b'\t' => out.push(byte),
                0x00..=0x1F | 0x7F => out.extend_from_slice(REPLACEMENT),
                C1_LEAD => match buf.get(i + 1) {
                    Some(0x80..=0x9F) => {
                        out.extend_from_slice(REPLACEMENT);
                        i += 1;
                    }
                    Some(_) => out.push(byte),
                    None => self.pending_lead = true,
                },
                _ => out.push(byte),
            }
            i += 1;
        }
        self.inner.write_all(&out)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending_lead {
            self.pending_lead = false;
            self.inner.write_all(&[C1_LEAD])?;
        }
        self.inner.flush()
    }
}

impl<W: Write> Drop for Printable<W> {
    /// Writes a held C1 lead byte, so nothing written is lost. Fail open: a
    /// stream that has gone away has nothing left to show it on.
    fn drop(&mut self) {
        let _ = self.flush();
    }
}
