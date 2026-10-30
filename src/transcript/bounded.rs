//! Bound serde's allocations before it materializes a string or segment.
//! Shallow separators delimit header fields and elements of the root segments
//! array. Nested segment contents share one budget; document length is unlimited.
use std::{
    cell::Cell,
    io::{self, Read, Write},
};

pub(super) const LIMIT: usize = 16 * 1024 * 1024;

struct Budget<'a> {
    exceeded: &'a Cell<bool>,
    depth: usize,
    quoted: bool,
    escaped: bool,
    used: usize,
}
impl<'a> Budget<'a> {
    fn new(exceeded: &'a Cell<bool>) -> Self {
        Self {
            exceeded,
            depth: 0,
            quoted: false,
            escaped: false,
            used: 0,
        }
    }
    fn accept(&mut self, byte: u8) -> io::Result<()> {
        // Whitespace outside strings consumes no serde allocation budget.
        if !self.quoted && byte.is_ascii_whitespace() {
            return Ok(());
        }
        self.used += 1;
        if self.used > LIMIT {
            self.exceeded.set(true);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "transcript value size limit",
            ));
        }
        if self.quoted {
            if self.escaped {
                self.escaped = false;
            } else if byte == b'\\' {
                self.escaped = true;
            } else if byte == b'"' {
                self.quoted = false;
            }
        } else {
            match byte {
                b'"' => self.quoted = true,
                b'[' | b'{' => self.depth = self.depth.saturating_add(1),
                b']' | b'}' => self.depth = self.depth.saturating_sub(1),
                b',' | b':' if self.depth <= 2 => self.used = 0,
                _ => {}
            }
        }
        Ok(())
    }
}
pub(super) struct Reader<'a, R> {
    inner: R,
    budget: Budget<'a>,
}
impl<'a, R> Reader<'a, R> {
    pub(super) fn new(inner: R, exceeded: &'a Cell<bool>) -> Self {
        Self {
            inner,
            budget: Budget::new(exceeded),
        }
    }
}
impl<R: Read> Read for Reader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(output)?;
        for &byte in &output[..count] {
            self.budget.accept(byte)?;
        }
        Ok(count)
    }
}

pub(super) struct Writer<'a, W> {
    inner: W,
    budget: Budget<'a>,
}
impl<'a, W> Writer<'a, W> {
    pub(super) fn new(inner: W, exceeded: &'a Cell<bool>) -> Self {
        Self {
            inner,
            budget: Budget::new(exceeded),
        }
    }
}
impl<W: Write> Write for Writer<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for &byte in bytes {
            self.budget.accept(byte)?;
        }
        self.inner.write_all(bytes)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub(super) fn failure() -> Box<dyn std::error::Error + Send + Sync> {
    crate::protocol::failure_details(
        "resource_limit",
        Some(false),
        "transcript value exceeds the encoded size budget",
        serde_json::json!({"stage":"transcript_json","limit_bytes":LIMIT,"preserved":true}),
    )
}
