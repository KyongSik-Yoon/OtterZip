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

use rayon::prelude::*;
use zeroize::Zeroizing;

use crate::backends::zip_writer::{
    self, Compression as ZipCompression, PreparedEntry, WriterOptions as ZipWriterOptions,
    ZipFileWriter,
};
use crate::error::{Result, OtterzipError};
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
        Err(OtterzipError::FeatureDisabled(
            "remove_entry is not supported for this archive format",
        ))
    }

    /// Finalise the archive and flush to disk. After `commit` returns, the
    /// writer must not be used further; the outer `Archive` drops it.
    fn commit(self: Box<Self>) -> Result<()>;

    /// Optional: format-specific bulk directory add. Default returns
    /// `None` — the outer [`add_dir_recursive_through`] then drives
    /// the generic per-entry walker. Backends that can parallelise
    /// the per-entry encode (the in-tree ZIP writer in particular)
    /// override and run a chunked rayon pipeline against the same
    /// directory tree, then either return `Some(Ok(()))` or fall
    /// through to `None` if the workload doesn't justify the
    /// overhead.
    ///
    /// `progress` is passed as `&mut Option<...>` rather than
    /// `Option<&mut ...>` so the borrow stays scoped to the method
    /// body — overriding implementations call `as_deref_mut()`
    /// internally, and the caller retains an undisturbed handle to
    /// drive the fallback walker when `None` comes back.
    fn add_directory_bulk(
        &mut self,
        _src: &Path,
        _entry_prefix: &str,
        _follow_symlinks: bool,
        _exclude_system_metadata: bool,
        _progress: &mut Option<&mut dyn crate::progress::ProgressSink>,
    ) -> Option<Result<()>> {
        None
    }
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
        ArchiveFormat::Rar => Err(OtterzipError::FeatureDisabled("RAR creation is forbidden")),
        ArchiveFormat::Gzip => Err(OtterzipError::FeatureDisabled(
            "single-stream gzip create (use .tar.gz)",
        )),
        ArchiveFormat::TarBz2 | ArchiveFormat::TarXz => Err(OtterzipError::FeatureDisabled(
            ".tar.bz2 / .tar.xz creation lands post-MVP",
        )),
        // PR-F7 — XZ single-stream writer. Bzip2 / Lzma single-stream
        // creation can land in a follow-up; the plan acceptance
        // explicitly calls for `.xz` only since `.bz2` and `.lzma`
        // single-stream are niche compared to .xz / .tar.xz.
        ArchiveFormat::Xz => Ok(Box::new(XzWriterBackend::create(path, opts)?)),
        ArchiveFormat::Bzip2 | ArchiveFormat::Lzma => Err(OtterzipError::FeatureDisabled(
            "Bzip2 / Lzma single-stream create — follow-up to PR-F7",
        )),
        // PR-F2 — Zstd / LZ4 single-stream + tar variants stay
        // disabled. Adding writer support requires extending
        // TarBackend's writer side; out of PR-F7 scope.
        ArchiveFormat::Zstd
        | ArchiveFormat::Lz4
        | ArchiveFormat::TarZst
        | ArchiveFormat::TarLz4 => Err(OtterzipError::FeatureDisabled(
            "Zstd/LZ4 family create (single + tar) — follow-up to PR-F7",
        )),
        // PR-F7 — ZIPX writer. zip-rs with bzip2 / lzma method
        // extensions; output reads cleanly in Bandizip / 7-Zip.
        ArchiveFormat::Zipx => Ok(Box::new(ZipxWriterBackend::create(path, opts, password)?)),
        // PR-F5 — ISO9660 is extract-only by policy. Disk-image
        // creation belongs to dedicated authoring tools (mkisofs /
        // oscdimg / xorriso); the schema explicitly OUTs it.
        ArchiveFormat::Iso => Err(OtterzipError::FeatureDisabled(
            "ISO9660 creation is out of scope (extract-only by design)",
        )),
        // PR-F6 — Windows installer family. CAB authoring is feasible
        // (cab crate has a Builder) but explicitly out of v1.0 scope
        // -- users who want to *create* CABs reach for makecab.exe.
        // MSI authoring is well outside scope (Wix / WiX Toolset).
        ArchiveFormat::Cab | ArchiveFormat::Msi => Err(OtterzipError::FeatureDisabled(
            "CAB / MSI creation is out of scope (extract-only)",
        )),
        // PR-F8 — Debian package authoring is `dpkg-deb` territory
        // and requires policy-aware control field generation that's
        // out of scope here.
        ArchiveFormat::Deb => Err(OtterzipError::FeatureDisabled(
            "DEB creation is out of scope (use dpkg-deb)",
        )),
        ArchiveFormat::Unknown => Err(OtterzipError::InvalidArgument("unknown create format")),
    }
}

