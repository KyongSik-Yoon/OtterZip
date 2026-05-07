//! Archive handle and operations. See `rust-core-api.md` §1.1.
//!
//! Sprint 1 scope: ZIP read path only. `open_with_password`, `create`,
//! `open_multi`, and mutation APIs return `OtterzipError::UnsupportedFormat`
//! or `FeatureDisabled` — they will be filled in on subsequent sprints.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use zeroize::Zeroizing;

use crate::backends::{
    add_dir_recursive_through, open_backend, open_writer, ArchiveBackend, ArchiveWriter,
    StreamingExtractCtx,
};
use crate::entry::EntryIter;
use crate::error::{Result, OtterzipError};
use crate::format::{detect, ArchiveFormat};
use crate::options::{CreateOptions, ExtractOptions, OverwritePolicy};
use crate::progress::{Progress, ProgressPhase, ProgressSink};

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OpenMode {
    Read = 0,
    CreateNew = 1,
    CreateOrOverwrite = 2,
    Update = 3,
}

/// Top-level archive handle. Opaque to external consumers.
pub struct Archive {
    path: PathBuf,
    format: ArchiveFormat,
    mode: OpenMode,
    /// Held with `zeroize` semantics so the bytes are wiped on drop. The
    /// password lives here for the duration of the handle and is forwarded
    /// to the backend on read; on write it is reserved for future
    /// encrypted-archive creation (Sprint 4+ depending on format).
    #[allow(dead_code)]
    password: Option<Zeroizing<String>>,
    /// Sibling-volume paths discovered by `open_multi`. `None` means
    /// the handle came from `open` (single-volume) — `volume_count`
    /// reports `Some(1)` in that case.
    volumes: Option<Vec<VolumeInfo>>,
    /// Phase 6+ — captured from `CreateOptions::exclude_system_metadata`
    /// at writer creation. Only consulted by `add_dir_recursive`; reader
    /// handles ignore it. `false` for read-mode archives.
    exclude_system_metadata: bool,
    inner: ArchiveInner,
}

/// Internal split between read-side and write-side backends. Public API
/// callers see [`Archive`] uniformly; runtime errors surface when a
/// caller invokes the wrong family for the current mode (e.g. `add_file`
/// on a read-mode archive).
enum ArchiveInner {
    Reader(Box<dyn ArchiveBackend + Send>),
    Writer(Box<dyn ArchiveWriter + Send>),
}

impl std::fmt::Debug for Archive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mode_kind = match self.inner {
            ArchiveInner::Reader(_) => "reader",
            ArchiveInner::Writer(_) => "writer",
        };
        f.debug_struct("Archive")
            .field("path", &self.path)
            .field("format", &self.format)
            .field("mode", &self.mode)
            .field("kind", &mode_kind)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct ExtractReport {
    pub entries_extracted: u32,
    pub entries_skipped: u32,
    pub bytes_written: u64,
    pub warnings: Vec<ExtractWarning>,
    pub elapsed: std::time::Duration,
}

/// Volume metadata for split-archive read (`schema.md` §2.3,
/// `rust-core-api.md` §1.1 `Archive::open_multi`).
#[derive(Debug, Clone)]
pub struct VolumeInfo {
    pub index: u32,
    pub total: Option<u32>,
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// Outcome of [`Archive::test`]. One row per entry visited; corrupted
/// entries surface in `corrupted_entries` so callers can re-prompt the
/// user with specifics rather than a single yes/no flag.
#[derive(Debug, Clone, Default)]
pub struct TestReport {
    pub entries_tested: u32,
    pub entries_corrupted: u32,
    pub corrupted_entries: Vec<String>,
    pub elapsed: std::time::Duration,
}

#[derive(Debug, Clone)]
pub enum ExtractWarning {
    PathTraversalClamped { original: String, clamped: PathBuf },
    EncodingFallback { lossy: String },
    DuplicateEntry { path: String },
    SymlinkSkipped { path: String, target: String },
}

impl Archive {
    /// Open an existing archive for reading.
    ///
    /// Dispatches to the appropriate backend after running magic-byte
    /// format detection. Sprint 3 wires ZIP / 7z / TAR / .tar.gz / .tar.bz2
    /// / .tar.xz; RAR remains unsupported until Sprint 4 (licence review).
    pub fn open(path: impl AsRef<Path>, mode: OpenMode) -> Result<Self> {
        Self::open_inner(path.as_ref(), mode, None)
    }

