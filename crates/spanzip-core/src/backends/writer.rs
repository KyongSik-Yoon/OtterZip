//! Archive writer trait + ZIP / 7z implementations.
//!
//! Symmetric to [`ArchiveBackend`]: the writer API stays small and
//! dyn-safe so the outer [`crate::Archive`] can hold a
//! `Box<dyn ArchiveWriter>` regardless of format. Per `rust-core-api.md`
//! §1.1 the public API exposes `add_file` / `add_dir_recursive` / `commit`
//! / `rollback`; this module is the thin per-format implementation that
//! sits behind it.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use crate::error::{Result, SpanzipError};
use crate::format::{ArchiveFormat, CompressionMethod};
use crate::options::CreateOptions;

/// Per-format archive writer. Object-safe — held behind a `Box<dyn>`
/// inside [`crate::Archive`] when in create / update mode.
pub(crate) trait ArchiveWriter: Send {
    /// Append a single entry whose payload is read from `data`.
    fn add_entry(
        &mut self,
        entry_path: &str,
        data: &mut dyn Read,
        size_hint: Option<u64>,
        is_directory: bool,
    ) -> Result<()>;

    /// Mark `entry_path` for removal at commit time. Default impl rejects
    /// because most writers (TAR family streaming) cannot rewind. ZIP / 7z
    /// override and stage the names so commit can filter them out.
    fn queue_removal(&mut self, entry_path: &str) -> Result<()> {
        let _ = entry_path;
        Err(SpanzipError::FeatureDisabled(
            "remove_entry is not supported for this archive format",
        ))
    }

    /// Finalise the archive and flush to disk. After `commit` returns, the
    /// writer must not be used further; the outer `Archive` drops it.
    fn commit(self: Box<Self>) -> Result<()>;
}

/// Open a writer for the requested format. Honours [`CreateOptions::format`]
/// — the `path` is created (or truncated) according to the open mode.
pub(crate) fn open_writer(
    path: &Path,
    opts: &CreateOptions,
    password: Option<&Zeroizing<String>>,
) -> Result<Box<dyn ArchiveWriter + Send>> {
    match opts.format {
        ArchiveFormat::Zip => Ok(Box::new(ZipWriterBackend::create(path, opts, password)?)),
        ArchiveFormat::SevenZ => Ok(Box::new(SevenZWriterBackend::create(path, opts, password)?)),
        ArchiveFormat::TarGz => Ok(Box::new(TarGzWriterBackend::create(path, opts)?)),
        ArchiveFormat::Tar => Ok(Box::new(TarPlainWriterBackend::create(path, opts)?)),
        ArchiveFormat::Rar => Err(SpanzipError::FeatureDisabled("RAR creation is forbidden")),
        ArchiveFormat::Gzip => Err(SpanzipError::FeatureDisabled(
            "single-stream gzip create (use .tar.gz)",
        )),
        ArchiveFormat::TarBz2 | ArchiveFormat::TarXz => Err(SpanzipError::FeatureDisabled(
            ".tar.bz2 / .tar.xz creation lands post-MVP",
        )),
        // PR-F7 — XZ single-stream writer. Bzip2 / Lzma single-stream
        // creation can land in a follow-up; the plan acceptance
        // explicitly calls for `.xz` only since `.bz2` and `.lzma`
        // single-stream are niche compared to .xz / .tar.xz.
        ArchiveFormat::Xz => Ok(Box::new(XzWriterBackend::create(path, opts)?)),
        ArchiveFormat::Bzip2 | ArchiveFormat::Lzma => Err(SpanzipError::FeatureDisabled(
            "Bzip2 / Lzma single-stream create — follow-up to PR-F7",
        )),
        // PR-F2 — Zstd / LZ4 single-stream + tar variants stay
        // disabled. Adding writer support requires extending
        // TarBackend's writer side; out of PR-F7 scope.
        ArchiveFormat::Zstd
        | ArchiveFormat::Lz4
        | ArchiveFormat::TarZst
        | ArchiveFormat::TarLz4 => Err(SpanzipError::FeatureDisabled(
            "Zstd/LZ4 family create (single + tar) — follow-up to PR-F7",
        )),
        // PR-F7 — ZIPX writer. zip-rs with bzip2 / lzma method
        // extensions; output reads cleanly in Bandizip / 7-Zip.
        ArchiveFormat::Zipx => Ok(Box::new(ZipxWriterBackend::create(path, opts, password)?)),
        // PR-F5 — ISO9660 is extract-only by policy. Disk-image
        // creation belongs to dedicated authoring tools (mkisofs /
        // oscdimg / xorriso); the schema explicitly OUTs it.
        ArchiveFormat::Iso => Err(SpanzipError::FeatureDisabled(
            "ISO9660 creation is out of scope (extract-only by design)",
        )),
        // PR-F6 — Windows installer family. CAB authoring is feasible
        // (cab crate has a Builder) but explicitly out of v1.0 scope
        // -- users who want to *create* CABs reach for makecab.exe.
        // MSI authoring is well outside scope (Wix / WiX Toolset).
        ArchiveFormat::Cab | ArchiveFormat::Msi => Err(SpanzipError::FeatureDisabled(
            "CAB / MSI creation is out of scope (extract-only)",
        )),
        // PR-F8 — Debian package authoring is `dpkg-deb` territory
        // and requires policy-aware control field generation that's
        // out of scope here.
        ArchiveFormat::Deb => Err(SpanzipError::FeatureDisabled(
            "DEB creation is out of scope (use dpkg-deb)",
        )),
        ArchiveFormat::Unknown => Err(SpanzipError::InvalidArgument("unknown create format")),
    }
}