// ---------------------------------------------------------------------------
// ZIP writer
// ---------------------------------------------------------------------------

/// ZIP writer backend — wraps the in-tree [`ZipFileWriter`] so the
/// outer `Archive` create / add / commit flow gets the libdeflater
/// dispatch and the rayon-parallel encode pipeline without the
/// upstream `zip` crate's serial encoder in the way.
pub(crate) struct ZipWriterBackend {
    inner: Option<ZipFileWriter>,
    /// Captured at create time so the parallel walker
    /// ([`Self::add_directory_bulk`]) can hand the same compression
    /// settings to every rayon worker without re-deriving them per
    /// chunk.
    compression: ZipCompression,
    /// Names queued for removal via `Archive::remove_entry`. Path B
    /// keeps the existing "drop the queue if user re-adds the name"
    /// semantics — Phase 8 G7's full remove-after-commit path is
    /// still on the backlog.
    pending_removals: Vec<String>,
}

impl ZipWriterBackend {
    fn create(
        path: &Path,
        opts: &CreateOptions,
        _password: Option<&Zeroizing<String>>,
    ) -> Result<Self> {
        let compression = map_compression_method(opts.compression, opts.compression_level);
        let inner = ZipFileWriter::create(path, ZipWriterOptions { compression })?;
        Ok(Self {
            inner: Some(inner),
            compression,
            pending_removals: Vec::new(),
        })
    }

    /// Walk `src` and return three ordered lists: directory entries
    /// (their entry-prefixed names), *small* file entries (suitable
    /// for the rayon worker pool), and *large* file entries (routed
    /// through the main-thread streaming path so they don't balloon
    /// per-worker memory or block a chunk-collect). The split policy
    /// is "file size ≥ `LARGE_ENTRY_THRESHOLD_BYTES` → large".
    fn collect_entries(
        src: &Path,
        entry_prefix: &str,
        follow_symlinks: bool,
        exclude_system_metadata: bool,
    ) -> Result<(Vec<String>, Vec<(PathBuf, String)>, Vec<(PathBuf, String, u64)>)> {
        let mut dirs: Vec<String> = Vec::new();
        let mut small_files: Vec<(PathBuf, String)> = Vec::new();
        let mut large_files: Vec<(PathBuf, String, u64)> = Vec::new();
        let mut stack: Vec<PathBuf> = vec![src.to_path_buf()];
        while let Some(current) = stack.pop() {
            if exclude_system_metadata
                && current != src
                && crate::options::is_system_metadata(&current)
            {
                continue;
            }
            let meta = if follow_symlinks {
                std::fs::metadata(&current)?
            } else {
                std::fs::symlink_metadata(&current)?
            };
            if meta.is_dir() {
                if current != src {
                    let name = compose_entry_name(src, &current, entry_prefix);
                    dirs.push(format!("{name}/"));
                }
                let mut children: Vec<PathBuf> = Vec::new();
                for child in std::fs::read_dir(&current)? {
                    children.push(child?.path());
                }
                // Reverse-push so the pop order matches alphabetical
                // (same convention as the generic walker).
                for child in children.into_iter().rev() {
                    stack.push(child);
                }
                continue;
            }
            if meta.file_type().is_symlink() && !follow_symlinks {
                continue;
            }
            let name = compose_entry_name(src, &current, entry_prefix);
            let size = meta.len();
            if size >= LARGE_ENTRY_THRESHOLD_BYTES {
                large_files.push((current, name, size));
            } else {
                small_files.push((current, name));
            }
        }
        Ok((dirs, small_files, large_files))
    }
}

/// Chunk size for the rayon parallel encode. Each chunk is `par_iter`-
/// processed off-thread and the results collected in input order
/// before the main thread plays them onto the writer. 64 entries
/// keeps the chunk-collect latency low (so the progress sink ticks
/// at least once per ~second on real-world archives) while still
/// amortising the rayon dispatch cost across enough work. Smaller
/// archives never split into multiple chunks; the user's 9 674-
/// entry corpus takes ~150 passes, each ~50 MiB of in-memory
/// deflated bytes worst case — well under 1 GiB.
const PARALLEL_CHUNK_SIZE: usize = 64;

/// Minimum file count for the parallel pipeline to be worth spinning
/// up. Below this we fall back to the serial walker — the rayon
/// thread-pool build cost outweighs 4-worker speed-up on a handful
/// of entries.
const PARALLEL_MIN_ENTRIES: usize = 16;