    /// Open an archive whose entries (or central directory) are encrypted.
    ///
    /// The password is held in [`Zeroizing<String>`] so its bytes are wiped
    /// when the `Archive` drops. Per `domain-model.md` §5 we never log or
    /// format the password value, even on error.
    pub fn open_with_password(
        path: impl AsRef<Path>,
        mode: OpenMode,
        password: impl Into<Zeroizing<String>>,
    ) -> Result<Self> {
        let pw = password.into();
        Self::open_inner(path.as_ref(), mode, Some(pw))
    }

    fn open_inner(
        path: &Path,
        mode: OpenMode,
        password: Option<Zeroizing<String>>,
    ) -> Result<Self> {
        if mode != OpenMode::Read {
            return Err(OtterzipError::FeatureDisabled(
                "open() requires OpenMode::Read; use Archive::create() for write modes",
            ));
        }
        let format = detect(path)?;
        let backend = open_backend(path, format, password.as_ref())?;
        Ok(Self {
            path: path.to_path_buf(),
            format,
            mode,
            password,
            volumes: None,
            exclude_system_metadata: false,
            inner: ArchiveInner::Reader(backend),
        })
    }

    /// Open a split archive given the path of its first volume. Phase 8
    /// G6: discovery only — siblings are listed via `volume_count` /
    /// `volumes()`, but extraction itself stays on the first-volume
    /// backend. Cross-volume reading needs format-specific glue
    /// (ZIP `.z01..zN` / 7z `.7z.001..00N`) that the underlying crates
    /// don't expose for our feature flags yet; backlog item ↓
    pub fn open_multi(first_volume: impl AsRef<Path>, mode: OpenMode) -> Result<Self> {
        let path = first_volume.as_ref();
        let mut archive = Self::open_inner(path, mode, None)?;
        archive.volumes = Some(discover_split_volumes(path));
        Ok(archive)
    }

    /// Number of volumes discovered. `Some(1)` when the archive is
    /// single-volume (the common case), `Some(n)` when `open_multi`
    /// found siblings, `None` when the format does not encode the count.
    #[must_use]
    pub fn volume_count(&self) -> Option<u32> {
        self.volumes
            .as_ref()
            .map(|v| u32::try_from(v.len()).unwrap_or(u32::MAX))
            .or(Some(1))
    }

    /// Per-volume metadata. Empty when the handle came from `open`.
    #[must_use]
    pub fn volumes(&self) -> &[VolumeInfo] {
        self.volumes.as_deref().unwrap_or(&[])
    }

    /// Create a new archive in `CreateNew` or `CreateOrOverwrite` mode.
    ///
    /// The returned handle is in **write mode** — calling [`Archive::extract_all`]
    /// or [`Archive::entries`] on it returns [`OtterzipError::InvalidArgument`].
    /// Use [`Archive::add_file`] / [`Archive::add_dir_recursive`] /
    /// [`Archive::commit`] to populate.
    pub fn create(path: impl AsRef<Path>, opts: CreateOptions) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let password = opts.password.clone();
        let exclude_system_metadata = opts.exclude_system_metadata;
        let writer = open_writer(path, &opts, password.as_ref())?;
        Ok(Self {
            path: path.to_path_buf(),
            format: opts.format,
            mode: OpenMode::CreateOrOverwrite,
            password,
            volumes: None,
            exclude_system_metadata,
            inner: ArchiveInner::Writer(writer),
        })
    }

    /// Append a single file to a write-mode archive.
    pub fn add_file(&mut self, src: impl AsRef<Path>, entry_path: &str) -> Result<()> {
        let writer = self.writer_mut()?;
        let src = src.as_ref();
        let meta = fs::metadata(src)?;
        let file = fs::File::open(src)?;
        let mut reader = std::io::BufReader::new(file);
        writer.add_entry(entry_path, &mut reader, Some(meta.len()), false)
    }

    /// Remove an entry by name from a write-mode archive. Phase 8 G7
    /// scope is **stage-and-commit**: removals queue inside the writer
    /// and apply during `commit`. ZIP/7z writers honour the queue;
    /// streaming TAR writers reject removal with `FeatureDisabled`
    /// because rewinding a streaming sink is not supported.
    pub fn remove_entry(&mut self, entry_path: &str) -> Result<()> {
        let writer = self.writer_mut()?;
        writer.queue_removal(entry_path)
    }

