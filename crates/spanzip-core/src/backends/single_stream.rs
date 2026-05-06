//! Single-stream compression backends — `.gz` / `.bz2` / `.xz` / `.lzma`.
//!
//! These formats compress **exactly one logical file** with no archive
//! metadata of their own (no central directory, no entry list, no
//! per-entry attributes). The backend therefore exposes a synthetic
//! single-entry view:
//!
//! * `entries()` yields one [`Entry`] whose `path` is the source file's
//!   stem (`payload.txt` for `payload.txt.gz`).
//! * `extract_entry` decompresses the whole stream once.
//! * `open_entry_stream` returns the decompressor as a `Read`.
//!
//! Per [`schema.md`](../../../../docs/01-plan/schema.md) §5.2 the
//! `OpenMode::Update` and `add_file`-twice cases are both
//! `FeatureDisabled` for this family; that gate lives in
//! [`crate::archive::Archive`] / [`crate::backends::open_writer`], not
//! here, because the writer side opens a different code path.
//!
//! Phase 7+ option Y (`docs/05-build/phase-7-plus-plan.md` PR-F1) adds
//! `Bzip2` / `Xz` / `Lzma`; the `Gzip` arm was originally
//! `FeatureDisabled` in `backends/mod.rs` despite schema §5.2 listing it
//! as supported — this PR closes that gap by routing all four kinds
//! through the same backend.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use crate::backends::ArchiveBackend;
use crate::entry::{Entry, HostOs};
use crate::error::{Result, SpanzipError};
use crate::format::{CompressionMethod, EncryptionMethod};

/// Which decompression filter to wrap around the source file.
#[derive(Copy, Clone, Debug)]
pub(crate) enum SingleStreamKind {
    Gzip,
    Bzip2,
    Xz,
    /// Raw LZMA1 stream (`.lzma`). Decoded via `xz2` in LZMA1 mode.
    Lzma,
    /// Zstandard frame (`.zst`). PR-F2.
    Zstd,
    /// LZ4 frame format (`.lz4`). PR-F2.
    Lz4,
}

impl SingleStreamKind {
    fn compression_method(self) -> CompressionMethod {
        match self {
            SingleStreamKind::Gzip => CompressionMethod::Deflate,
            SingleStreamKind::Bzip2 => CompressionMethod::Bzip2,
            SingleStreamKind::Xz => CompressionMethod::Lzma2,
            SingleStreamKind::Lzma => CompressionMethod::Lzma,
            SingleStreamKind::Zstd => CompressionMethod::Zstd,
            // CompressionMethod has no LZ4 variant in v7 — frame is a
            // standalone codec that doesn't show up inside ZIP/7z entry
            // metadata. Reuse `Store` as a "no nested compression
            // metadata" sentinel; the per-entry .lz4 file size still
            // reflects the compressed bytes via `compressed_size`.
            SingleStreamKind::Lz4 => CompressionMethod::Store,
        }
    }

    /// Strip the format-specific extension to recover the logical filename.
    /// Falls back to the source file's full name when the extension does
    /// not match the expected suffix (e.g. `archive` with no extension).
    fn strip_extension(self, src: &Path) -> String {
        let name = src
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("payload")
            .to_string();
        let suffixes: &[&str] = match self {
            SingleStreamKind::Gzip => &[".gz", ".GZ", ".gZ", ".Gz"],
            SingleStreamKind::Bzip2 => &[".bz2", ".BZ2", ".Bz2", ".bZ2"],
            SingleStreamKind::Xz => &[".xz", ".XZ", ".Xz", ".xZ"],
            SingleStreamKind::Lzma => &[".lzma", ".LZMA", ".Lzma"],
            SingleStreamKind::Zstd => &[".zst", ".ZST", ".Zst"],
            SingleStreamKind::Lz4 => &[".lz4", ".LZ4", ".Lz4"],
        };
        for s in suffixes {
            if let Some(stripped) = name.strip_suffix(s) {
                if !stripped.is_empty() {
                    return stripped.to_string();
                }
            }
        }
        // Lowercase compare as a final safety net.
        let lower = name.to_ascii_lowercase();
        let lower_suffix = match self {
            SingleStreamKind::Gzip => ".gz",
            SingleStreamKind::Bzip2 => ".bz2",
            SingleStreamKind::Xz => ".xz",
            SingleStreamKind::Lzma => ".lzma",
            SingleStreamKind::Zstd => ".zst",
            SingleStreamKind::Lz4 => ".lz4",
        };
        if let Some(idx) = lower.rfind(lower_suffix) {
            if idx > 0 && idx + lower_suffix.len() == lower.len() {
                return name[..idx].to_string();
            }
        }
        // No recognisable suffix — keep the original name; the caller will
        // still receive a valid (if redundant) extracted file path.
        name
    }
}

