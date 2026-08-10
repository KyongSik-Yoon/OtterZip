//! MSI (OLE2 Compound Document) extract backend.
//!
//! PR-F6 (Phase 7+ option Y). Built on the `cfb` 0.10 crate (MIT,
//! pure-Rust). Read-only by policy; creation modes are gated to
//! `FeatureDisabled` in `backends::writer::open_writer`.
//!
//! Scope (v1.0)
//! ------------
//! An MSI file is a CFB container whose streams hold installer
//! tables (`_StringPool`, `_Tables`, `Property`, ...) plus
//! optional embedded `Binary.<name>` streams (which are often
//! Cabinet payloads). For the v1.0 mainstream-cover slot we
//! flatten **every stream** into a synthetic file with the
//! original CFB path, which is what users typically want when
//! they right-click → Extract on an .msi for inspection.
//!
//! What is *out of scope* in v1.0:
//! * Decoding the obfuscated stream-name characters (MSI uses
//!   characters in the U+3800..U+483F range as a 64-character
//!   alphabet over 8-bit cleartext). The CFB path is reported
//!   verbatim; downstream UI can prettify it later.
//! * Recursing into embedded Binary streams that happen to be
//!   CABs. Users get the raw `.cab` blob; they can re-open it
//!   with OtterZip if they want the inner files. Auto-flattening
//!   is a v1.1 ergonomic improvement.
//! * Storage entries are surfaced as directories so the on-disk
//!   layout matches the CFB tree -- but with no per-storage
//!   metadata (CFB has none).

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use cfb::CompoundFile;

use crate::backends::ArchiveBackend;
use crate::entry::{Entry, HostOs};
use crate::error::{Result, OtterzipError};
use crate::format::{CompressionMethod, EncryptionMethod};

#[derive(Clone)]
struct MsiMeta {
    /// CFB path inside the archive. We translate the MS reserved
    /// `\` separator to forward slash to match the rest of the
    /// codebase's path convention.
    archive_path: String,
    is_storage: bool,
    len: u64,
}

pub(crate) struct MsiBackend {
    path: PathBuf,
    flat: Vec<MsiMeta>,
    cache: RefCell<Option<Vec<Entry>>>,
}

impl MsiBackend {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let comp = open_compound(path)?;

        let mut flat: Vec<MsiMeta> = Vec::new();
        for entry in comp.walk() {
            // walk() emits the root entry first. Skip it so the
            // archive has a sensible relative-path layout.
            let raw = entry.path().to_string_lossy().into_owned();
            if raw == "/" || raw.is_empty() {
                continue;
            }
            let archive_path = normalise_cfb_path(&raw);
            flat.push(MsiMeta {
                archive_path,
                is_storage: entry.is_storage(),
                len: entry.len(),
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            flat,
            cache: RefCell::new(None),
        })
    }
}

fn open_compound(path: &Path) -> Result<CompoundFile<BufReader<File>>> {
    let f = File::open(path).map_err(OtterzipError::Io)?;
    cfb::CompoundFile::open(BufReader::new(f)).map_err(|e| {
        // The CFB header magic is shared with .doc/.xls/.ppt so a
        // mismatch usually means the user pointed us at a non-OLE2
        // file. Surface as UnsupportedFormat so the dispatcher's
        // detect path can still fall back to other backends if a
        // future change re-enters this point.
        if e.kind() == io::ErrorKind::InvalidData {
            OtterzipError::UnsupportedFormat(Some(format!("msi: {e}")))
        } else {
            OtterzipError::BackendError(format!("msi open: {e}"))
        }
    })
}

/// CFB uses backslash as the path separator. Translate to forward
/// slash to match the rest of OtterZip (and trim the leading slash
/// `walk()` emits so callers get a relative archive path).
fn normalise_cfb_path(raw: &str) -> String {
    let trimmed = raw.trim_start_matches('/').trim_start_matches('\\');
    trimmed.replace('\\', "/")
}

fn meta_to_entry(m: &MsiMeta) -> Entry {
    Entry {
        path: m.archive_path.clone(),
        is_directory: m.is_storage,
        is_symlink: false,
        uncompressed_size: if m.is_storage { 0 } else { m.len },
        compressed_size: if m.is_storage { 0 } else { m.len },
        // CFB streams are stored uncompressed; the surrounding MSI
        // metadata may *describe* CAB-compressed payloads but the
        // stream bytes themselves are raw.
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

impl ArchiveBackend for MsiBackend {
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>> {
        if self.cache.borrow().is_none() {
            let v: Vec<Entry> = self.flat.iter().map(meta_to_entry).collect();
            *self.cache.borrow_mut() = Some(v);
        }
        let snapshot = self.cache.borrow().as_ref().unwrap().clone();
        Ok(Box::new(snapshot.into_iter().map(Ok)))
    }

    fn extract_entry(&self, entry_path: &str, out: &mut dyn std::io::Write) -> Result<u64> {
        // Find the metadata first so we can return EntryNotFound
        // without paying for a CFB re-open.
        let meta = self
            .flat
            .iter()
            .find(|m| m.archive_path == entry_path)
            .ok_or_else(|| OtterzipError::EntryNotFound(entry_path.to_string()))?;
        if meta.is_storage {
            // Storage entries (CFB directories) carry no payload of
            // their own; the wrapper creates the directory on disk
            // before calling us.
            return Ok(0);
        }
        let mut comp = open_compound(&self.path)?;
        // CFB's `open_stream` takes a `std::path::Path`, so it only ever
        // splits on the HOST's separator: `/Inner\StreamTwo` resolves on
        // Windows and is one nonsense component named `Inner\StreamTwo` on
        // Unix ("No such stream"). Forward slashes parse identically on both,
        // and are what the crate itself emits from `path_from_name_chain`, so
        // the walk-time normalisation needs no reversing at all — only the
        // leading slash that makes the path absolute within the compound file.
        let cfb_path = format!("/{entry_path}");
        let mut stream = comp
            .open_stream(&cfb_path)
            .map_err(|e| OtterzipError::BackendError(format!("msi open_stream: {e}")))?;
        let bytes = io::copy(&mut stream, out)?;
        Ok(bytes)
    }

    fn open_entry_stream(&self, entry_path: &str) -> Result<Box<dyn Read + Send + '_>> {
        let mut buf = Vec::new();
        self.extract_entry(entry_path, &mut buf)?;
        Ok(Box::new(io::Cursor::new(buf)))
    }
}