    /// Append a directory recursively. `entry_prefix` is empty for "files
    /// directly at archive root", or e.g. `"docs"` to nest the source under
    /// `docs/...` inside the archive.
    pub fn add_dir_recursive<S: ProgressSink>(
        &mut self,
        src: impl AsRef<Path>,
        entry_prefix: &str,
        progress: Option<&mut S>,
    ) -> Result<()> {
        let exclude_meta = self.exclude_system_metadata;
        let writer = self.writer_mut()?;
        let src = src.as_ref();
        // Sprint 4 keeps progress hooks coarse — we tick once per
        // `add_dir_recursive_through` invocation, which is fine for a
        // first compress flow. Per-file progress wires in S5.
        let _ = progress;
        add_dir_recursive_through(writer, src, entry_prefix, false, exclude_meta)
    }

    /// Finalise a write-mode archive and flush to disk. Consumes the handle.
    pub fn commit(mut self) -> Result<()> {
        match std::mem::replace(
            &mut self.inner,
            ArchiveInner::Reader(Box::new(EmptyBackend)),
        ) {
            ArchiveInner::Writer(w) => w.commit(),
            ArchiveInner::Reader(_) => Err(OtterzipError::InvalidArgument(
                "commit called on a read-mode archive",
            )),
        }
    }

