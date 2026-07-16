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
        ArchiveFormat::Zip => {
            // Encrypted ZIP routes through the upstream zip crate's writer
            // (AES-256); plain ZIP keeps the fast in-tree pipeline.
            if password.is_some() && opts.encryption != crate::format::EncryptionMethod::None {
                Ok(Box::new(EncryptedZipWriterBackend::create(path, opts, password)?))
            } else {
                Ok(Box::new(ZipWriterBackend::create(path, opts, password)?))
            }
        }
        ArchiveFormat::SevenZ => Ok(Box::new(SevenZWriterBackend::create(path, opts, password)?)),
        ArchiveFormat::TarGz => Ok(Box::new(TarGzWriterBackend::create(path, opts)?)),
        ArchiveFormat::Tar => Ok(Box::new(TarPlainWriterBackend::create(path, opts)?)),
        // PERMANENT, and a LEGAL constraint rather than a scope decision: the
        // read path (`backends/rar.rs`) links RARLAB's UnRAR sources, whose
        // licence forbids using them to develop a RAR-compatible archiver. Do
        // not "finish" this arm — there is no sprint in which it lands.
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

// `PARALLEL_CHUNK_SIZE` (previously 64) was the chunk size for the
// `par_iter().collect()` chunked-rayon path. The single-ordered-
// pipeline overhaul replaced that path with an mpsc-channel
// streaming pipeline, so the chunk size is no longer a tunable
// parameter — the channel depth (`CHANNEL_DEPTH`, defined locally
// in `add_directory_bulk`) plays the analogous role of bounding
// in-flight memory.

/// Minimum file count for the parallel pipeline to be worth spinning
/// up. Below this we fall back to the serial walker — the rayon
/// thread-pool build cost outweighs 4-worker speed-up on a handful
/// of entries.
const PARALLEL_MIN_ENTRIES: usize = 16;

/// Worker-count cap. The read-side `LenientZipBackend` extract path
/// caps at 4 because NTFS *file-create* serialisation under the
/// destination directory's MFT lock dominated once we went past
/// that on extracts — every output file is a fresh `File::create`
/// against the same parent dir.
///
/// The write side is a *different* contention profile, though:
///
///   * **Only one output file is being written** (the archive). Per-
///     worker file-create serialisation doesn't apply — workers
///     never touch the destination volume directly. They `File::open`
///     against the *source* tree (sibling reads, MFT-cache hot) and
///     hand deflated bytes back to the main thread which serialises
///     the single archive write.
///   * **Disk usage measured at 5 %** on the user's 16-core / 32 GB
///     reproducer with 4 workers active — kernel I/O queue has
///     massive headroom. We're CPU-bound on deflate, not I/O-bound.
///
/// 8 strikes the practical balance: doubles the compress throughput
/// vs the 4-cap, exploits more of the 16-core Ultra 7 the user
/// runs without overshooting the dev / mid-tier machines we still
/// need to run cleanly on (8-core Ryzen / i7 with hyperthreads).
/// Above 8 the rayon dispatch overhead per chunk starts costing
/// more than the per-worker gain on chunks of 64 entries.
const PARALLEL_WORKER_CAP: usize = 8;