/// Worker-count cap — NTFS write contention dominates beyond 4
/// workers on the same volume even on 16-core machines, mirroring
/// the read-side `LenientZipBackend::extract_all_parallel` finding.
const PARALLEL_WORKER_CAP: usize = 4;

/// Per-entry uncompressed size above which the bulk dispatcher
/// stops sending the file through the rayon worker pool and instead
/// drives it through the main-thread serial path. Two reasons:
///   1. **Memory**: a worker that reads a 1 GiB file into its
///      Vec-of-bytes buffer can spike per-worker working memory
///      into the multi-GB range, and four of those running at
///      once on a 32 GiB machine pushes the system into swap.
///   2. **Parallel efficiency**: a single multi-GB file occupies
///      one worker for the entire chunk while the other three sit
///      idle. Routing it to the main thread frees those workers
///      to chew through the long tail of small entries.
///
/// 64 MiB picked so the threshold sits well above the libdeflater
/// one-shot limit (16 MiB) — small entries still take the fast
/// libdeflater path, mid-sized entries still hit flate2 streaming
/// in parallel, only the genuinely-large outliers get serialised.
const LARGE_ENTRY_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

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
            .ok_or(OtterzipError::InvalidArgument("zip writer already committed"))?;
        if is_directory {
            writer.add_directory(entry_path)?;
            return Ok(());
        }
        writer.add_entry(entry_path, data)?;
        Ok(())
    }

    fn queue_removal(&mut self, entry_path: &str) -> Result<()> {
        self.pending_removals.push(entry_path.to_string());
        Ok(())
    }

    fn commit(mut self: Box<Self>) -> Result<()> {
        if let Some(writer) = self.inner.take() {
            writer.finish()?;
        }
        // Phase 8 G7 — same caveat as the prior zip-rs implementation:
        // removals through this surface only affect entries that were
        // never actually committed by this writer. Re-applying name
        // filters against a pre-existing archive on disk is post-commit
        // backlog work.
        Ok(())
    }

    /// Chunked rayon-parallel directory walk. Mirrors the read-side
    /// pipeline in `LenientZipBackend::extract_all_parallel` and is
    /// the second half of the v1.1 compress-speed sprint (Path B
    /// commit 2). Returns `None` when the workload is too small to
    /// amortise the rayon pool build cost; the generic walker then
    /// takes over.
    fn add_directory_bulk(
        &mut self,
        src: &Path,
        entry_prefix: &str,
        follow_symlinks: bool,
        exclude_system_metadata: bool,
        progress: &mut Option<&mut dyn crate::progress::ProgressSink>,
    ) -> Option<Result<()>> {
        let (dirs, small_files, large_files) =
            match Self::collect_entries(src, entry_prefix, follow_symlinks, exclude_system_metadata)
            {
                Ok(p) => p,
                Err(e) => return Some(Err(e)),
            };
        let total_files = small_files.len() + large_files.len();
        let total_entries = dirs.len() + total_files;

        if total_files < PARALLEL_MIN_ENTRIES && large_files.is_empty() {
            // Below the threshold the generic per-entry walker is
            // already fast enough; declining `bulk` lets the caller's
            // fallback path take over (carrying its own progress sink
            // through the regular `add_entry` plumbing). No archive
            // bytes have been written yet so this is side-effect-free.
            // We still take over when there's at least one large
            // entry, though — the generic walker would block on it
            // in memory and the streaming path is strictly better.
            tracing::info!(
                target: "otterzip::compress",
                small = small_files.len(),
                large = large_files.len(),
                dirs = dirs.len(),
                "parallel compress declined — below threshold, falling back to serial"
            );
            return None;
        }

        let started = std::time::Instant::now();
        let small_bytes: u64 = small_files
            .iter()
            .filter_map(|(p, _)| std::fs::metadata(p).ok().map(|m| m.len()))
            .sum();
        let large_bytes: u64 = large_files.iter().map(|(_, _, s)| *s).sum();
        let bytes_total = small_bytes + large_bytes;
        tracing::info!(
            target: "otterzip::compress",
            small = small_files.len(),
            small_bytes,
            large = large_files.len(),
            large_bytes,
            dirs = dirs.len(),
            bytes_total,
            "parallel compress dispatch begin"
        );

        // Directory entries are cheap — write them through the
        // sequential surface first so the per-entry LFH offsets in
        // the file batch all sit past the directory CDFH records.
        // (Any sub-directory entries come ahead of their leaf files
        // anyway because the collect_entries walker emits each dir
        // before recursing into its children.)
        let writer = match self.inner.as_mut() {
            Some(w) => w,
            None => {
                return Some(Err(OtterzipError::InvalidArgument(
                    "zip writer already committed",
                )))
            }
        };
        for dir_name in &dirs {
            if let Err(e) = writer.add_directory(dir_name) {
                return Some(Err(e));
            }
        }

        // Initial Scanning tick so the UI moves off 0 % the moment
        // dispatch starts.
        if let Some(sink) = progress.as_deref_mut() {
            let snapshot = crate::progress::Progress {
                bytes_processed: 0,
                bytes_total,
                entries_processed: dirs.len() as u32,
                entries_total: total_entries as u32,
                current_entry: None,
                phase: crate::progress::ProgressPhase::Scanning,
                elapsed: started.elapsed(),
                current_entry_bytes_processed: 0,
                current_entry_bytes_total: 0,
            };
            if !sink.update(&snapshot) {
                return Some(Err(OtterzipError::Canceled));
            }
        }

        // Build a private rayon pool capped at 4 workers — NTFS
        // contention on metadata operations dominates past that even
        // on 16-core machines (same finding as the lenient extract
        // pool). The pool's threads die with the writer drop so a
        // long-lived FFI session doesn't accumulate idle workers.
        let worker_count = std::cmp::min(rayon::current_num_threads(), PARALLEL_WORKER_CAP);
        let pool = match rayon::ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .build()
        {
            Ok(p) => p,
            Err(e) => {
                return Some(Err(OtterzipError::BackendError(format!(
                    "rayon thread-pool build failed: {e}"
                ))));
            }
        };

        let compression = self.compression;
        let (mtime, mdate) = zip_writer::now_dos_datetime();
        let mut entries_done: u32 = dirs.len() as u32;
        let mut bytes_done: u64 = 0;

        // ── Small entries — chunked rayon ─────────────────────────
        //
        // Each chunk is `par_iter`-deflated off-thread and collected
        // in input order, then the main thread plays the results
        // onto the writer serially so LFH byte offsets stay strictly
        // monotonic. Peak in-flight deflated bytes ≈ chunk_size ×
        // worst-case per-entry size; with the 64-entry chunk and
        // ≤64 MiB-per-entry guarantee the bound is ~4 GiB worst
        // case (in practice <100 MiB on the user's corpus). Progress
        // ticks after each chunk so the UI sees motion at least
        // every ~1 second on real workloads.
        //
        // The secondary progress bar (ABI v9 `current_entry_bytes_*`)
        // is filled with the *current chunk*'s byte progress —
        // chunk_bytes_done / chunk_total_bytes. The bar cycles 0 →
        // 100 % once per chunk; combined with the archive-wide bar
        // above it, the UI never collapses to "single bar mode" so
        // there's no jarring layout shift when the dispatcher
        // transitions between small-chunk and large-streaming.
        for chunk in small_files.chunks(PARALLEL_CHUNK_SIZE) {
            // Chunk-wide byte total — used to drive the secondary
            // bar's "current chunk" fraction. Cheap stat() per
            // chunk-entry; the kernel almost certainly has the
            // metadata cached from the walk pass anyway.
            let chunk_total_bytes: u64 = chunk
                .iter()
                .filter_map(|(p, _)| std::fs::metadata(p).ok().map(|m| m.len()))
                .sum();
            let mut chunk_bytes_done: u64 = 0;

            let prepared: Vec<Result<PreparedEntry>> = pool.install(|| {
                chunk
                    .par_iter()
                    .map(|(path, name)| zip_writer::prepare_entry(path, name, compression, mtime, mdate))
                    .collect()
            });
            for result in prepared {
                let entry = match result {
                    Ok(e) => e,
                    Err(e) => return Some(Err(e)),
                };
                let entry_bytes = entry.uncompressed_size;
                let entry_name = String::from_utf8_lossy(&entry.name).into_owned();
                if let Err(e) = writer.add_entry_prepared(entry) {
                    return Some(Err(e));
                }
                entries_done += 1;
                bytes_done += entry_bytes;
                chunk_bytes_done = chunk_bytes_done.saturating_add(entry_bytes);
                if let Some(sink) = progress.as_deref_mut() {
                    let snapshot = crate::progress::Progress {
                        bytes_processed: bytes_done,
                        bytes_total,
                        entries_processed: entries_done,
                        entries_total: total_entries as u32,
                        current_entry: Some(entry_name),
                        phase: crate::progress::ProgressPhase::Writing,
                        elapsed: started.elapsed(),
                        // Small-chunk secondary bar — chunk byte
                        // progress so the UI sees the bar move every
                        // splice tick. Resets implicitly on the next
                        // chunk because chunk_total_bytes changes.
                        current_entry_bytes_processed: chunk_bytes_done,
                        current_entry_bytes_total: chunk_total_bytes,
                    };
                    if !sink.update(&snapshot) {
                        return Some(Err(OtterzipError::Canceled));
                    }
                }
            }
        }

        // ── Large entries — main-thread streaming ────────────────
        //
        // Files past LARGE_ENTRY_THRESHOLD_BYTES (64 MiB) go through
        // ZipFileWriter::add_entry_streaming which deflates straight
        // from disk to disk with a 64 KiB buffer — no GB-sized
        // intermediate `Vec<u8>` per worker, and no chunk-collect
        // block. The streaming encoder calls back here every ~1 MiB
        // so the UI sees mid-entry byte progress (the whole point
        // of the second progress bar in the host).
        //
        // Order: directories first, then small entries' CDFH records
        // already pushed above, then large entries' CDFH records
        // appended here. External readers ignore CDFH order anyway;
        // what matters is the byte-cursor monotonicity for the
        // local_header_offset field, and the writer's `cursor`
        // bookkeeping guarantees that.
        for (path, name, size) in &large_files {
            let entry_name_owned = name.clone();
            let entry_bytes_total = *size;
            let entry_start_bytes_done = bytes_done;
            let started_local = started;
            // Cancellation + progress hooks are accessed through the
            // borrowed `progress` Option. We can't move it through
            // the FnMut, so the closure re-fetches each tick.
            let progress_ref: *mut Option<&mut dyn crate::progress::ProgressSink> = &mut *progress;
            let total_entries_u32 = total_entries as u32;
            let bytes_total_local = bytes_total;
            let entries_done_at_start = entries_done;

            let mut on_tick = |bytes_in_entry: u64| -> Result<()> {
                // SAFETY: the streaming encoder runs synchronously on
                // this thread inside add_entry_streaming, so the
                // closure can't outlive the borrow. We use a raw
                // pointer because moving `progress` through the
                // closure would conflict with the &mut self on
                // `writer` directly above (which holds a borrow on
                // `self`, and `self` also holds the rayon `pool`).
                let progress_opt = unsafe { &mut *progress_ref };
                if let Some(sink) = progress_opt.as_deref_mut() {
                    let snapshot = crate::progress::Progress {
                        bytes_processed: entry_start_bytes_done + bytes_in_entry,
                        bytes_total: bytes_total_local,
                        entries_processed: entries_done_at_start,
                        entries_total: total_entries_u32,
                        current_entry: Some(entry_name_owned.clone()),
                        phase: crate::progress::ProgressPhase::Writing,
                        elapsed: started_local.elapsed(),
                        current_entry_bytes_processed: bytes_in_entry,
                        current_entry_bytes_total: entry_bytes_total,
                    };
                    if !sink.update(&snapshot) {
                        return Err(OtterzipError::Canceled);
                    }
                }
                Ok(())
            };
            if let Err(e) = writer.add_entry_streaming(name, path, &mut on_tick) {
                return Some(Err(e));
            }
            entries_done += 1;
            bytes_done += *size;
        }

        let elapsed = started.elapsed();
        let elapsed_ms = elapsed.as_millis() as u64;
        let mb_per_sec = if elapsed.as_secs_f64() > 0.0 {
            (bytes_done as f64) / (elapsed.as_secs_f64() * 1_048_576.0)
        } else {
            0.0
        };
        tracing::info!(
            target: "otterzip::compress",
            workers = worker_count,
            elapsed_ms,
            entries = entries_done,
            bytes_uncompressed = bytes_done,
            mb_per_sec = format!("{mb_per_sec:.1}"),
            "parallel compress dispatch done — throughput summary"
        );
        Some(Ok(()))
    }
}

