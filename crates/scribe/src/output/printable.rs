//! Stored text reaches a human's terminal or a pipe as text, never as
//! terminal control (STD-01 R9). Entries, quotes, session text and tag
//! descriptions can hold any character a transcript, a model or an agent's
//! argument carried, including ESC, BEL and CR, so every human-mode payload
//! and every notice passes through [`Printable`] on its way to stdout or
//! stderr: each control character other than a line break or a tab becomes
//! U+FFFD.
//!
//! The only escapes that survive are scribe's own color codes, and only
//! when color is on (`allow_color`): exactly the SGR sequences
//! [`super::color::paint`] writes. Machine output (JSON, hook and plugin
//! replies) does not pass through here: its encoder escapes control
//! characters, and its quotes must stay verbatim.

use std::io::{self, Write};

/// What a neutralized control character prints as.
const REPLACEMENT: &[u8] = "\u{FFFD}".as_bytes();

/// The escapes [`super::color::paint`] writes, the only ones let through.
const OWN_SGR: [&[u8]; 4] = [b"\x1b[33m", b"\x1b[32m", b"\x1b[90m", b"\x1b[0m"];

/// The UTF-8 lead byte of C1 controls (U+0080 to U+009F).
const C1_LEAD: u8 = 0xC2;

/// A writer that neutralizes control characters in what passes through it.
pub(crate) struct Printable<W: Write> {
    inner: W,
    allow_color: bool,
    /// A C1 lead byte that ended the previous write: whether it starts a
    /// control depends on the next byte.
    pending_lead: bool,
}

impl<W: Write> Printable<W> {
    pub(crate) fn new(inner: W, allow_color: bool) -> Self {
        Self {
            inner,
            allow_color,
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
                0x1B if self.allow_color => {
                    if let Some(own) = OWN_SGR.iter().find(|sgr| buf[i..].starts_with(sgr)) {
                        out.extend_from_slice(own);
                        i += own.len();
                        continue;
                    }
                    out.extend_from_slice(REPLACEMENT);
                }
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