/// Per-entry uncompressed size above which the bulk dispatcher
/// stops sending the file through the rayon worker pool and instead
/// drives it through the main-thread serial streaming path.
///
/// History: 64 MiB was the original cap, picked when the encode
/// pipeline still went through `par_iter().collect()` chunk
/// barriers. The `add_directory_bulk` overhaul replaced that with
/// an ordered mpsc pipeline that bounds in-flight memory through
/// `CHANNEL_DEPTH` instead of chunk size, so the per-worker buffer
/// concern at 64 MiB no longer applies — workers move on to the
/// next entry the moment they send. The user's reproducer log
/// (commit `de2b99b` measurement) showed Phase 2 (large entries)
/// consuming 253 s out of 274 s wall-clock because 14 mid-sized
/// `.dll` / `.exe` files (each 64–256 MiB) were taking the
/// main-thread serial path even though the worker pool would have
/// handled them in parallel without breaking a sweat.
///
/// 256 MiB lifts those mid-sized files into the small / pipeline
/// path. Only the genuinely-large outliers (the 3.58 GB Setup.exe,
/// the 542 MB Setup.exe, etc.) stay on the streaming path where
/// the worker-per-file model would otherwise spike memory.
///
/// Memory ceiling at 256 MiB: bounded channel (16 slots) × 256 MiB
/// = 4 GiB worst case, comfortably inside the user's 31 GiB
/// machine and well below the previous chunked-rayon path's
/// uncapped peak.
const LARGE_ENTRY_THRESHOLD_BYTES: u64 = 256 * 1024 * 1024;

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

        // ── Small entries — ordered streaming pipeline ───────────
        //
        // Replaces the previous chunked `par_iter().collect()` sync
        // barrier with an mpsc-channel pipeline. The barrier was
        // the single biggest serialisation point — every 64-entry
        // chunk forced the main thread to wait until every worker
        // in the chunk finished before any splice started, and
        // then forced every worker to wait while the main thread
        // spliced the chunk's worth of results onto the writer.
        // CPU graph showed the symptom directly: 9–28 % usage
        // (4–5 cores) instead of the 70 %+ Bandizip pegs at on the
        // same hardware.
        //
        // The pipeline pattern:
        //
        //   * Worker dispatcher (one std::thread, spawned via
        //     thread::scope) runs `par_iter` on the rayon pool
        //     and `for_each_with`-sends each deflated result to
        //     the bounded sync_channel. The channel's bound caps
        //     in-flight memory at ~CHANNEL_DEPTH × max-small-
        //     entry-bytes (16 × 64 MiB worst-case ≈ 1 GiB, in
        //     practice <100 MiB on the user's corpus).
        //   * Main thread receives `(idx, Result<PreparedEntry>)`
        //     pairs out-of-order, buffers them in a BTreeMap, and
        //     drains contiguous prefixes onto the writer so LFH
        //     byte offsets stay strictly monotonic (the CDFH
        //     `local_header_offset` field has to match).
        //   * Workers never idle waiting for the splice; the main
        //     thread never idles waiting for the next batch.
        //
        // Secondary progress bar (ABI v9
        // `current_entry_bytes_*`) reports per-entry byte progress
        // within the small-entries phase via a sliding window —
        // we don't have per-chunk semantics any more, so the bar
        // simply tracks `phase1_bytes_done / phase1_bytes_total`
        // and ticks every splice. Resets when the phase finishes
        // and the large-streaming phase takes over.
        if !small_files.is_empty() {
            use std::collections::BTreeMap;
            use std::sync::atomic::{AtomicU64, Ordering};
            use std::sync::mpsc::{self, RecvTimeoutError};
            use std::sync::Arc;
            use std::time::Duration;

            const CHANNEL_DEPTH: usize = 16;
            // recv_timeout cadence — 33 ms matches one compositor
            // frame at 30 Hz, the same wall-clock the host-side
            // `JobQueue.BuildProgressReporter` throttle uses. Going
            // tighter wastes CPU on atomic loads with no visible
            // benefit; going wider lets a 200+ MiB worker finish
            // an entry before any in-flight tick fires, which is
            // exactly the bug this refactor exists to fix.
            const INFLIGHT_TICK_INTERVAL: Duration = Duration::from_millis(33);
            let (tx, rx) = mpsc::sync_channel::<(usize, Result<PreparedEntry>)>(CHANNEL_DEPTH);

            let small_total: u64 = small_files
                .iter()
                .filter_map(|(p, _)| std::fs::metadata(p).ok().map(|m| m.len()))
                .sum();
            let mut phase1_bytes_done: u64 = 0;
            let mut pipeline_error: Option<OtterzipError> = None;

            // Cumulative count of uncompressed bytes the worker pool
            // has read from disk so far. Each `prepare_entry` ticks
            // this atomic in 1 MiB increments. The main thread polls
            // it on `recv_timeout` wake-ups to surface mid-entry
            // progress without ever touching the archive write path
            // — so byte-identical archive output is structurally
            // guaranteed (the only `add_entry_prepared` calls still
            // sit inside the ordered drain below).
            let bytes_in_flight = Arc::new(AtomicU64::new(0));
            let bytes_in_flight_worker = Arc::clone(&bytes_in_flight);

            std::thread::scope(|s| {
                // Worker dispatcher — runs the rayon par_iter on
                // the prebuilt pool and streams deflated results
                // into the channel as they finish. The pool's
                // install() ensures every par_iter task lands on
                // *our* 8-worker pool instead of the global rayon
                // default (which would spawn 16 threads on the
                // user's Ultra 7 and double-saturate the deflate
                // CPU budget).
                let small_files_ref = &small_files;
                let pool_ref = &pool;
                let bytes_in_flight_ref = bytes_in_flight_worker;
                s.spawn(move || {
                    pool_ref.install(|| {
                        small_files_ref
                            .par_iter()
                            .enumerate()
                            .for_each_with(tx, |tx, (idx, (path, name))| {
                                let result = zip_writer::prepare_entry(
                                    path, name, compression, mtime, mdate,
                                    &bytes_in_flight_ref,
                                );
                                let _ = tx.send((idx, result));
                            });
                    });
                });

                // Main thread: ordered receive + splice. Pending
                // BTreeMap holds out-of-order results until the
                // next contiguous index arrives. We switched from
                // `for (idx, result) in rx.iter()` to a
                // `recv_timeout` loop so the timeout branch can
                // poll `bytes_in_flight` and emit a fine-grained
                // progress tick even when no entry has completed.
                //
                // Invariants:
                //   * `spliced_bytes` ≡ `bytes_done` for this phase
                //     (kept separate from the archive-wide
                //     `bytes_done` in case future phases interleave).
                //   * `bytes_in_flight - spliced_bytes` is the worker
                //     pool's in-memory backlog — bytes that have
                //     been read from disk but not yet committed to
                //     the archive. Always ≥ 0; saturating_sub
                //     defends against the freak case where the
                //     atomic load races slightly behind splice.
                //   * Visible top-bar = `bytes_done + in_flight`
                //     clamped by `bytes_total` — an in-flight
                //     forecast that converges exactly on entry
                //     completion.
                let mut pending: BTreeMap<usize, PreparedEntry> = BTreeMap::new();
                let mut next_idx: usize = 0;
                let mut spliced_bytes: u64 = 0;
                let mut last_inflight_emit: u64 = 0;
                // Monotonic guards. The in-flight tick uses
                // `bytes_done + in_flight` as a *forecast*, but
                // `in_flight` is the sum of every worker's read
                // progress — with 8 workers each reading 50 MiB
                // entries, the forecast can outrun the actual
                // `bytes_done` of any single entry that completes
                // afterwards. Without these guards the emit
                // sequence would visibly regress, breaking the
                // host's "progress only goes up" contract.
                //
                // We never lower the emitted value: any
                // entry-complete or in-flight tick is `max`'d
                // against the last emit. Forecast over-estimates
                // simply hold the bar slightly ahead of reality
                // until enough entries complete to catch up —
                // which is the visually smooth outcome the user
                // wants.
                let mut last_emitted_top: u64 = 0;
                let mut last_emitted_entry: u64 = 0;
                loop {
                    let recv = rx.recv_timeout(INFLIGHT_TICK_INTERVAL);
                    if pipeline_error.is_some() {
                        // Drain remaining messages so workers don't
                        // block on a full channel; we'll surface the
                        // first error captured below.
                        match recv {
                            Err(RecvTimeoutError::Disconnected) => break,
                            _ => continue,
                        }
                    }
                    match recv {
                        Ok((idx, Ok(entry))) => {
                            pending.insert(idx, entry);
                            while let Some(entry) = pending.remove(&next_idx) {
                                let entry_bytes = entry.uncompressed_size;
                                let entry_name =
                                    String::from_utf8_lossy(&entry.name).into_owned();
                                if let Err(err) = writer.add_entry_prepared(entry) {
                                    pipeline_error = Some(err);
                                    break;
                                }
                                entries_done += 1;
                                bytes_done += entry_bytes;
                                spliced_bytes = spliced_bytes.saturating_add(entry_bytes);
                                phase1_bytes_done = phase1_bytes_done.saturating_add(entry_bytes);
                                next_idx += 1;
                                // Monotonic guard — clamp the
                                // visible values up to the actual
                                // committed bytes, but never below
                                // the prior in-flight forecast.
                                last_emitted_top = last_emitted_top.max(bytes_done);
                                last_emitted_entry =
                                    last_emitted_entry.max(phase1_bytes_done);
                                if let Some(sink) = progress.as_deref_mut() {
                                    let snapshot = crate::progress::Progress {
                                        bytes_processed: last_emitted_top,
                                        bytes_total,
                                        entries_processed: entries_done,
                                        entries_total: total_entries as u32,
                                        current_entry: Some(entry_name),
                                        phase: crate::progress::ProgressPhase::Writing,
                                        elapsed: started.elapsed(),
                                        current_entry_bytes_processed: last_emitted_entry,
                                        current_entry_bytes_total: small_total,
                                    };
                                    if !sink.update(&snapshot) {
                                        pipeline_error = Some(OtterzipError::Canceled);
                                        break;
                                    }
                                }
                            }
                        }
                        Ok((_, Err(err))) => {
                            pipeline_error = Some(err);
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            // In-flight tick — surfaces the worker
                            // pool's mid-entry read progress before
                            // any entry completes. We only tick
                            // when the atomic actually moved
                            // (otherwise idle waits would spam the
                            // sink with duplicates) and clamp the
                            // visible values to the per-phase /
                            // archive-wide totals so a transient
                            // over-estimate can't make the bar
                            // overshoot 100 %.
                            let read = bytes_in_flight.load(Ordering::Relaxed);
                            if read == last_inflight_emit {
                                continue;
                            }
                            last_inflight_emit = read;
                            let in_flight = read.saturating_sub(spliced_bytes);
                            let forecast_top =
                                (bytes_done.saturating_add(in_flight)).min(bytes_total);
                            let forecast_entry = phase1_bytes_done
                                .saturating_add(in_flight)
                                .min(small_total);
                            // Monotonic guard. `read` is the sum of
                            // every worker's in-progress disk read,
                            // so the forecast can momentarily run
                            // ahead of any single entry that completes
                            // afterward. We `max` against the last
                            // emit so the visible value never moves
                            // backwards.
                            last_emitted_top = last_emitted_top.max(forecast_top);
                            last_emitted_entry =
                                last_emitted_entry.max(forecast_entry);
                            #[cfg(debug_assertions)]
                            tracing::trace!(
                                target: "otterzip::progress::flight",
                                read,
                                spliced = spliced_bytes,
                                visible_top = last_emitted_top,
                                visible_entry = last_emitted_entry,
                                "in-flight tick"
                            );
                            if let Some(sink) = progress.as_deref_mut() {
                                let snapshot = crate::progress::Progress {
                                    bytes_processed: last_emitted_top,
                                    bytes_total,
                                    entries_processed: entries_done,
                                    entries_total: total_entries as u32,
                                    current_entry: None,
                                    phase: crate::progress::ProgressPhase::Writing,
                                    elapsed: started.elapsed(),
                                    current_entry_bytes_processed: last_emitted_entry,
                                    current_entry_bytes_total: small_total,
                                };
                                if !sink.update(&snapshot) {
                                    pipeline_error = Some(OtterzipError::Canceled);
                                }
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            });

            if let Some(err) = pipeline_error {
                return Some(Err(err));
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
            // Pigz path gated behind `OTTERZIP_PIGZ=1` for the
            // current release. Background: the 2026-05-13 sprint
            // re-enabled it on top of `pigz_parallel_*` /
            // `pigz_deflate_block_concat_*` unit tests, the user
            // immediately reproduced the 4-entry corruption set
            // (COMMON/GRAMCHAT/Setup.exe, COMMON/MEI/MUP3.zip,
            // COMMON/MEI/SetupME.exe, COMMON/MYGRAM/Setup.exe), and
            // the path was reverted in `7953f49`. 2026-05-15
            // `diagnose_pigz` on a 149 MiB VSCode installer
            // pinpointed the root cause as a Sync-flush-completion
            // sentinel bug in `deflate_block_raw` (now fixed) plus a
            // missing smart-store gate (now added). Env flag bounds
            // the blast radius for one release cycle — if a fourth
            // failure mode surfaces we flip the var off without a
            // revert commit. Remove the flag once a clean release
            // ships against the full user corpus.
            let use_pigz = std::env::var("OTTERZIP_PIGZ")
                .ok()
                .as_deref()
                == Some("1");
            let large_result = if use_pigz {
                writer.add_entry_pigz_parallel(name, path, &pool, &mut on_tick)
            } else {
                writer.add_entry_streaming(name, path, &mut on_tick)
            };
            if let Err(e) = large_result {
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

/// Peak in-memory bound for a buffered solid block. The sevenz crate's own
/// solid path uses a 4 GiB block but reads files lazily by path; our backend
/// only receives `Read` streams, so a block is buffered in RAM — cap it
/// small. Entries at/over this size stream individually (no buffering).
const SOLID_BLOCK_CAP: u64 = 64 * 1024 * 1024; // 64 MiB

pub(crate) struct SevenZWriterBackend {
    inner: Option<sevenz_rust2::ArchiveWriter<File>>,
    /// When true, small entries are packed into shared solid blocks via
    /// `push_archive_entries` (better ratio for many small files); large
    /// entries still stream individually.
    solid: bool,
    pending_entries: Vec<sevenz_rust2::ArchiveEntry>,
    pending_data: Vec<std::io::Cursor<Vec<u8>>>,
    pending_bytes: u64,
}

impl SevenZWriterBackend {
    fn create(
        path: &Path,
        opts: &CreateOptions,
        password: Option<&Zeroizing<String>>,
    ) -> Result<Self> {
        let mut writer =
            sevenz_rust2::ArchiveWriter::create(path).map_err(map_sevenz_err)?;

        // === Multi-threaded LZMA2 — tuned for ship-quality throughput ========
        //
        // sevenz-rust2 0.21 exposes parallel LZMA2 via
        // `Lzma2Options::from_level_mt(level, threads, chunk_size)`. Each
        // thread encodes one independent chunk with a fresh dictionary; the
        // writer concatenates the chunks into a single valid LZMA2 stream.
        // This is the 7z analogue of the ZIP pigz path — same per-block
        // parallel idea, native to the LZMA2 container.
        //
        // Tuning rationale (2026-05-15 sprint follow-up, real-corpus
        // measurement: 17.6 → ~40 MiB/s target):
        //
        //   * **level=1 (was 3)**. LZMA2 preset 1 = HC4 fast matcher
        //     with 1 MiB dict. Level 3 (HC4 fast / 4 MiB dict) cost ~2×
        //     wall-clock for only ~1.5% better ratio on real installer
        //     mixes. Level 4+ enters BT4 normal mode — a memory-bandwidth
        //     cliff that drops throughput to ~25 MiB/s on 8T regardless
        //     of CPU. The 35+ MiB/s ship bar lives in the HC4 fast
        //     family; level 1 is the sweet spot. Trade: archive grows
        //     ~1.5% (≈ 140 MB on a 9.5 GB corpus) vs Bandizip's
        //     comparable preset.
        //
        //   * **threads = num_physical, capped at 8**. Empirical
        //     measurement on the user's 16-core / 16-logical machine:
        //     16 threads was 22 % SLOWER than 8 threads on the 9.5 GB
        //     corpus (5m38s @ 27 MiB/s vs 4m37s @ 33 MiB/s). This
        //     reproduces the "memory-bandwidth bound past 8 threads"
        //     finding from published 7-Zip scaling community thread
        //     and the known `lzma-rust2` MT-writer immaturity (issue
        //     #83 — unbounded buffering forces flushes that kill
        //     pipeline parallelism past 8 workers). The 8-cap also
        //     matches the historical 7-Zip MT cap before its 25.00
        //     "Threadripper Edition" release.
        //
        //   * **chunk_size = 1 MiB (was 4 MiB)**. yeah.nah.nz's xz-T
        //     benchmark showed 4 MiB → 1 MiB chunks bought a 2.4×
        //     throughput jump on a 32-thread box for ~kB of ratio
        //     drift. Smaller chunks keep all workers fed when input
        //     entropy varies (the user corpus has both PE binaries
        //     and already-LZMA-packed payloads).
        let level = 1u32;
        let threads = (num_cpus::get_physical() as u32).clamp(1, 8);
        let chunk_size: u64 = 1024 * 1024; // 1 MiB

        let lzma2 = sevenz_rust2::encoder_options::Lzma2Options::from_level_mt(
            level, threads, chunk_size,
        );

        // AES-256 when a password is supplied. The 7z method chain is
        // ordered [compress, encrypt] so data flows LZMA2 → AES → stored.
        // `set_encrypt_header(true)` also encrypts the file-name list
        // (matches 7-Zip's "Encrypt file names" default). 7z only supports
        // AES-256; any non-None encryption with a password maps to it
        // (ZipCrypto is a ZIP-only concept). No password → no encryption.
        let encrypt = password.is_some()
            && opts.encryption != crate::format::EncryptionMethod::None;
        if encrypt {
            let pw = password.expect("encrypt implies password present");
            let aes = sevenz_rust2::encoder_options::AesEncoderOptions::new(
                sevenz_rust2::Password::from(pw.as_str()),
            );
            writer.set_content_methods(vec![lzma2.into(), aes.into()]);
            writer.set_encrypt_header(true);
        } else {
            writer.set_content_methods(vec![lzma2.into()]);
        }

        tracing::info!(
            target: "otterzip::compress",
            level,
            threads,
            chunk_size,
            encrypt,
            user_level = opts.compression_level,
            "sevenz writer configured with multi-thread LZMA2 (tuned for throughput)"
        );

        Ok(Self {
            inner: Some(writer),
            solid: opts.solid,
            pending_entries: Vec::new(),
            pending_data: Vec::new(),
            pending_bytes: 0,
        })
    }

    /// Flush the buffered solid block (if any) as one packed stream via
    /// `push_archive_entries`. No-op when the block is empty. Peak memory
    /// stays ~one block (`SOLID_BLOCK_CAP`) regardless of archive size.
    fn flush_solid_block(&mut self) -> Result<()> {
        if self.pending_entries.is_empty() {
            return Ok(());
        }
        let writer = self
            .inner
            .as_mut()
            .ok_or(OtterzipError::InvalidArgument("7z writer already committed"))?;
        let entries = std::mem::take(&mut self.pending_entries);
        let readers: Vec<sevenz_rust2::SourceReader<std::io::Cursor<Vec<u8>>>> =
            std::mem::take(&mut self.pending_data)
                .into_iter()
                .map(sevenz_rust2::SourceReader::new)
                .collect();
        writer
            .push_archive_entries(entries, readers)
            .map_err(map_sevenz_err)?;
        self.pending_bytes = 0;
        Ok(())
    }
}

impl ArchiveWriter for SevenZWriterBackend {
    fn add_entry(
        &mut self,
        entry_path: &str,
        data: &mut dyn Read,
        size_hint: Option<u64>,
        is_directory: bool,
    ) -> Result<()> {
        // Directory entries are metadata-only. In solid mode flush the open
        // block first so the on-disk entry order is preserved.
        if is_directory {
            if self.solid {
                self.flush_solid_block()?;
            }
            let writer = self
                .inner
                .as_mut()
                .ok_or(OtterzipError::InvalidArgument("7z writer already committed"))?;
            let mut entry = sevenz_rust2::ArchiveEntry::default();
            entry.name = entry_path.to_string();
            entry.is_directory = true;
            writer
                .push_archive_entry::<&[u8]>(entry, None)
                .map_err(map_sevenz_err)?;
            return Ok(());
        }

        // Solid: pack small entries into shared blocks; stream large ones.
        if self.solid {
            let size = size_hint.unwrap_or(0);
            if size >= SOLID_BLOCK_CAP {
                self.flush_solid_block()?;
                let writer = self
                    .inner
                    .as_mut()
                    .ok_or(OtterzipError::InvalidArgument("7z writer already committed"))?;
                let mut entry = sevenz_rust2::ArchiveEntry::default();
                entry.name = entry_path.to_string();
                entry.size = size;
                writer
                    .push_archive_entry(entry, Some(&mut *data))
                    .map_err(map_sevenz_err)?;
                return Ok(());
            }
            let mut buf = Vec::with_capacity(size as usize);
            data.read_to_end(&mut buf)?;
            let actual = buf.len() as u64;
            let mut entry = sevenz_rust2::ArchiveEntry::default();
            entry.name = entry_path.to_string();
            entry.size = actual;
            // push_archive_entries trusts the entry's has_stream (unlike
            // push_archive_entry, which sets it from the Some(reader)).
            entry.has_stream = true;
            self.pending_entries.push(entry);
            self.pending_data.push(std::io::Cursor::new(buf));
            self.pending_bytes += actual;
            if self.pending_bytes >= SOLID_BLOCK_CAP {
                self.flush_solid_block()?;
            }
            return Ok(());
        }

        // Non-solid: stream each entry immediately (no `Vec<u8>` slurp).
        // `&mut *data` re-borrows the `&mut dyn Read` we received, yielding
        // a `Read` that streams without materialising the whole file.
        let writer = self
            .inner
            .as_mut()
            .ok_or(OtterzipError::InvalidArgument("7z writer already committed"))?;
        let mut entry = sevenz_rust2::ArchiveEntry::default();
        entry.name = entry_path.to_string();
        if let Some(size) = size_hint {
            entry.size = size;
        }
        writer
            .push_archive_entry(entry, Some(&mut *data))
            .map_err(map_sevenz_err)?;
        Ok(())
    }

    fn commit(mut self: Box<Self>) -> Result<()> {
        // Flush any trailing solid block before finalizing.
        self.flush_solid_block()?;
        if let Some(writer) = self.inner.take() {
            // sevenz-rust2 0.13+ `finish()` consumes self and returns the
            // underlying writer (`io::Result<W>`); discard — Drop closes it.
            let _file = writer.finish().map_err(OtterzipError::Io)?;
        }
        Ok(())
    }
}

fn map_sevenz_err(e: sevenz_rust2::Error) -> OtterzipError {
    use sevenz_rust2::Error as E;
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
/// Finalizer for a tar output sink. `commit` calls [`TarSink::finalize`] so
/// the final flush / gzip-trailer write RETURNS its I/O error instead of
/// swallowing it in `Drop` — a disk-full on the very last write must fail the
/// commit loudly, not silently truncate the archive (pre-1.0.1 review BK-M2).
trait TarSink: std::io::Write + Send {
    fn finalize(self: Box<Self>) -> std::io::Result<()>;
}

impl TarSink for BufWriter<File> {
    fn finalize(mut self: Box<Self>) -> std::io::Result<()> {
        // Drains the buffered final block to the file, surfacing any error.
        // (`Write` isn't `use`d in this module — fully qualify the method.)
        std::io::Write::flush(&mut *self)
    }
}

impl TarSink for flate2::write::GzEncoder<BufWriter<File>> {
    fn finalize(self: Box<Self>) -> std::io::Result<()> {
        // finish() writes the gzip CRC/ISIZE trailer and returns the inner
        // BufWriter; flush() then drains it. Both can fail on a full disk —
        // Drop would have discarded exactly these errors.
        let mut inner = (*self).finish()?;
        std::io::Write::flush(&mut inner)
    }
}

struct TarBuilderHolder {
    builder: Option<tar::Builder<Box<dyn TarSink>>>,
}

impl TarBuilderHolder {
    fn new(inner: Box<dyn TarSink>) -> Self {
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

    fn finish(mut self) -> Result<Box<dyn TarSink>> {
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
        let inner: Box<dyn TarSink> = Box::new(BufWriter::new(file));
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
        // finalize() flushes the final block and RETURNS any I/O error
        // (Drop would have swallowed it, reporting a truncated archive as OK).
        self.holder.finish()?.finalize().map_err(OtterzipError::Io)?;
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
        let inner: Box<dyn TarSink> = Box::new(gz);
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
        // finalize() calls GzEncoder::finish() (writes the gzip CRC/ISIZE
        // trailer) and flushes the underlying BufWriter, RETURNING any I/O
        // error. The old `drop(inner)` relied on GzEncoder::Drop, which
        // discards trailer-write errors — a disk-full on the final block
        // produced a corrupt .tar.gz reported as a successful commit (BK-M2).
        self.holder.finish()?.finalize().map_err(OtterzipError::Io)?;
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

// ---------------------------------------------------------------------------
// Encrypted ZIP writer (AES-256)
// ---------------------------------------------------------------------------

/// AES-256 encrypted ZIP writer. `open_writer` selects this only when a
/// password is supplied for `ArchiveFormat::Zip` — the fast in-tree
/// `ZipWriterBackend` has no encryption path, so encrypted writes route
/// through the upstream `zip` crate's `ZipWriter` (same approach as
/// `ZipxWriterBackend`). The encrypted path forgoes the in-tree
/// rayon/libdeflater pipeline; an acceptable trade since encryption is
/// opt-in and interop (readable in 7-Zip / WinRAR / Bandizip) dominates.
/// AES-256 only — legacy ZipCrypto is refused.
pub(crate) struct EncryptedZipWriterBackend {
    inner: Option<zip::ZipWriter<BufWriter<File>>>,
    method: zip::CompressionMethod,
    level: Option<i64>,
    /// Owned password copy — `with_aes_encryption` borrows it, so it must
    /// outlive every `start_file`. Zeroized on drop.
    password: Zeroizing<String>,
}

impl EncryptedZipWriterBackend {
    fn create(
        path: &Path,
        opts: &CreateOptions,
        password: Option<&Zeroizing<String>>,
    ) -> Result<Self> {
        if opts.encryption == crate::format::EncryptionMethod::ZipCrypto {
            return Err(OtterzipError::FeatureDisabled(
                "ZipCrypto write is disabled; OtterZip encrypts ZIP with AES-256",
            ));
        }
        let pw = password.ok_or(OtterzipError::InvalidArgument(
            "encrypted ZIP create requires a password",
        ))?;
        let method = match opts.compression {
            CompressionMethod::Store => zip::CompressionMethod::Stored,
            // AES is an encryption layer over the method; Deflate is the
            // universally-readable ZIP baseline.
            _ => zip::CompressionMethod::Deflated,
        };
        let level = match method {
            zip::CompressionMethod::Stored => None,
            _ => Some(if opts.compression_level == 0 {
                6
            } else {
                i64::from(opts.compression_level.clamp(1, 9))
            }),
        };
        let file = File::create(path)?;
        let writer = zip::ZipWriter::new(BufWriter::new(file));
        Ok(Self {
            inner: Some(writer),
            method,
            level,
            password: pw.clone(),
        })
    }
}

impl ArchiveWriter for EncryptedZipWriterBackend {
    fn add_entry(
        &mut self,
        entry_path: &str,
        data: &mut dyn Read,
        _size_hint: Option<u64>,
        is_directory: bool,
    ) -> Result<()> {
        if is_directory {
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            let writer = self.inner.as_mut().ok_or(OtterzipError::InvalidArgument(
                "encrypted zip writer already committed",
            ))?;
            writer.add_directory(entry_path, opts).map_err(map_zip_err)?;
            return Ok(());
        }
        // Build options borrowing only `self.password` (field-level), so the
        // disjoint `&mut self.inner` borrow below is permitted by NLL.
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(self.method)
            .compression_level(self.level)
            .unix_permissions(0o644)
            .with_aes_encryption(zip::AesMode::Aes256, self.password.as_str());
        let writer = self.inner.as_mut().ok_or(OtterzipError::InvalidArgument(
            "encrypted zip writer already committed",
        ))?;
        writer.start_file(entry_path, opts).map_err(map_zip_err)?;
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