/// Map the public [`CompressionMethod`] + level to the in-tree
/// writer's enum. We only ship two encoder paths (Stored, Deflate);
/// other methods through the regular ZIP backend collapse onto
/// Deflate so old fixtures keep working. The level is clamped to
/// `1..=9` for Deflate and ignored for Stored.
fn map_compression_method(
    method: CompressionMethod,
    level: u8,
) -> ZipCompression {
    match method {
        CompressionMethod::Store => ZipCompression::Stored,
        _ => ZipCompression::Deflate {
            // Level 0 historically meant "default" in our public API
            // even though it semantically clashes with method 0
            // (Stored). Map 0 → 5 (the Default impl in CreateOptions
            // and zip-rs's own default) so we don't accidentally
            // hand libdeflater an invalid level.
            level: if level == 0 { 5 } else { level.min(9) },
        },
    }
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
            .ok_or(OtterzipError::InvalidArgument("7z writer already committed"))?;
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
            writer.finish().map_err(OtterzipError::Io)?;
        }
        Ok(())
    }
}

fn map_sevenz_err(e: sevenz_rust::Error) -> OtterzipError {
    use sevenz_rust::Error as E;
    match e {
        E::Io(io, _) => OtterzipError::Io(io),
        other => OtterzipError::BackendError(other.to_string()),
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
            .ok_or(OtterzipError::InvalidArgument("tar writer already committed"))?;

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
                .map_err(OtterzipError::Io)?;
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
            .map_err(OtterzipError::Io)?;
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
            .map_err(OtterzipError::Io)?;
        Ok(())
    }

    fn finish(mut self) -> Result<Box<dyn std::io::Write + Send>> {
        let builder = self
            .builder
            .take()
            .ok_or(OtterzipError::InvalidArgument("tar writer already committed"))?;
        builder.into_inner().map_err(OtterzipError::Io)
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
/// skipped (Phase 6+ — the matching policy lives in
/// [`crate::options::is_system_metadata`]).
///
/// When a `progress` sink is supplied, this function:
///   * pre-walks `src` once to compute `entries_total` / `bytes_total`
///     (cheap metadata-only traversal) and reports a single
///     `ProgressPhase::Scanning` tick;
///   * reports a `ProgressPhase::Writing` tick after each file entry is
///     added, with cumulative `entries_processed` and `bytes_processed`;
///   * returns [`OtterzipError::Canceled`] as soon as
///     [`ProgressSink::update`] returns `false`.
pub(crate) fn add_dir_recursive_through(
    writer: &mut dyn ArchiveWriter,
    src: &Path,
    entry_prefix: &str,
    follow_symlinks: bool,
    exclude_system_metadata: bool,
    mut progress: Option<&mut dyn crate::progress::ProgressSink>,
) -> Result<()> {
    if !src.exists() {
        return Err(OtterzipError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            src.display().to_string(),
        )));
    }

    // Format-specific bulk path takes priority — ZIP overrides this
    // for chunked rayon-parallel deflate, every other backend falls
    // through. Pass `&mut progress` so the bulk method borrows it
    // only for the call's duration; if it declines (`None`) the
    // generic walker below still has the original sink to drive
    // its per-entry ticks.
    if let Some(result) = writer.add_directory_bulk(
        src,
        entry_prefix,
        follow_symlinks,
        exclude_system_metadata,
        &mut progress,
    ) {
        return result;
    }

    // Pre-scan to enable percentage-style progress. Cheap: metadata-only
    // traversal of the same tree we're about to compress. Skipped when
    // no sink is attached — saves the second filesystem walk for headless
    // / API callers that don't care about progress.
    let (entries_total, bytes_total) = if progress.is_some() {
        pre_scan(src, follow_symlinks, exclude_system_metadata)?
    } else {
        (0, 0)
    };

    let start = std::time::Instant::now();
    let mut state = WalkState {
        entries_processed: 0,
        bytes_processed: 0,
        entries_total,
        bytes_total,
        start,
    };

    // Resolve to a single concrete `&mut dyn ProgressSink` for the rest
    // of the pipeline — passing `Option<&mut dyn>` through a recursive
    // / iterative walker hits the borrow checker's invariance rules
    // every time. A no-op `NullSink` covers the "no progress" case.
    let mut null = NullSink;
    let sink: &mut dyn crate::progress::ProgressSink = match progress.as_deref_mut() {
        Some(s) => s,
        None => &mut null,
    };

    // Initial "Scanning" tick lets the UI move off 0% instantly so the
    // user doesn't think the click was lost.
    let snapshot = state.snapshot(crate::progress::ProgressPhase::Scanning, None);
    if !sink.update(&snapshot) {
        return Err(OtterzipError::Canceled);
    }

    let walk_result = walk(
        writer,
        src,
        entry_prefix,
        follow_symlinks,
        exclude_system_metadata,
        &mut state,
        sink,
    );

    // Throughput summary — diagnostic for the compress-speed-vs-Bandizip
    // investigation (the lenient ZIP sprint shipped read-side parallel
    // extract; write side is still single-threaded deflate). MB/s here
    // is uncompressed input throughput, the figure that directly maps
    // to "how saturated is the deflate worker"; a single-core zlib-ng
    // encoder typically tops out around 60–120 MB/s on modern x86 —
    // anything in that band confirms the CPU-bound serial deflate
    // diagnosis. Below ~40 MB/s usually means I/O or NTFS metadata
    // contention is overlapping the deflate cost.
    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    let mb_per_sec = if elapsed_ms > 0 {
        (state.bytes_processed as f64) / (elapsed.as_secs_f64() * 1_048_576.0)
    } else {
        0.0
    };
    tracing::info!(
        target: "otterzip::compress",
        elapsed_ms,
        entries = state.entries_processed,
        bytes_uncompressed = state.bytes_processed,
        mb_per_sec = format!("{mb_per_sec:.1}"),
        canceled = walk_result.is_err(),
        "compress walk done — throughput summary"
    );
    walk_result
}