    /// Discard the in-progress writer. Removes the partially-written file
    /// so callers can retry without manual cleanup.
    pub fn rollback(mut self) -> Result<()> {
        // Drop the writer so the file handle releases.
        let path = self.path.clone();
        self.inner = ArchiveInner::Reader(Box::new(EmptyBackend));
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(OtterzipError::Io(e)),
        }
    }

    fn writer_mut(&mut self) -> Result<&mut (dyn ArchiveWriter + Send)> {
        match &mut self.inner {
            ArchiveInner::Writer(w) => Ok(w.as_mut()),
            ArchiveInner::Reader(_) => Err(OtterzipError::InvalidArgument(
                "operation requires write mode (Archive::create)",
            )),
        }
    }

    fn reader(&self) -> Result<&(dyn ArchiveBackend + Send)> {
        match &self.inner {
            ArchiveInner::Reader(r) => Ok(r.as_ref()),
            ArchiveInner::Writer(_) => Err(OtterzipError::InvalidArgument(
                "operation requires read mode (Archive::open)",
            )),
        }
    }

    #[must_use]
    pub const fn format(&self) -> ArchiveFormat {
        self.format
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn mode(&self) -> OpenMode {
        self.mode
    }

    /// Number of entries this archive declares. Cheap on backends that
    /// keep the central directory in memory (ZIP / 7z), `O(n)` linear
    /// scan on streaming-only formats (TAR family).
    pub fn entry_count(&self) -> Result<u64> {
        let n = self.reader()?.entries()?.count();
        Ok(n as u64)
    }

    /// `true` if any entry in the archive declares an encryption method
    /// other than `None`. Walks the entry list (cheap for ZIP/7z, linear
    /// scan for tar.* — but `tar` doesn't carry encryption metadata
    /// anyway, so this returns `false` immediately on those formats).
    pub fn is_encrypted(&self) -> Result<bool> {
        for entry in self.reader()?.entries()? {
            let entry = entry?;
            if entry.encryption != crate::format::EncryptionMethod::None {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `Some(true)` for 7z archives marked solid; `Some(false)` for
    /// non-solid 7z; `None` for any other format (the metric is
    /// 7z-specific). Phase 8 keeps this as a forward-looking metadata
    /// hook — the 7z backend currently always reports `None` because
    /// `sevenz-rust` 0.6 does not surface the solid flag through its
    /// public API. When it does (or we swap to a fork that exposes it),
    /// the implementation lights up here without consumer changes.
    #[must_use]
    pub fn is_solid(&self) -> Option<bool> {
        match self.format {
            ArchiveFormat::SevenZ => None,
            _ => None,
        }
    }

    /// Archive-level comment, if any. ZIP carries an optional EOCD
    /// comment; 7z and TAR do not. Returns `None` for unsupported
    /// formats. Sprint 8: ZIP comment retrieval is best-effort — the
    /// `zip` 2.x crate does not currently expose the EOCD comment via
    /// its high-level API, so this returns `None` until we either
    /// upstream that or parse the EOCD ourselves.
    #[must_use]
    pub fn comment(&self) -> Option<String> {
        None
    }

    /// Walk every entry, decompress its body into a CRC32 sink, and
    /// report which entries fail their stored checksum. For formats that
    /// don't carry per-entry CRCs (TAR family — checksums live in the
    /// outer compressor's frame instead) we still decompress as a
    /// "does it parse" smoke check.
    ///
    /// Per `rust-core-api.md` §1.1 / `ffi-api.md` §10.
    pub fn test<S: ProgressSink>(&self, mut progress: Option<&mut S>) -> Result<TestReport> {
        let start = Instant::now();
        let mut report = TestReport::default();

        let entries: Vec<_> = self
            .reader()?
            .entries()?
            .collect::<Result<Vec<_>>>()?;
        let total_entries = u32::try_from(entries.len()).unwrap_or(u32::MAX);

        for (i, entry) in entries.iter().enumerate() {
            if let Some(sink) = progress.as_deref_mut() {
                let snapshot = Progress {
                    bytes_processed: 0,
                    bytes_total: 0,
                    entries_processed: u32::try_from(i).unwrap_or(u32::MAX),
                    entries_total: total_entries,
                    current_entry: Some(entry.path.clone()),
                    phase: ProgressPhase::Reading,
                    elapsed: start.elapsed(),
                };
                if !sink.update(&snapshot) {
                    return Err(OtterzipError::Canceled);
                }
            }

            if entry.is_directory || entry.is_symlink {
                continue;
            }

            let mut crc_sink = Crc32Sink::new();
            match self.reader()?.extract_entry(&entry.path, &mut crc_sink) {
                Ok(_) => {
                    report.entries_tested += 1;
                    if let Some(expected) = entry.crc32 {
                        if crc_sink.finalize() != expected {
                            report.entries_corrupted += 1;
                            report.corrupted_entries.push(entry.path.clone());
                        }
                    }
                }
                Err(_) => {
                    // Couldn't even decompress — definitely corrupted.
                    report.entries_corrupted += 1;
                    report.corrupted_entries.push(entry.path.clone());
                }
            }
        }

        report.elapsed = start.elapsed();
        Ok(report)
    }

    /// Stream a single entry's decompressed bytes. Implements the
    /// `read_entry` slot from `rust-core-api.md` §1.1.
    ///
    /// The returned reader buffers the entry content in memory for
    /// archives where random access requires consuming the underlying
    /// file (ZIP encrypted entries, sevenz, tar.*); for the plain-ZIP
    /// path it remains a thin Cursor over the decompressed bytes. Phase
    /// 8 ships this as the documented API; true streaming for >1 GiB
    /// entries lands when the backend trait grows a lifetime-aware hook.
    pub fn read_entry(&self, entry_path: &str) -> Result<Box<dyn std::io::Read + Send + '_>> {
        self.reader()?.open_entry_stream(entry_path)
    }

    /// Iterate over all entries in the archive.
    pub fn entries(&self) -> Result<EntryIter<'_>> {
        let inner = self.reader()?.entries()?;
        Ok(EntryIter { inner })
    }

    /// Extract every entry under `opts.destination`.
    ///
    /// Sequential for Sprint 1; Sprint 3 will swap in `rayon` parallelism.
    /// Respects `overwrite`, `block_path_traversal`, and `follow_symlinks`
    /// (the latter currently defaults to *skip* for symlink entries — we
    /// will restore symlinks in Sprint 2 once the platform abstraction
    /// lands).
    #[allow(clippy::too_many_lines)]
    pub fn extract_all<S: ProgressSink>(
        &self,
        opts: &ExtractOptions,
        mut progress: Option<&mut S>,
    ) -> Result<ExtractReport> {
        let start = Instant::now();
        let dest_root = canonical_root(&opts.destination)?;

        let mut report = ExtractReport {
            entries_extracted: 0,
            entries_skipped: 0,
            bytes_written: 0,
            warnings: Vec::new(),
            elapsed: std::time::Duration::ZERO,
        };

        // Streaming-only backends (tar.*) install their own extractor — saves
        // an O(n²) re-scan per entry on big tarballs. We adapt the caller's
        // `Option<&mut S>` to a stable `&mut dyn ProgressSink` by routing
        // through a no-op shim when the caller didn't pass one.
        let mut noop_sink = NoopSink;
        let sink_dyn: &mut dyn ProgressSink = match progress.as_deref_mut() {
            Some(s) => s,
            None => &mut noop_sink,
        };
        // PR-7A: capture source archive's Zone.Identifier once. Cheap
        // (≤1 KB usually). Skipped when the user disabled the toggle so
        // we don't even open the ADS handle.
        let motw_payload = if opts.preserve_zone_identifier {
            crate::motw::read_zone_identifier(&self.path)
        } else {
            None
        };

        let mut ctx = StreamingExtractCtx {
            dest_root: &dest_root,
            opts,
            progress: sink_dyn,
            report: &mut report,
            start,
            motw_payload: motw_payload.as_deref(),
        };
        if let Some(streamed) = self.reader()?.extract_all_streaming(&mut ctx) {
            streamed?;
            report.elapsed = start.elapsed();
            return Ok(report);
        }

        // Default per-entry path (random-access backends: ZIP / 7z).
        let mut entries = Vec::new();
        {
            let iter = self.reader()?.entries()?;
            for item in iter {
                entries.push(item?);
            }
        }

        let total_bytes: u64 = entries.iter().map(|e| e.uncompressed_size).sum();
        let total_entries = u32::try_from(entries.len()).unwrap_or(u32::MAX);
        let mut bomb_monitor = __BombMonitor::new(opts);

        for (i, entry) in entries.iter().enumerate() {
            if let Some(sink) = progress.as_deref_mut() {
                let snapshot = Progress {
                    bytes_processed: report.bytes_written,
                    bytes_total: total_bytes,
                    entries_processed: u32::try_from(i).unwrap_or(u32::MAX),
                    entries_total: total_entries,
                    current_entry: Some(entry.path.clone()),
                    phase: ProgressPhase::Writing,
                    elapsed: start.elapsed(),
                };
                if !sink.update(&snapshot) {
                    return Err(OtterzipError::Canceled);
                }
            }

            // ZIP-bomb gate. `domain-model.md` §5 specifies a pre-extraction
            // ratio check; we only enforce it on real (data-bearing) entries
            // and skip directories / zero-sized files where the metric is
            // either undefined (compressed=0) or trivially huge.
            if let Some(err) = check_zip_bomb(entry, opts) {
                if let OtterzipError::ZipBombSuspected { entry: name, ratio, limit } = &err {
                    tracing::warn!(
                        target: "otterzip::security",
                        event = "zip_bomb_per_entry",
                        entry = %name,
                        ratio = *ratio,
                        limit = *limit,
                        "per-entry compression ratio exceeded threshold"
                    );
                }
                return Err(err);
            }

            if entry.is_symlink && !opts.follow_symlinks {
                report.warnings.push(ExtractWarning::SymlinkSkipped {
                    path: entry.path.clone(),
                    target: String::new(),
                });
                report.entries_skipped += 1;
                continue;
            }

            let out_path = match resolve_output_path(&dest_root, &entry.path, opts) {
                Ok(p) => p,
                Err(Skipped::Traversal(orig)) => {
                    if opts.block_path_traversal {
                        // OWASP A09 trace site — path-traversal block. The
                        // event is rare and security-relevant, so we log
                        // at WARN with the original entry path (not the
                        // resolved one — that's the attacker-controlled
                        // string we want to surface).
                        tracing::warn!(
                            target: "otterzip::security",
                            event = "path_traversal_blocked",
                            entry = %orig,
                            "rejected entry escaping destination root"
                        );
                        return Err(OtterzipError::PathTraversalBlocked(orig));
                    }
                    tracing::warn!(
                        target: "otterzip::security",
                        event = "path_traversal_clamped",
                        entry = %orig,
                        "clamped entry escaping destination root"
                    );
                    report.warnings.push(ExtractWarning::PathTraversalClamped {
                        original: orig,
                        clamped: dest_root.clone(),
                    });
                    report.entries_skipped += 1;
                    continue;
                }
            };

            if entry.is_directory {
                fs::create_dir_all(&out_path)?;
                report.entries_extracted += 1;
                continue;
            }

            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }

            if out_path.exists() {
                match opts.overwrite {
                    OverwritePolicy::Never => {
                        return Err(OtterzipError::Io(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            out_path.display().to_string(),
                        )));
                    }
                    OverwritePolicy::Always => {}
                    OverwritePolicy::IfNewer | OverwritePolicy::AskCallback => {
                        report.entries_skipped += 1;
                        continue;
                    }
                }
            }

            let file = fs::File::create(&out_path)?;
            let mut writer = BufWriter::new(file);
            let written = self.reader()?.extract_entry(&entry.path, &mut writer)?;
            writer.flush()?;
            // PR-7A: propagate Zone.Identifier from source archive.
            // Best-effort — log + continue on ADS failure (likely
            // non-NTFS or owner mismatch). Never aborts extraction.
            if let Some(payload) = motw_payload.as_deref() {
                if let Err(e) = crate::motw::write_zone_identifier(&out_path, payload) {
                    tracing::warn!(
                        target: "otterzip::motw",
                        path = %out_path.display(),
                        error = %e,
                        "failed to propagate Zone.Identifier — file extracted without MOTW"
                    );
                }
            }
            // Cumulative gate. Per-entry `check_zip_bomb` already fired
            // for this entry; here we additionally watch the aggregate so
            // a "many small entries" bomb still trips somewhere in the
            // middle of extraction rather than after exhausting the disk.
            if let Err(err) = bomb_monitor.record(written, entry.compressed_size) {
                if let OtterzipError::ZipBombSuspected { ratio, limit, .. } = &err {
                    tracing::warn!(
                        target: "otterzip::security",
                        event = "zip_bomb_cumulative",
                        ratio = *ratio,
                        limit = *limit,
                        bytes_written = report.bytes_written + written,
                        "cumulative compression ratio or output cap exceeded"
                    );
                }
                return Err(err);
            }
            report.bytes_written += written;
            report.entries_extracted += 1;
        }

        report.elapsed = start.elapsed();
        Ok(report)
    }
}