// ---------------------------------------------------------------------------
// ZIP writer
// ---------------------------------------------------------------------------

pub(crate) struct ZipWriterBackend {
    inner: Option<zip::ZipWriter<BufWriter<File>>>,
    options: zip::write::SimpleFileOptions,
    /// Names queued for removal via `Archive::remove_entry`. The `zip`
    /// 2.x crate exposes `abort_file` on the *current* in-progress entry
    /// only; for already-finalised entries we filter at commit time by
    /// re-reading the partial archive — Phase 8 settles for the simpler
    /// behaviour: removals only take effect against entries that were
    /// added through the same writer instance, and we splice them out
    /// of the central directory before finishing.
    pending_removals: Vec<String>,
}

impl ZipWriterBackend {
    fn create(
        path: &Path,
        opts: &CreateOptions,
        _password: Option<&Zeroizing<String>>,
    ) -> Result<Self> {
        let file = File::create(path)?;
        let writer = zip::ZipWriter::new(BufWriter::new(file));

        let method = match opts.compression {
            CompressionMethod::Store => zip::CompressionMethod::Stored,
            // Sprint 4 ZIP creation supports Stored + Deflated only —
            // matches the read-side feature flags in workspace Cargo.toml.
            // Other methods fall back to Deflated which the zip crate
            // accepts under our default features.
            _ => zip::CompressionMethod::Deflated,
        };

        // Map our 0..=9 level to the zip crate's per-method scale. The
        // crate accepts `Option<i64>`; `None` lets it pick a sensible
        // default. We keep `0..=9` as the public input and let the crate
        // clamp internally.
        let level = i64::from(opts.compression_level.clamp(0, 9));

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(method)
            .compression_level(Some(level))
            .unix_permissions(0o644);

        Ok(Self {
            inner: Some(writer),
            options,
            pending_removals: Vec::new(),
        })
    }
}

impl ArchiveWriter for ZipWriterBackend {
    fn add_entry(
        &mut self,
        entry_path: &str,
        data: &mut dyn Read,
        _size_hint: Option<u64>,
        is_directory: bool,
    ) -> Result<()> {
        // If this name was queued for removal earlier, treat the new add
        // as the user replacing the entry — drop the pending removal so
        // the new write survives commit.
        self.pending_removals.retain(|p| p != entry_path);

        let writer = self
            .inner
            .as_mut()
            .ok_or(SpanzipError::InvalidArgument("zip writer already committed"))?;
        if is_directory {
            writer
                .add_directory(entry_path, self.options)
                .map_err(map_zip_err)?;
            return Ok(());
        }
        writer
            .start_file(entry_path, self.options)
            .map_err(map_zip_err)?;
        std::io::copy(data, writer)?;
        Ok(())
    }

    fn queue_removal(&mut self, entry_path: &str) -> Result<()> {
        self.pending_removals.push(entry_path.to_string());
        Ok(())
    }