/// No-op sink used when the caller didn't supply one. Lets the rest of
/// the writer pipeline assume a non-null sink and skip the everywhere
/// `Option`-shaped borrows.
struct NullSink;
impl crate::progress::ProgressSink for NullSink {
    fn update(&mut self, _: &crate::progress::Progress) -> bool {
        true
    }
}

struct WalkState {
    entries_processed: u32,
    bytes_processed: u64,
    entries_total: u32,
    bytes_total: u64,
    start: std::time::Instant,
}

impl WalkState {
    fn snapshot(
        &self,
        phase: crate::progress::ProgressPhase,
        current_entry: Option<&str>,
    ) -> crate::progress::Progress {
        crate::progress::Progress {
            bytes_processed: self.bytes_processed,
            bytes_total: self.bytes_total,
            entries_processed: self.entries_processed,
            entries_total: self.entries_total,
            current_entry_bytes_processed: 0,
            current_entry_bytes_total: 0,
            current_entry: current_entry.map(str::to_owned),
            phase,
            elapsed: self.start.elapsed(),
        }
    }
}

fn pre_scan(
    src: &Path,
    follow_symlinks: bool,
    exclude_system_metadata: bool,
) -> Result<(u32, u64)> {
    let mut entries: u32 = 0;
    let mut bytes: u64 = 0;
    pre_scan_visit(src, src, follow_symlinks, exclude_system_metadata, &mut entries, &mut bytes)?;
    Ok((entries, bytes))
}