/// No-op sink used when the caller didn't provide one. Keeps the dyn-trait
/// dispatch in the streaming hook simple.
struct NoopSink;
impl ProgressSink for NoopSink {
    fn update(&mut self, _progress: &Progress) -> bool {
        true
    }
}

/// Look for split-archive siblings next to `first`. Recognises the
/// two common conventions:
///   - ZIP: `name.z01`, `name.z02`, ..., `name.zip` (last)
///   - 7z:  `name.7z.001`, `name.7z.002`, ...
/// Phase 8 G6 stops at *discovery* — the first-volume backend handles
/// extraction. Cross-volume read support depends on backend capability
/// upgrades (tracked in the gap report).
fn discover_split_volumes(first: &Path) -> Vec<VolumeInfo> {
    let mut out = vec![first_volume_info(first, 1)];

    let parent = match first.parent() {
        Some(p) => p,
        None => return out,
    };
    let stem = match first.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return out,
    };

    // 7z: scan stem.7z.NNN for NNN >= 002.
    let mut idx: u32 = 2;
    loop {
        let candidate = parent.join(format!("{stem}.7z.{idx:03}"));
        if !candidate.exists() {
            break;
        }
        out.push(volume_info_at(&candidate, idx));
        idx = idx.saturating_add(1);
        if idx > 999 {
            break;
        }
    }

    // ZIP-style: name.z01, name.z02, ... (the .zip itself counts as last).
    let mut idx: u32 = 1;
    loop {
        let candidate = parent.join(format!("{stem}.z{idx:02}"));
        if !candidate.exists() {
            break;
        }
        out.push(volume_info_at(&candidate, idx + 1));
        idx = idx.saturating_add(1);
        if idx > 99 {
            break;
        }
    }
    out
}

