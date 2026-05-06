//! Backend dispatch layer.
//!
//! Each supported archive format implements [`ArchiveBackend`]. The
//! [`Archive`](crate::Archive) handle owns a boxed backend and forwards
//! read/extract calls through this trait. Sprint 3 adds `tar` family
//! and `7z` backends in addition to the Sprint 1 ZIP backend.

use std::io::Read;
use std::path::Path;

use zeroize::Zeroizing;

use crate::entry::Entry;
use crate::error::{Result, SpanzipError};

pub(crate) mod sevenz;
pub(crate) mod tar_family;
pub(crate) mod writer;
pub(crate) mod zip;

pub(crate) use writer::{add_dir_recursive_through, open_writer, ArchiveWriter};

/// Trait implemented by every archive-format backend.
///
/// **Dyn-safe on purpose.** The outer `Archive` keeps a `Box<dyn ArchiveBackend>`
/// so format dispatch happens once at open time, not per-entry. Per
/// `performance.md` §5, dynamic dispatch must stay out of hot loops — the
/// inner per-entry iteration happens inside each backend with concrete types.
///
/// All metadata methods take `&self`: backends use interior mutability
/// (`RefCell`) to honour the public `Archive` API contract in
/// `rust-core-api.md` §1.1, which specifies `&self` on `entries`,
/// `read_entry`, and `extract_all`. `Archive` is `!Sync` (§5), so
/// single-threaded borrow discipline is the caller's responsibility.
pub(crate) trait ArchiveBackend: Send {
    /// Enumerate entries.
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>>;

    /// Extract a single entry by its in-archive path string. The caller is
    /// expected to have already performed path-traversal checks.
    fn extract_entry(&self, entry_path: &str, out: &mut dyn std::io::Write) -> Result<u64>;

    /// Stream a single entry as a `Read` (used by `Archive::read_entry`).
    fn open_entry_stream(&self, entry_path: &str) -> Result<Box<dyn Read + Send + '_>>;

    /// Optional override: when `Some`, [`crate::Archive::extract_all`] uses
    /// this rather than the default per-entry loop. Streaming-only formats
    /// (tar.*) provide a streaming implementation here to avoid an
    /// O(n²) re-scan per entry.
    fn extract_all_streaming(
        &self,
        _ctx: &mut StreamingExtractCtx<'_>,
    ) -> Option<Result<()>> {
        None
    }
}

/// Context passed to a streaming extractor. The backend is responsible for
/// honouring overwrite policy / path-traversal / progress hooks itself by
/// going through these helpers — this keeps the security checks centralised
/// rather than re-implementing them per backend.
pub(crate) struct StreamingExtractCtx<'a> {
    pub dest_root: &'a std::path::Path,
    pub opts: &'a crate::options::ExtractOptions,
    pub progress: &'a mut dyn crate::progress::ProgressSink,
    pub report: &'a mut crate::archive::ExtractReport,
    pub start: std::time::Instant,
    /// PR-7A: payload of the source archive's `Zone.Identifier` ADS,
    /// captured once at extract start. `None` when the source has no
    /// MOTW (locally-created) or when `opts.preserve_zone_identifier`
    /// is off. Backends call [`crate::motw::write_zone_identifier`]
    /// against each output file with this payload.
    pub motw_payload: Option<&'a [u8]>,
}

/// Open the correct backend for the format detected at `path`.
pub(crate) fn open_backend(
    path: &Path,
    format: crate::format::ArchiveFormat,
    password: Option<&Zeroizing<String>>,
) -> Result<Box<dyn ArchiveBackend + Send>> {
    use crate::format::ArchiveFormat as F;
    match format {
        F::Zip => Ok(Box::new(self::zip::ZipBackend::open(path, password)?)),
        F::SevenZ => Ok(Box::new(self::sevenz::SevenZBackend::open(path, password)?)),
        F::Tar => Ok(Box::new(self::tar_family::TarBackend::open(
            path,
            self::tar_family::Compression::None,
        )?)),
        F::TarGz => Ok(Box::new(self::tar_family::TarBackend::open(
            path,
            self::tar_family::Compression::Gzip,
        )?)),
        F::TarBz2 => Ok(Box::new(self::tar_family::TarBackend::open(
            path,
            self::tar_family::Compression::Bzip2,
        )?)),
        F::TarXz => Ok(Box::new(self::tar_family::TarBackend::open(
            path,
            self::tar_family::Compression::Xz,
        )?)),
        F::Gzip => Err(SpanzipError::FeatureDisabled(
            "single-stream gzip extraction (use .tar.gz)",
        )),
        F::Rar => Err(SpanzipError::FeatureDisabled("RAR backend (Sprint 4+)")),
        F::Unknown => Err(SpanzipError::UnsupportedFormat(None)),
    }
}