fn pre_scan_visit(
    root: &Path,
    current: &Path,
    follow_symlinks: bool,
    exclude_system_metadata: bool,
    entries: &mut u32,
    bytes: &mut u64,
) -> Result<()> {
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
        for child in std::fs::read_dir(current)? {
            let child = child?;
            pre_scan_visit(
                root,
                &child.path(),
                follow_symlinks,
                exclude_system_metadata,
                entries,
                bytes,
            )?;
        }
        return Ok(());
    }
    if meta.file_type().is_symlink() && !follow_symlinks {
        return Ok(());
    }
    *entries = entries.saturating_add(1);
    *bytes = bytes.saturating_add(meta.len());
    Ok(())
}

/// Iterative DFS over the source tree. Rewritten from recursive form so
/// the borrow checker only sees one reborrow of `progress` per loop
/// iteration — recursive calls were getting tangled because
/// `Option<&mut dyn Trait>` is invariant in the trait object's lifetime.
fn walk(
    writer: &mut dyn ArchiveWriter,
    root: &Path,
    entry_prefix: &str,
    follow_symlinks: bool,
    exclude_system_metadata: bool,
    state: &mut WalkState,
    progress: &mut dyn crate::progress::ProgressSink,
) -> Result<()> {
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        if exclude_system_metadata
            && current != root
            && crate::options::is_system_metadata(&current)
        {
            continue;
        }

        let meta = if follow_symlinks {
            std::fs::metadata(&current)?
        } else {
            std::fs::symlink_metadata(&current)?
        };

        if meta.is_dir() {
            if current != root {
                let entry_name = compose_entry_name(root, &current, entry_prefix);
                writer.add_entry(
                    &format!("{entry_name}/"),
                    &mut std::io::empty(),
                    Some(0),
                    true,
                )?;
            }
            // Push children in reverse so pop order matches alphabetical
            // (same as the previous recursive read_dir order).
            let mut children: Vec<PathBuf> = Vec::new();
            for child in std::fs::read_dir(&current)? {
                children.push(child?.path());
            }
            for child in children.into_iter().rev() {
                stack.push(child);
            }
            continue;
        }

        if meta.file_type().is_symlink() && !follow_symlinks {
            continue;
        }

        let entry_name = compose_entry_name(root, &current, entry_prefix);
        let snapshot_template = state.snapshot(
            crate::progress::ProgressPhase::Writing,
            Some(&entry_name),
        );
        process_file_entry(
            writer,
            &current,
            &entry_name,
            meta.len(),
            state,
            snapshot_template,
            &mut *progress,
        )?;
    }
    Ok(())
}