fn first_volume_info(p: &Path, index: u32) -> VolumeInfo {
    VolumeInfo {
        index,
        total: None,
        path: p.to_path_buf(),
        size_bytes: fs::metadata(p).map(|m| m.len()).unwrap_or(0),
    }
}

fn volume_info_at(p: &Path, index: u32) -> VolumeInfo {
    VolumeInfo {
        index,
        total: None,
        path: p.to_path_buf(),
        size_bytes: fs::metadata(p).map(|m| m.len()).unwrap_or(0),
    }
}

/// `std::io::Write` adapter that hashes incoming bytes with CRC-32
/// (IEEE polynomial — same as ZIP / GZip / PNG). Phase 8 G2 keeps a
/// minimal table-free implementation rather than pulling in `crc32fast`
/// for one call site; the test path is not a hot loop and 90 LoC of
/// branchless table init keep the dep graph small.
struct Crc32Sink {
    state: u32,
    table: [u32; 256],
}

impl Crc32Sink {
    fn new() -> Self {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut crc = i as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    0xedb8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
            *slot = crc;
        }
        Self {
            state: 0xffff_ffff,
            table,
        }
    }
    fn finalize(&self) -> u32 {
        self.state ^ 0xffff_ffff
    }
}

impl std::io::Write for Crc32Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for &b in buf {
            let idx = ((self.state ^ u32::from(b)) & 0xff) as usize;
            self.state = self.table[idx] ^ (self.state >> 8);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
/// `ArchiveInner::Writer(_)` slot can be consumed without leaving an
/// invalid value behind. None of its methods are reachable from the
/// public API after the transition.
struct EmptyBackend;
impl ArchiveBackend for EmptyBackend {
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<crate::Entry>> + '_>> {
        Err(OtterzipError::InvalidArgument(
            "archive handle has been committed or rolled back",
        ))
    }
    fn extract_entry(&self, _entry_path: &str, _out: &mut dyn std::io::Write) -> Result<u64> {
        Err(OtterzipError::InvalidArgument(
            "archive handle has been committed or rolled back",
        ))
    }
    fn open_entry_stream(
        &self,
        _entry_path: &str,
    ) -> Result<Box<dyn std::io::Read + Send + '_>> {
        Err(OtterzipError::InvalidArgument(
            "archive handle has been committed or rolled back",
        ))
    }
}

