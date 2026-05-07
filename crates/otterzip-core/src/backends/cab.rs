//! Microsoft Cabinet (CAB) extract backend.
//!
//! PR-F6 (Phase 7+ option Y). Built on the `cab` 0.6 crate (MIT,
//! pure-Rust). Read-only -- creation modes are gated to
//! `FeatureDisabled` in `backends::writer::open_writer`.
//!
//! `cab::Cabinet` consumes a single `Read + Seek`, decompresses
//! folder data block-by-block on demand, and exposes per-file
//! readers via `read_file(name)`. We materialise the file list once
//! at open time so `entries()` is cheap and `extract_entry` is an
//! O(1) lookup against the cached metadata.
//!
//! Multi-cabinet sets (where a folder spans multiple `.cab` files
//! with `next_cabinet` pointers) are NOT supported in v1.0 -- a
//! folder that references a continuation file fails at first read
//! with the `cab` crate's own error. Single-file CABs (which is
//! what 99% of Windows installers ship) round-trip correctly.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use cab::Cabinet;

use crate::backends::ArchiveBackend;
use crate::entry::{Entry, HostOs};
use crate::error::{Result, OtterzipError};
use crate::format::{CompressionMethod, EncryptionMethod};

/// Minimal metadata snapshot — enough to populate the public
/// [`Entry`] without re-opening the cabinet.
#[derive(Clone)]
struct CabMeta {
    name: String,
    uncompressed_size: u32,
}

pub(crate) struct CabBackend {
    /// Source path; we re-open per `extract_entry` so the cab
    /// reader's `&mut` borrow doesn't fight the `&self` API.
    path: PathBuf,
    flat: Vec<CabMeta>,
    /// Metadata we need to construct synthetic entries even when
    /// the source file is currently busy with another extract.
    cache: RefCell<Option<Vec<Entry>>>,
}

impl CabBackend {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let cab = open_cabinet(path)?;
        let mut flat: Vec<CabMeta> = Vec::new();
        for folder in cab.folder_entries() {
            for file in folder.file_entries() {
                flat.push(CabMeta {
                    name: file.name().to_string(),
                    uncompressed_size: file.uncompressed_size(),
                });
            }
        }
        Ok(Self {
            path: path.to_path_buf(),
            flat,
            cache: RefCell::new(None),
        })
    }
}

fn open_cabinet(path: &Path) -> Result<Cabinet<BufReader<File>>> {
    let f = File::open(path).map_err(OtterzipError::Io)?;
    Cabinet::new(BufReader::new(f)).map_err(|e| {
        // `cab` returns a plain io::Error; unwrap the kind so we
        // surface InvalidData (not-a-CAB) as UnsupportedFormat
        // rather than a generic IO error.
        if e.kind() == io::ErrorKind::InvalidData {
            OtterzipError::UnsupportedFormat(Some(format!("cab: {e}")))
        } else {
            OtterzipError::BackendError(format!("cab open: {e}"))
        }
    })
}

fn meta_to_entry(m: &CabMeta) -> Entry {
    Entry {
        path: m.name.clone(),
        is_directory: false,
        is_symlink: false,
        uncompressed_size: u64::from(m.uncompressed_size),
        // CAB header doesn't carry per-file compressed size; we
        // can't compute it cheaply (the data is interleaved by
        // folder), so report the uncompressed size here. The two
        // sizes are equal for `Compression::None` folders anyway.
        compressed_size: u64::from(m.uncompressed_size),
        // CAB folders use one of MSZIP / Quantum / LZX. The cab
        // crate exposes per-folder compression but we don't push
        // it into per-entry metadata yet (would need a folder-of
        // pointer). Report `Store` as a neutral placeholder; the
        // public API contract treats it as informational.
        compression: CompressionMethod::Store,
        encryption: EncryptionMethod::None,
        crc32: None,
        modified: None,
        accessed: None,
        created: None,
        attributes: 0,
        comment: None,
        host_os: HostOs::Windows,
    }
}

impl ArchiveBackend for CabBackend {
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>> {
        if self.cache.borrow().is_none() {
            let v: Vec<Entry> = self.flat.iter().map(meta_to_entry).collect();
            *self.cache.borrow_mut() = Some(v);
        }
        let snapshot = self.cache.borrow().as_ref().unwrap().clone();
        Ok(Box::new(snapshot.into_iter().map(Ok)))
    }

    fn extract_entry(&self, entry_path: &str, out: &mut dyn std::io::Write) -> Result<u64> {
        // Verify the entry exists in our cached list before
        // touching disk; cheaper than letting the cab crate fail.
        let exists = self.flat.iter().any(|m| m.name == entry_path);
        if !exists {
            return Err(OtterzipError::EntryNotFound(entry_path.to_string()));
        }
        let mut cab = open_cabinet(&self.path)?;
        let mut reader = cab
            .read_file(entry_path)
            .map_err(|e| OtterzipError::BackendError(format!("cab read_file: {e}")))?;
        let bytes = io::copy(&mut reader, out)?;
        Ok(bytes)
    }

    fn open_entry_stream(&self, entry_path: &str) -> Result<Box<dyn Read + Send + '_>> {
        let mut buf = Vec::new();
        self.extract_entry(entry_path, &mut buf)?;
        Ok(Box::new(io::Cursor::new(buf)))
    }
}
