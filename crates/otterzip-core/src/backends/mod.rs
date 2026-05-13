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
use crate::error::{Result, OtterzipError};

pub(crate) mod cab;
pub(crate) mod deb;
pub(crate) mod iso;
#[cfg(feature = "libarchive-fallback")]
pub(crate) mod libarchive;
pub(crate) mod msi;
pub(crate) mod multi_volume_reader;
pub(crate) mod sevenz;
pub(crate) mod single_stream;
pub(crate) mod spanned_zip;
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

    /// Cheap is-anything-encrypted probe. Default impl walks
    /// [`Self::entries`] until one reports encryption; backends with
    /// a faster path (libarchive can't enumerate without a full
    /// stream walk, so it short-circuits to "unknown" by returning
    /// false here) override to avoid a multi-second probe on
    /// big archives. Surfaced via [`crate::Archive::is_encrypted`].
    fn is_encrypted_fast(&self) -> Result<bool> {
        for entry in self.entries()? {
            let entry = entry?;
            if entry.encryption != crate::format::EncryptionMethod::None {
                return Ok(true);
            }
        }
        Ok(false)
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

/// Returns true when an error from the strict backend warrants a
/// lenient (libarchive) retry. We intentionally start tight — only
/// errors whose root cause is "the archive bytes don't quite match
/// the spec" qualify. WrongPassword / PathTraversal / Bomb /
/// FeatureDisabled / Io don't trigger fallback because libarchive
/// won't fix them either, just spend another N seconds rejecting.
#[cfg(feature = "libarchive-fallback")]
pub(crate) fn is_recoverable(err: &OtterzipError) -> bool {
    match err {
        OtterzipError::Corrupted { reason, .. } => {
            let r = reason.to_ascii_lowercase();
            // Known zip-crate strict-rejection patterns observed on
            // real-world archives, plus our own deadline trip.
            r.contains("invalid cdfh offset")
                || r.contains("could not find central directory")
                || r.contains("could not find eocd")
                || r.contains("invalid zip archive")
                || r.contains("invalid signature")
                || r.contains("invalid local header")
                || r.contains("data descriptor")
                || r.contains("malformed")
                || r.contains("deadline exceeded")
        }
        // UnsupportedFormat with a message often means "format
        // variant we don't read but libarchive might" (older 7z, RAR
        // read, some tar variants). Bare UnsupportedFormat(None) is
        // detect-layer "no idea what this is" — don't retry.
        OtterzipError::UnsupportedFormat(Some(_)) => true,
        _ => false,
    }
}

/// Build the fallback backend for `path` / `format`. Returns
/// `FeatureDisabled` when the `libarchive-fallback` Cargo feature is
/// off so callers see a clean signal rather than mysterious "no
/// backend" errors.
#[cfg(feature = "libarchive-fallback")]
pub(crate) fn open_fallback_backend(
    path: &Path,
    _format: crate::format::ArchiveFormat,
    password: Option<&Zeroizing<String>>,
) -> Result<Box<dyn ArchiveBackend + Send>> {
    tracing::warn!(
        target: "otterzip::backends",
        path = %path.display(),
        format = ?_format,
        "strict backend rejected — retrying with libarchive fallback"
    );
    Ok(Box::new(self::libarchive::LibarchiveBackend::open(
        path, password,
    )?))
}

#[cfg(not(feature = "libarchive-fallback"))]
pub(crate) fn open_fallback_backend(
    _path: &Path,
    _format: crate::format::ArchiveFormat,
    _password: Option<&Zeroizing<String>>,
) -> Result<Box<dyn ArchiveBackend + Send>> {
    Err(OtterzipError::FeatureDisabled(
        "libarchive-fallback feature is not enabled — recompile with --features libarchive-fallback",
    ))
}

#[cfg(not(feature = "libarchive-fallback"))]
pub(crate) fn is_recoverable(_err: &OtterzipError) -> bool {
    false
}

/// Open the correct backend for the format detected at `path`. When
/// the strict (fast-path) backend returns a recoverable error and
/// the `libarchive-fallback` feature is on, retry once via libarchive
/// so malformed-but-readable archives still extract.
pub(crate) fn open_backend(
    path: &Path,
    format: crate::format::ArchiveFormat,
    password: Option<&Zeroizing<String>>,
) -> Result<Box<dyn ArchiveBackend + Send>> {
    use crate::format::ArchiveFormat as F;
    match format {
        F::Zip | F::Zipx => match self::zip::ZipBackend::open(path, password) {
            Ok(b) => Ok(Box::new(b)),
            Err(e) if is_recoverable(&e) => open_fallback_backend(path, format, password),
            Err(e) => Err(e),
        },
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
        F::TarXz => {
            // PR-F4 — `.tlz` aliases route through this enum slot but
            // carry a different codec (LZMA1 vs LZMA2). The detect
            // layer collapses both onto TarXz so we don't grow the
            // public ABI for an alias; pick the right Compression
            // arm by inspecting the source extension.
            let is_tlz = path
                .extension()
                .and_then(|s| s.to_str())
                .map(|e| e.eq_ignore_ascii_case("tlz"))
                .unwrap_or(false);
            let compression = if is_tlz {
                self::tar_family::Compression::Lzma
            } else {
                self::tar_family::Compression::Xz
            };
            Ok(Box::new(self::tar_family::TarBackend::open(
                path,
                compression,
            )?))
        }
        // Phase 7+ option Y (PR-F1): single-stream backends.
        // GZIP was previously FeatureDisabled despite schema §5.2 listing it
        // as supported — the gap closes here so the four single-stream
        // formats route through the same SingleStreamBackend.
        F::Gzip => Ok(Box::new(self::single_stream::SingleStreamBackend::open(
            path,
            self::single_stream::SingleStreamKind::Gzip,
        )?)),
        F::Bzip2 => Ok(Box::new(self::single_stream::SingleStreamBackend::open(
            path,
            self::single_stream::SingleStreamKind::Bzip2,
        )?)),
        F::Xz => Ok(Box::new(self::single_stream::SingleStreamBackend::open(
            path,
            self::single_stream::SingleStreamKind::Xz,
        )?)),
        F::Lzma => Ok(Box::new(self::single_stream::SingleStreamBackend::open(
            path,
            self::single_stream::SingleStreamKind::Lzma,
        )?)),
        // PR-F2 — Zstd / LZ4 single-stream + tar wrappers.
        F::Zstd => Ok(Box::new(self::single_stream::SingleStreamBackend::open(
            path,
            self::single_stream::SingleStreamKind::Zstd,
        )?)),
        F::Lz4 => Ok(Box::new(self::single_stream::SingleStreamBackend::open(
            path,
            self::single_stream::SingleStreamKind::Lz4,
        )?)),
        F::TarZst => Ok(Box::new(self::tar_family::TarBackend::open(
            path,
            self::tar_family::Compression::Zstd,
        )?)),
        F::TarLz4 => Ok(Box::new(self::tar_family::TarBackend::open(
            path,
            self::tar_family::Compression::Lz4,
        )?)),
        // PR-F5 — ISO9660 disk image (read-only). Joliet + Rock Ridge
        // come from the iso9660-rs `all-extensions` feature flag.
        // UDF goes via the same enum slot but the open path returns
        // UnsupportedFormat with a hint -- see iso::map_iso_err.
        F::Iso => Ok(Box::new(self::iso::IsoBackend::open(path)?)),
        // PR-F6 — Windows installer family (read-only). Both routes
        // surface UnsupportedFormat for non-matching payloads so the
        // detect path can still report "this isn't a real CAB / MSI"
        // without leaking io::Error to callers.
        F::Cab => Ok(Box::new(self::cab::CabBackend::open(path)?)),
        F::Msi => Ok(Box::new(self::msi::MsiBackend::open(path)?)),
        // PR-F8 — Debian package (read-only). Hand-rolled ar parser
        // surfaces the three canonical members verbatim; users
        // re-open the inner data.tar.* with OtterZip for the
        // payload.
        F::Deb => Ok(Box::new(self::deb::DebBackend::open(path)?)),
        F::Rar => Err(OtterzipError::FeatureDisabled("RAR backend (Sprint 4+)")),
        F::Unknown => Err(OtterzipError::UnsupportedFormat(None)),
    }
}