pub(crate) struct SingleStreamBackend {
    path: PathBuf,
    kind: SingleStreamKind,
    /// Logical entry name (computed once at open time so `entries()` is cheap).
    entry_name: String,
    /// Source file size in bytes — exposed as the `compressed_size` of the
    /// synthetic entry. The uncompressed size is unknown without decoding,
    /// reported as `0` per the same contract as TAR streaming entries.
    src_size: u64,
    /// Cached metadata so repeated `entries()` calls don't re-stat.
    cache: RefCell<Option<Entry>>,
}

impl SingleStreamBackend {
    pub(crate) fn open(path: &Path, kind: SingleStreamKind) -> Result<Self> {
        // Validate openability up front — `Archive::open` callers expect
        // I/O errors at this point, not lazily on first decode.
        let meta = std::fs::metadata(path)?;
        let src_size = meta.len();
        let entry_name = kind.strip_extension(path);
        Ok(Self {
            path: path.to_path_buf(),
            kind,
            entry_name,
            src_size,
            cache: RefCell::new(None),
        })
    }

    fn open_decoder(&self) -> Result<Box<dyn Read + Send>> {
        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        match self.kind {
            SingleStreamKind::Gzip => Ok(Box::new(flate2::read::GzDecoder::new(reader))),
            SingleStreamKind::Bzip2 => Ok(Box::new(bzip2::read::BzDecoder::new(reader))),
            SingleStreamKind::Xz => Ok(Box::new(xz2::read::XzDecoder::new(reader))),
            // Raw LZMA1 (.lzma) — xz2 exposes `Stream::new_lzma_decoder`
            // which expects the legacy alone-format header. Wrap it
            // through `XzDecoder::new_stream` to keep the io::Read API.
            // Memory limit u64::MAX = "trust the file" (consistent with
            // other backends; ZIP-bomb defence is enforced higher up).
            SingleStreamKind::Lzma => {
                let stream = xz2::stream::Stream::new_lzma_decoder(u64::MAX)
                    .map_err(|e| SpanzipError::BackendError(format!("lzma decoder init: {e}")))?;
                Ok(Box::new(xz2::read::XzDecoder::new_stream(reader, stream)))
            }
            // PR-F2 — Zstandard frame. `zstd::Decoder` is the streaming
            // API; the `?Sized` wrapper on `Read` means we don't lose
            // anything by going through Box<dyn Read + Send>.
            SingleStreamKind::Zstd => {
                let dec = zstd::stream::read::Decoder::new(reader)
                    .map_err(|e| SpanzipError::BackendError(format!("zstd decoder init: {e}")))?;
                Ok(Box::new(dec))
            }
            // PR-F2 — LZ4 frame. `lz4_flex::frame::FrameDecoder` reads
            // the frame header, dictionary, blocks, and trailer in one
            // pass; pure-Rust so no dependency on system liblz4.
            SingleStreamKind::Lz4 => {
                let dec = lz4_flex::frame::FrameDecoder::new(reader);
                Ok(Box::new(dec))
            }
        }
    }

    fn build_entry(&self) -> Entry {
        let modified = std::fs::metadata(&self.path).ok().and_then(|m| m.modified().ok());
        Entry {
            path: self.entry_name.clone(),
            is_directory: false,
            is_symlink: false,
            // Single-stream formats have no length-prefix in their headers
            // (the few that do — like xz with the index field — are not
            // worth a full pre-pass). Report 0 like tar streaming does.
            uncompressed_size: 0,
            compressed_size: self.src_size,
            compression: self.kind.compression_method(),
            encryption: EncryptionMethod::None,
            crc32: None,
            modified,
            accessed: None,
            created: None,
            attributes: 0,
            comment: None,
            host_os: HostOs::Unknown,
        }
    }
}

impl ArchiveBackend for SingleStreamBackend {
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>> {
        if self.cache.borrow().is_none() {
            *self.cache.borrow_mut() = Some(self.build_entry());
        }
        let snapshot = self.cache.borrow().as_ref().unwrap().clone();
        Ok(Box::new(std::iter::once(Ok(snapshot))))
    }

    fn extract_entry(&self, entry_path: &str, out: &mut dyn std::io::Write) -> Result<u64> {
        // The single synthetic entry must match by name. Anything else is
        // a caller bug — return EntryNotFound so the wrapper can surface
        // an actionable error.
        if entry_path != self.entry_name {
            return Err(SpanzipError::EntryNotFound(entry_path.to_string()));
        }
        let mut decoder = self.open_decoder()?;
        let bytes = io::copy(&mut decoder, out)?;
        Ok(bytes)
    }

    fn open_entry_stream(&self, entry_path: &str) -> Result<Box<dyn Read + Send + '_>> {
        if entry_path != self.entry_name {
            return Err(SpanzipError::EntryNotFound(entry_path.to_string()));
        }
        // Buffer the entire output: matching TarBackend's contract avoids
        // lifetime headaches for a callsite that's typically very small
        // (single-stream payloads are usually < 100 MB; if a user hits a
        // multi-GB lzma blob we can revisit with a streaming wrapper).
        let mut buf = Vec::new();
        self.extract_entry(entry_path, &mut buf)?;
        Ok(Box::new(io::Cursor::new(buf)))
    }
}