/// Crate-internal alias used by streaming backends so they share a single
/// implementation of the bomb heuristic.
#[doc(hidden)]
pub(crate) fn __check_bomb_for_streaming(
    entry: &crate::Entry,
    opts: &ExtractOptions,
) -> Option<OtterzipError> {
    check_zip_bomb(entry, opts)
}

/// Phase 7 — cumulative bomb gate. Streaming backends call this after
/// each entry write to track aggregate ratio + total output bytes. A
/// fresh `BombMonitor` is created per `extract_all` invocation.
#[doc(hidden)]
pub(crate) struct __BombMonitor {
    pub uncompressed_total: u64,
    pub compressed_total: u64,
    pub max_total_ratio: u32,
    pub max_total_bytes: u64,
}

impl __BombMonitor {
    pub fn new(opts: &ExtractOptions) -> Self {
        Self {
            uncompressed_total: 0,
            compressed_total: 0,
            max_total_ratio: opts.max_total_compression_ratio,
            max_total_bytes: opts.max_total_output_bytes,
        }
    }
    pub fn record(
        &mut self,
        uncompressed: u64,
        compressed: u64,
    ) -> std::result::Result<(), OtterzipError> {
        self.uncompressed_total = self.uncompressed_total.saturating_add(uncompressed);
        self.compressed_total = self.compressed_total.saturating_add(compressed);

        if self.max_total_bytes > 0 && self.uncompressed_total > self.max_total_bytes {
            return Err(OtterzipError::ZipBombSuspected {
                entry: "<aggregate>".to_string(),
                ratio: self.uncompressed_total / self.compressed_total.max(1),
                limit: self.max_total_ratio,
            });
        }
        if self.max_total_ratio > 0 && self.compressed_total > 0 {
            let ratio = self.uncompressed_total / self.compressed_total.max(1);
            if ratio > u64::from(self.max_total_ratio) {
                return Err(OtterzipError::ZipBombSuspected {
                    entry: "<aggregate>".to_string(),
                    ratio,
                    limit: self.max_total_ratio,
                });
            }
        }
        Ok(())
    }
}

