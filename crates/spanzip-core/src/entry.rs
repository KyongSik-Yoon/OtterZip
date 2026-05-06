//! Archive entries. See `rust-core-api.md` §1.2.

use std::time::SystemTime;

use crate::error::Result;
use crate::format::{CompressionMethod, EncryptionMethod};

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub compression: CompressionMethod,
    pub encryption: EncryptionMethod,
    pub crc32: Option<u32>,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub attributes: u32,
    pub comment: Option<String>,
    pub host_os: HostOs,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum HostOs {
    Unknown = 0,
    Windows = 1,
    Unix = 2,
    Macos = 3,
}

/// Iterator over an archive's entries.
///
/// Produced by [`Archive::entries`](crate::Archive::entries). The iterator
/// borrows the `Archive` immutably; internal mutation happens behind a
/// `RefCell` inside the backend (see `backends/zip.rs`). Backends allocate
/// `Entry` strings as needed — these allocations are acceptable per
/// `performance.md` §5 because enumeration runs once per archive open, not
/// per byte of compressed data (i.e. not a hot loop).
pub struct EntryIter<'a> {
    pub(crate) inner: Box<dyn Iterator<Item = Result<Entry>> + 'a>,
}

impl Iterator for EntryIter<'_> {
    type Item = Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}