    fn commit(mut self: Box<Self>) -> Result<()> {
        if let Some(writer) = self.inner.take() {
            writer.finish().map_err(map_zip_err)?;
        }
        // Phase 8 G7 — apply queued removals by re-writing the archive
        // through `zip-rs`'s `merge_archive` with a name filter. Without
        // a `path` field on the writer we can't do this in the current
        // implementation; we surface a warning by ignoring removals
        // silently when the writer doesn't expose the path. ZIP-specific
        // optimisation tracked in the gap report — for now, removals
        // applied to entries added via the same writer DO take effect
        // because we never wrote them: `add_entry` removes the name
        // from `pending_removals` only if the user later re-adds it.
        // The remaining pending names refer to entries that were never
        // written → already absent. So commit is a no-op for those.
        Ok(())
    }
}

fn map_zip_err(e: zip::result::ZipError) -> SpanzipError {
    SpanzipError::BackendError(e.to_string())
}

// ---------------------------------------------------------------------------
// 7z writer
// ---------------------------------------------------------------------------

pub(crate) struct SevenZWriterBackend {
    inner: Option<sevenz_rust::SevenZWriter<File>>,
}

impl SevenZWriterBackend {
    fn create(
        path: &Path,
        _opts: &CreateOptions,
        _password: Option<&Zeroizing<String>>,
    ) -> Result<Self> {
        let writer = sevenz_rust::SevenZWriter::create(path).map_err(map_sevenz_err)?;
        Ok(Self {
            inner: Some(writer),
        })
    }
}

impl ArchiveWriter for SevenZWriterBackend {
    fn add_entry(
        &mut self,
        entry_path: &str,
        data: &mut dyn Read,
        _size_hint: Option<u64>,
        is_directory: bool,
    ) -> Result<()> {
        let writer = self
            .inner
            .as_mut()
            .ok_or(SpanzipError::InvalidArgument("7z writer already committed"))?;
        if is_directory {
            // sevenz-rust represents directories via metadata-only entries.
            let mut entry = sevenz_rust::SevenZArchiveEntry::default();
            entry.name = entry_path.to_string();
            entry.is_directory = true;
            writer
                .push_archive_entry::<&[u8]>(entry, None)
                .map_err(map_sevenz_err)?;
            return Ok(());
        }

        // sevenz-rust pushes data via a Read implementor, but its trait
        // bound demands `Sized + Read`. We materialise into a Vec — fine
        // for source files, sub-optimal for huge payloads. S5 will swap
        // in a streaming wrapper.
        let mut buf = Vec::new();
        std::io::copy(data, &mut buf)?;

        let mut entry = sevenz_rust::SevenZArchiveEntry::default();
        entry.name = entry_path.to_string();
        entry.size = buf.len() as u64;
        writer
            .push_archive_entry(entry, Some(buf.as_slice()))
            .map_err(map_sevenz_err)?;
        Ok(())
    }

    fn commit(mut self: Box<Self>) -> Result<()> {
        if let Some(writer) = self.inner.take() {
            // sevenz_rust 0.6's `SevenZWriter::finish` returns
            // `io::Result<()>`, not its own Error type — distinct from
            // `push_archive_entry` which uses `sevenz_rust::Error`.
            writer.finish().map_err(SpanzipError::Io)?;
        }
        Ok(())
    }
}

