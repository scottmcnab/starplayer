//! [`ModuleReader`] — the whole of the loader's IO surface.
//!
//! # Synchronous over bytes already obtained
//!
//! Architecture §10: *acquiring* a module's bytes is asynchronous and platform-specific —
//! `fetch` in the browser, a file or an mmap natively, a flash slice on embedded — and
//! that half lives in the platform crates. Loading is then **synchronous** over the bytes
//! it produced, so no core crate needs an async runtime and every loader is a plain
//! function that either returns a `Module` or an [`Error`].
//!
//! The trait is minimal on purpose: `len` and `read_at` are enough to write any loader,
//! and [`ModuleReader::slice_at`] is the zero-copy fast path that a byte source already
//! in memory — which is every source we have today — answers without copying.

use starplayer_core::Error;

/// A byte source a loader can read a module out of.
pub trait ModuleReader {
    /// Total bytes available.
    fn len(&self) -> usize;

    /// Whether the source is empty.
    fn is_empty(&self) -> bool { self.len() == 0 }

    /// Fill `buffer` from `offset`, or fail with [`Error::Truncated`] if the source ends
    /// first. A partial read is never reported as success.
    fn read_at(&self, offset: usize, buffer: &mut [u8]) -> Result<(), Error>;

    /// Borrow `length` bytes at `offset` without copying, if this source can.
    ///
    /// The default is `None`, which means "copy through [`read_at`](ModuleReader::read_at)
    /// instead"; a loader must handle that. Every in-memory source overrides it, and that
    /// is the path S3M pattern decoding takes.
    fn slice_at(&self, offset: usize, length: usize) -> Option<&[u8]> {
        let _ = (offset, length);
        None
    }
}

impl ModuleReader for [u8] {
    fn len(&self) -> usize { <[u8]>::len(self) }

    fn read_at(&self, offset: usize, buffer: &mut [u8]) -> Result<(), Error> {
        let end = offset.checked_add(buffer.len()).ok_or(Error::Truncated { offset, needed: buffer.len() })?;
        let source = self.get(offset..end).ok_or(Error::Truncated { offset, needed: buffer.len() })?;
        buffer.copy_from_slice(source);
        Ok(())
    }

    fn slice_at(&self, offset: usize, length: usize) -> Option<&[u8]> {
        let end = offset.checked_add(length)?;
        self.get(offset..end)
    }
}

impl<T: ModuleReader + ?Sized> ModuleReader for &T {
    fn len(&self) -> usize { T::len(self) }

    fn read_at(&self, offset: usize, buffer: &mut [u8]) -> Result<(), Error> { T::read_at(self, offset, buffer) }

    fn slice_at(&self, offset: usize, length: usize) -> Option<&[u8]> { T::slice_at(self, offset, length) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_byte_slice_reads_and_reports_its_length() {
        let source: &[u8] = &[1, 2, 3, 4];
        let mut buffer = [0u8; 2];

        assert_eq!(source.len(), 4);
        assert!(!source.is_empty());
        assert_eq!(source.read_at(1, &mut buffer), Ok(()));
        assert_eq!(buffer, [2, 3]);
    }

    #[test]
    fn a_read_past_the_end_is_truncated_and_says_where() {
        let source: &[u8] = &[1, 2, 3, 4];
        let mut buffer = [0u8; 3];

        assert_eq!(source.read_at(2, &mut buffer), Err(Error::Truncated { offset: 2, needed: 3 }));
        assert_eq!(source.read_at(usize::MAX, &mut buffer), Err(Error::Truncated { offset: usize::MAX, needed: 3 }));
    }

    #[test]
    fn the_zero_copy_path_borrows_in_range_and_refuses_out_of_range() {
        let source: &[u8] = &[1, 2, 3, 4];

        assert_eq!(source.slice_at(1, 2), Some(&[2u8, 3][..]));
        assert_eq!(source.slice_at(3, 2), None);
        assert_eq!(source.slice_at(4, 0), Some(&[][..]), "an empty slice at the very end is in range");
        assert_eq!(source.slice_at(usize::MAX, 1), None);
    }

    #[test]
    fn an_empty_source_is_empty() {
        let source: &[u8] = &[];
        assert!(source.is_empty());
    }
}