fn process_file_entry(
    writer: &mut dyn ArchiveWriter,
    file_path: &Path,
    entry_name: &str,
    file_size: u64,
    state: &mut WalkState,
    snapshot_template: crate::progress::Progress,
    progress: &mut dyn crate::progress::ProgressSink,
) -> Result<()> {
    let write_result;
    {
        let mut counting = ProgressReader {
            inner: BufReader::new(File::open(file_path)?),
            bytes_in_entry: 0,
            last_report: 0,
            snapshot: snapshot_template,
            sink: progress,
        };
        write_result = writer.add_entry(entry_name, &mut counting, Some(file_size), false);
    }

    match write_result {
        Err(OtterzipError::Io(io_err)) if is_cancel_marker(&io_err) => {
            return Err(OtterzipError::Canceled);
        }
        Err(e) => return Err(e),
        Ok(()) => {}
    }

    state.entries_processed = state.entries_processed.saturating_add(1);
    state.bytes_processed = state.bytes_processed.saturating_add(file_size);
    Ok(())
}

/// IO-layer wrapper that ticks the supplied `ProgressSink` while bytes
/// stream into the archive writer. Without this, the sink only fires
/// once per entry — a single multi-GB file would otherwise pin the
/// progress bar between two file boundaries.
struct ProgressReader<'a, R: std::io::Read> {
    inner: R,
    bytes_in_entry: u64,
    last_report: u64,
    /// Cumulative-state template captured at entry start. The reader
    /// only mutates `bytes_processed` on top of this; everything else
    /// (totals, entries_processed, elapsed origin) stays as the caller
    /// recorded it.
    snapshot: crate::progress::Progress,
    sink: &'a mut dyn crate::progress::ProgressSink,
}