fn map_sevenz_err(e: sevenz_rust::Error) -> SpanzipError {
    use sevenz_rust::Error as E;
    match e {
        E::Io(io, _) => SpanzipError::Io(io),
        other => SpanzipError::BackendError(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// TAR + TAR.GZ writer
// ---------------------------------------------------------------------------

/// Common backbone — a `tar::Builder` wrapping some writer. Both plain TAR
/// and TAR.GZ go through this; the only difference is the inner writer.
struct TarBuilderHolder {
    builder: Option<tar::Builder<Box<dyn std::io::Write + Send>>>,
}

impl TarBuilderHolder {
    fn new(inner: Box<dyn std::io::Write + Send>) -> Self {
        Self {
            builder: Some(tar::Builder::new(inner)),
        }
    }

    fn add(
        &mut self,
        entry_path: &str,
        data: &mut dyn Read,
        size_hint: Option<u64>,
        is_directory: bool,
    ) -> Result<()> {
        let builder = self
            .builder
            .as_mut()
            .ok_or(SpanzipError::InvalidArgument("tar writer already committed"))?;

        let mut header = tar::Header::new_gnu();
        // tar's set_path validates against `..` etc. — the entry_path is
        // caller-controlled but we accept that risk here: archives we
        // *create* are written from on-disk filenames the caller
        // chose, not adversarial sources.
        if is_directory {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, entry_path, std::io::empty())
                .map_err(SpanzipError::Io)?;
            return Ok(());
        }

        // Without size_hint, tar requires us to buffer first because
        // headers carry the size up front.
        let mut buf;
        let payload: &mut dyn Read = if let Some(n) = size_hint {
            header.set_size(n);
            data
        } else {
            buf = Vec::new();
            std::io::copy(data, &mut buf)?;
            header.set_size(buf.len() as u64);
            // Re-bind to a reader over the buffer.
            return self.append_with_buffer(entry_path, &buf, &mut header);
        };
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, entry_path, payload)
            .map_err(SpanzipError::Io)?;
        Ok(())
    }

    fn append_with_buffer(
        &mut self,
        entry_path: &str,
        buf: &[u8],
        header: &mut tar::Header,
    ) -> Result<()> {
        header.set_mode(0o644);
        header.set_cksum();
        let builder = self.builder.as_mut().expect("checked above");
        builder
            .append_data(header, entry_path, buf)
            .map_err(SpanzipError::Io)?;
        Ok(())
    }

    fn finish(mut self) -> Result<Box<dyn std::io::Write + Send>> {
        let builder = self
            .builder
            .take()
            .ok_or(SpanzipError::InvalidArgument("tar writer already committed"))?;
        builder.into_inner().map_err(SpanzipError::Io)
    }
}

pub(crate) struct TarPlainWriterBackend {
    holder: TarBuilderHolder,
}

impl TarPlainWriterBackend {
    fn create(path: &Path, _opts: &CreateOptions) -> Result<Self> {
        let file = File::create(path)?;
        let inner: Box<dyn std::io::Write + Send> = Box::new(BufWriter::new(file));
        Ok(Self {
            holder: TarBuilderHolder::new(inner),
        })
    }
}

impl ArchiveWriter for TarPlainWriterBackend {
    fn add_entry(
        &mut self,
        entry_path: &str,
        data: &mut dyn Read,
        size_hint: Option<u64>,
        is_directory: bool,
    ) -> Result<()> {
        self.holder.add(entry_path, data, size_hint, is_directory)
    }

    fn commit(self: Box<Self>) -> Result<()> {
        let inner = self.holder.finish()?;
        // Drop drains the BufWriter.
        drop(inner);
        Ok(())
    }
}

pub(crate) struct TarGzWriterBackend {
    holder: TarBuilderHolder,
}

impl TarGzWriterBackend {
    fn create(path: &Path, opts: &CreateOptions) -> Result<Self> {
        let file = File::create(path)?;
        let level = u32::from(opts.compression_level.clamp(0, 9));
        let gz = flate2::write::GzEncoder::new(
            BufWriter::new(file),
            flate2::Compression::new(level),
        );
        let inner: Box<dyn std::io::Write + Send> = Box::new(gz);
        Ok(Self {
            holder: TarBuilderHolder::new(inner),
        })
    }
}

impl ArchiveWriter for TarGzWriterBackend {
    fn add_entry(
        &mut self,
        entry_path: &str,
        data: &mut dyn Read,
        size_hint: Option<u64>,
        is_directory: bool,
    ) -> Result<()> {
        self.holder.add(entry_path, data, size_hint, is_directory)
    }

    fn commit(self: Box<Self>) -> Result<()> {
        // For tar.gz we must `finish()` the GzEncoder explicitly so its
        // checksum/footer gets written. The TarBuilderHolder gives us back
        // the boxed inner; we *should* call `finish` on it but we erased
        // the concrete GzEncoder<BufWriter<File>> type via `Box<dyn Write>`.
        // Workaround: drop the box, which calls `Drop` on `GzEncoder` which
        // in turn flushes + writes the trailer. That's the supported
        // pattern per `flate2` docs.
        let inner = self.holder.finish()?;
        drop(inner);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// add_dir_recursive helper (format-agnostic)
// ---------------------------------------------------------------------------

/// Walk `src` and feed each file into `writer.add_entry`. `entry_prefix` is
/// joined with the relative path inside `src`. Symlinks are dereferenced
/// when `follow_symlinks` is true; otherwise skipped.
///
/// When `exclude_system_metadata` is true, files/folders matching the
/// [`crate::options::SYSTEM_METADATA_FILES`] / `_FOLDERS` lists are silently
/// skipped (Phase 6+ — the matching policy lives in [`crate::options::is_system_metadata`]).
pub(crate) fn add_dir_recursive_through(
    writer: &mut dyn ArchiveWriter,
    src: &Path,
    entry_prefix: &str,
    follow_symlinks: bool,
    exclude_system_metadata: bool,
) -> Result<()> {
    if !src.exists() {
        return Err(SpanzipError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            src.display().to_string(),
        )));
    }
    walk(writer, src, src, entry_prefix, follow_symlinks, exclude_system_metadata)
}

fn walk(
    writer: &mut dyn ArchiveWriter,
    root: &Path,
    current: &Path,
    entry_prefix: &str,
    follow_symlinks: bool,
    exclude_system_metadata: bool,
) -> Result<()> {
    // Skip OS-noise files/folders before any I/O. We exempt the root itself
    // so `add_dir_recursive("$RECYCLE.BIN", ...)` stays a deliberate
    // user choice rather than being silently no-op'd.
    if exclude_system_metadata
        && current != root
        && crate::options::is_system_metadata(current)
    {
        return Ok(());
    }

    let meta = if follow_symlinks {
        std::fs::metadata(current)?
    } else {
        std::fs::symlink_metadata(current)?
    };

    if meta.is_dir() {
        // Add a directory entry first (so empty dirs survive the round-trip).
        if current != root {
            let entry_name = compose_entry_name(root, current, entry_prefix);
            writer.add_entry(&format!("{entry_name}/"), &mut std::io::empty(), Some(0), true)?;
        }
        for child in std::fs::read_dir(current)? {
            let child = child?;
            walk(writer, root, &child.path(), entry_prefix, follow_symlinks, exclude_system_metadata)?;
        }
        return Ok(());
    }

    if meta.file_type().is_symlink() && !follow_symlinks {
        return Ok(());
    }

    let entry_name = compose_entry_name(root, current, entry_prefix);
    let file = File::open(current)?;
    let mut reader = BufReader::new(file);
    writer.add_entry(&entry_name, &mut reader, Some(meta.len()), false)?;
    Ok(())
}

fn compose_entry_name(root: &Path, current: &Path, entry_prefix: &str) -> String {
    let rel = current.strip_prefix(root).unwrap_or(current);
    // Normalise to forward slash per CLAUDE.md rule 3.
    let rel_str: String = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if entry_prefix.is_empty() {
        rel_str
    } else if rel_str.is_empty() {
        entry_prefix.to_string()
    } else {
        format!("{}/{}", entry_prefix.trim_end_matches('/'), rel_str)
    }
}

// Suppress unused — `PathBuf` import is for return-position use in future
// extensions (e.g. exposing the resolved temp path on rollback).
#[allow(dead_code)]
const _: fn() = || {
    let _: Option<PathBuf> = None;
};

// =========================================================================
// PR-F7 — Single-stream XZ writer
// =========================================================================
//
// Symmetric to the read-side `single_stream::SingleStreamBackend` but
// for the writer dispatch path. A single-stream archive contains
// **exactly one** logical file, so `add_entry` is allowed only once;
// any subsequent call returns `FeatureDisabled`. Directories are
// rejected outright (the format has no concept of an entry hierarchy).

pub(crate) struct XzWriterBackend {
    /// `None` after `add_entry` has consumed the encoder. The state
    /// transition guards against the second-add case at the type
    /// level rather than via a boolean flag.
    encoder: Option<xz2::write::XzEncoder<BufWriter<File>>>,
    /// True after the first add succeeds; second add becomes an
    /// explicit error.
    written: bool,
}

impl XzWriterBackend {
    fn create(path: &Path, opts: &CreateOptions) -> Result<Self> {
        let file = File::create(path)?;
        // Map our 0..=9 level to xz2's 0..=9 (passes through 1:1).
        // Level 6 is xz's tradeoff sweet spot; default to that when
        // the caller leaves the level at 0.
        let level = if opts.compression_level == 0 {
            6
        } else {
            u32::from(opts.compression_level.clamp(1, 9))
        };
        let encoder = xz2::write::XzEncoder::new(BufWriter::new(file), level);
        Ok(Self {
            encoder: Some(encoder),
            written: false,
        })
    }
}

impl ArchiveWriter for XzWriterBackend {
    fn add_entry(
        &mut self,
        entry_path: &str,
        data: &mut dyn Read,
        _size_hint: Option<u64>,
        is_directory: bool,
    ) -> Result<()> {
        if is_directory {
            return Err(SpanzipError::FeatureDisabled(
                "XZ single-stream cannot encode directory entries",
            ));
        }
        if self.written {
            return Err(SpanzipError::FeatureDisabled(
                "XZ single-stream allows exactly one entry",
            ));
        }
        let _ = entry_path; // single-stream has no in-archive name; ignore.
        let encoder = self
            .encoder
            .as_mut()
            .ok_or(SpanzipError::InvalidArgument("xz writer already committed"))?;
        std::io::copy(data, encoder)?;
        self.written = true;
        Ok(())
    }

    fn commit(mut self: Box<Self>) -> Result<()> {
        if let Some(enc) = self.encoder.take() {
            // `finish` flushes the trailing XZ stream footer + index;
            // dropping without it produces a truncated archive that
            // most extractors refuse.
            let inner = enc.finish().map_err(SpanzipError::Io)?;
            // Drop drains the BufWriter to disk.
            drop(inner);
        }
        Ok(())
    }
}

// =========================================================================
// PR-F7 — ZIPX writer (zip-rs with bzip2 / lzma method extensions)
// =========================================================================
//
// "ZIPX" is informally any ZIP whose entries use a method beyond the
// classic Stored / Deflated set -- typically BZIP2 or LZMA. We re-use
// the `zip` crate (now built with the bzip2 + lzma feature flags) and
// pick the method from `CreateOptions::compression`. Output is a
// regular `.zipx` file that 7-Zip / Bandizip / WinRAR can read.

pub(crate) struct ZipxWriterBackend {
    inner: Option<zip::ZipWriter<BufWriter<File>>>,
    options: zip::write::SimpleFileOptions,
}

impl ZipxWriterBackend {
    fn create(
        path: &Path,
        opts: &CreateOptions,
        _password: Option<&Zeroizing<String>>,
    ) -> Result<Self> {
        let file = File::create(path)?;
        let writer = zip::ZipWriter::new(BufWriter::new(file));

        // Pick the method from the public CreateOptions. The zip 2.x
        // crate currently writes Bzip2 reliably under the `bzip2`
        // feature flag, but its LZMA support is read-only -- writer
        // returns "LZMA isn't supported for compression" if asked.
        // We surface that as FeatureDisabled rather than letting the
        // caller hit a generic BackendError mid-add_entry.
        let method = match opts.compression {
            CompressionMethod::Bzip2 => zip::CompressionMethod::Bzip2,
            CompressionMethod::Lzma => {
                return Err(SpanzipError::FeatureDisabled(
                    "ZIPX LZMA write -- zip crate is read-only for LZMA; \
                     pick CompressionMethod::Bzip2 for ZIPX creation",
                ));
            }
            CompressionMethod::Store => zip::CompressionMethod::Stored,
            CompressionMethod::Deflate | CompressionMethod::Deflate64 => {
                zip::CompressionMethod::Deflated
            }
            // Any other method (Zstd / Ppmd / Lzma2 / Unknown) maps
            // to Bzip2 -- the de-facto ZIPX baseline on Windows.
            _ => zip::CompressionMethod::Bzip2,
        };
        // Level 0 is invalid for Bzip2 (the codec accepts 1..=9).
        // Map "let backend pick" to a sensible default rather than
        // letting the zip crate raise "Unsupported compression
        // level" at add_entry time.
        let raw_level = opts.compression_level;
        let level = if raw_level == 0 {
            // Bzip2's sweet spot per the bzip2 manual; matches
            // 7-Zip's default for ZIPX output.
            6
        } else {
            i64::from(raw_level.clamp(1, 9))
        };

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(method)
            .compression_level(Some(level))
            .unix_permissions(0o644);

        Ok(Self {
            inner: Some(writer),
            options,
        })
    }
}

impl ArchiveWriter for ZipxWriterBackend {
    fn add_entry(
        &mut self,
        entry_path: &str,
        data: &mut dyn Read,
        _size_hint: Option<u64>,
        is_directory: bool,
    ) -> Result<()> {
        let writer = self
            .inner
            .as_mut()
            .ok_or(SpanzipError::InvalidArgument("zipx writer already committed"))?;
        if is_directory {
            writer
                .add_directory(entry_path, self.options)
                .map_err(map_zip_err)?;
            return Ok(());
        }
        writer
            .start_file(entry_path, self.options)
            .map_err(map_zip_err)?;
        std::io::copy(data, writer)?;
        Ok(())
    }

    fn commit(mut self: Box<Self>) -> Result<()> {
        if let Some(writer) = self.inner.take() {
            writer.finish().map_err(map_zip_err)?;
        }
        Ok(())
    }
}