/// Returns `Some(err)` when the entry's compression ratio crosses the
/// caller-supplied threshold. Threshold `0` disables the check.
fn check_zip_bomb(
    entry: &crate::Entry,
    opts: &ExtractOptions,
) -> Option<OtterzipError> {
    if opts.max_compression_ratio == 0 {
        return None;
    }
    if entry.is_directory || entry.is_symlink {
        return None;
    }
    if entry.compressed_size == 0 || entry.uncompressed_size == 0 {
        return None;
    }
    let ratio = entry.uncompressed_size / entry.compressed_size.max(1);
    if ratio > u64::from(opts.max_compression_ratio) {
        Some(OtterzipError::ZipBombSuspected {
            entry: entry.path.clone(),
            ratio,
            limit: opts.max_compression_ratio,
        })
    } else {
        None
    }
}

/// Outcome of resolving an entry path to an on-disk output path.
enum Skipped {
    /// The entry's path escapes `destination` via `..` or absolute segments.
    Traversal(String),
}

/// Canonicalize (or create+canonicalize) the destination root so we have a
/// stable prefix to anchor path-traversal checks against. We avoid making
/// the directory if it already exists.
fn canonical_root(dest: &Path) -> Result<PathBuf> {
    if !dest.exists() {
        fs::create_dir_all(dest)?;
    }
    // `canonicalize` on Windows returns `\\?\` extended-length paths, which
    // `Path::starts_with` still handles correctly — the prefix form is
    // consistent across the two paths we compare.
    Ok(fs::canonicalize(dest)?)
}

/// Phase 7 hardening — reject path components that look benign to a Path
/// parser but become weapons on Windows. Returns `Err(reason)` so the
/// caller can map to either `PathTraversalBlocked` (when block flag is on)
/// or a clamped warning.
///
/// Crate-internal so streaming backends (`tar_family`, `sevenz`, parallel
/// ZIP) can reuse the exact same rule set rather than re-implementing it.
#[doc(hidden)]
pub(crate) fn __validate_component(component: &str) -> std::result::Result<(), &'static str> {
    validate_component_phase7(component)
}

fn validate_component_phase7(component: &str) -> std::result::Result<(), &'static str> {
    if component.is_empty() {
        return Err("empty component");
    }
    // Null / control / DEL bytes never belong in a path.
    if component.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return Err("control character in path");
    }
    // Embedded backslash inside what *should* be a single segment — common
    // attacker trick to smuggle traversal past forward-slash splitters.
    if component.contains('\\') {
        return Err("embedded backslash");
    }
    // NTFS Alternate Data Stream syntax: `file.txt:hidden`. Filesystems
    // accept it; we do not.
    if component.contains(':') {
        return Err("colon (NTFS ADS or drive letter)");
    }
    // Windows reserved device names — open as character devices regardless
    // of extension. `CON.txt` resolves to CON.
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.iter().any(|r| *r == stem) {
        return Err("Windows reserved device name");
    }
    // Trailing dot/space silently stripped by Win32 — `evil.txt.` is
    // identical to `evil.txt` once persisted, but the strip happens after
    // path resolution so it can be used to bypass extension blocklists.
    if component.ends_with('.') || component.ends_with(' ') {
        return Err("trailing dot or space");
    }
    Ok(())
}

/// Combine an entry's in-archive path with the destination root.
///
/// Rejects absolute paths and `..` components (per the default
/// `block_path_traversal=true`). When `flatten_paths` is set, only the
/// file name is retained.
fn resolve_output_path(
    dest_root: &Path,
    entry_path: &str,
    opts: &ExtractOptions,
) -> std::result::Result<PathBuf, Skipped> {
    // Normalize: ZIP uses forward-slashes always; replace for OS.
    let as_path = Path::new(entry_path);

    if opts.flatten_paths {
        let name = as_path
            .file_name()
            .map_or_else(|| PathBuf::from(entry_path), PathBuf::from);
        return Ok(dest_root.join(name));
    }

    let mut out = dest_root.to_path_buf();
    for comp in as_path.components() {
        match comp {
            Component::Normal(c) => {
                let s = c.to_string_lossy();
                if validate_component_phase7(&s).is_err() {
                    return Err(Skipped::Traversal(entry_path.to_string()));
                }
                out.push(c);
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => {
                return Err(Skipped::Traversal(entry_path.to_string()));
            }
        }
    }

    // Defense in depth: make sure the final path really starts with the
    // destination root even after OS-level normalization.
    if !out.starts_with(dest_root) {
        return Err(Skipped::Traversal(entry_path.to_string()));
    }
    Ok(out)
}