impl<R: std::io::Read> std::io::Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_in_entry = self.bytes_in_entry.saturating_add(n as u64);

        // Throttle to ~1 MiB ticks so the sink doesn't get hammered on
        // small reads from BufReader. Always tick on EOF (n == 0) so
        // the final state is reported even when total entry size is
        // below the throttle threshold.
        const TICK_BYTES: u64 = 1_048_576;
        let should_tick = n == 0
            || self.bytes_in_entry.saturating_sub(self.last_report) >= TICK_BYTES;
        if should_tick {
            self.last_report = self.bytes_in_entry;
            let mut snap = self.snapshot.clone();
            snap.bytes_processed = snap.bytes_processed.saturating_add(self.bytes_in_entry);
            if !self.sink.update(&snap) {
                return Err(std::io::Error::other(CANCEL_MARKER));
            }
        }
        Ok(n)
    }
}

/// Sentinel string we attach to the io::Error so the outer walk can
/// recognise "the user asked to cancel" vs "the disk is broken".
const CANCEL_MARKER: &str = "otterzip-canceled";

fn is_cancel_marker(io_err: &std::io::Error) -> bool {
    io_err.to_string().contains(CANCEL_MARKER)
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
            return Err(OtterzipError::FeatureDisabled(
                "XZ single-stream cannot encode directory entries",
            ));
        }
        if self.written {
            return Err(OtterzipError::FeatureDisabled(
                "XZ single-stream allows exactly one entry",
            ));
        }
        let _ = entry_path; // single-stream has no in-archive name; ignore.
        let encoder = self
            .encoder
            .as_mut()
            .ok_or(OtterzipError::InvalidArgument("xz writer already committed"))?;
        std::io::copy(data, encoder)?;
        self.written = true;
        Ok(())
    }

    fn commit(mut self: Box<Self>) -> Result<()> {
        if let Some(enc) = self.encoder.take() {
            // `finish` flushes the trailing XZ stream footer + index;
            // dropping without it produces a truncated archive that
            // most extractors refuse.
            let inner = enc.finish().map_err(OtterzipError::Io)?;
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
                return Err(OtterzipError::FeatureDisabled(
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
            .ok_or(OtterzipError::InvalidArgument("zipx writer already committed"))?;
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

/// Maps zip-rs errors onto our taxonomy. Kept narrowly scoped to the
/// `ZipxWriterBackend` (still leans on zip-rs because the BZip2 /
/// LZMA method extensions live there); the regular ZIP writer goes
/// through the in-tree `ZipFileWriter` and never sees this.
fn map_zip_err(e: zip::result::ZipError) -> OtterzipError {
    OtterzipError::BackendError(e.to_string())
}
