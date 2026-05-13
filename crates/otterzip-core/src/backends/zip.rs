//! ZIP backend — wraps the `zip` crate for Sprint 1 read path.
//!
//! Performance notes (per `performance.md` §2, §5):
//! * The archive file is opened once with `BufReader` so random-access
//!   reads during entry iteration don't cause syscall storms.
//! * Entry enumeration allocates per-entry only for the owned
//!   `String` fields of the public `Entry` POD (unavoidable per the
//!   API contract in `rust-core-api.md` §1.2). No per-byte allocations
//!   occur in the extraction path.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use rayon::prelude::*;
use zeroize::Zeroizing;
use zip::ZipArchive;

use crate::archive::ExtractWarning;
use crate::backends::multi_volume_reader::open_for_sequential_read;
use crate::backends::spanned_zip::SpannedZipReader;
use crate::backends::{ArchiveBackend, StreamingExtractCtx};
use crate::entry::{Entry, HostOs};
use crate::error::{Result, OtterzipError};
use crate::format::{CompressionMethod, EncryptionMethod};
use crate::options::OverwritePolicy;
use crate::progress::{Progress, ProgressPhase};

/// Reader source for the ZIP backend. Single-file is the common case;
/// multi-volume uses the [`SpannedZipReader`] overlay so APPNOTE-spanned
/// archives (per-disk offset references) and raw byte-split archives
/// both look like single-disk ZIPs to the upstream `zip` crate.
///
/// Both variants are wrapped in `BufReader` so the upstream parser's
/// many small reads (each CD record is ~46 bytes + variable filename;
/// each local-file-header decode kicks off a chain of small header
/// reads before the entry payload begins) coalesce into ~64 KiB
/// syscalls. Without this, the multi-volume path was hitting the
/// underlying file once per `zip` crate read — a measurable
/// regression on archives whose payload is decompressed by zlib-ng
/// quickly enough that syscall overhead dominates.
pub(crate) enum ZipReader {
    Single(BufReader<File>),
    Multi(BufReader<SpannedZipReader>),
}

impl Read for ZipReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Single(r) => r.read(buf),
            Self::Multi(r) => r.read(buf),
        }
    }
}

impl Seek for ZipReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Single(r) => r.seek(pos),
            Self::Multi(r) => r.seek(pos),
        }
    }
}

/// Wrapper around `zip::ZipArchive` over either a single-file or
/// multi-volume reader.
///
/// Held behind `Box<dyn ArchiveBackend>` in `Archive`. The inner archive
/// is wrapped in a `RefCell` because the `zip` crate requires `&mut self`
/// on every read, while our public API exposes `&self` methods per
/// `rust-core-api.md` §1.1. Safety of `RefCell` here relies on `Archive`
/// being `!Sync` — the doc contract already forbids cross-thread sharing
/// without a mutex (§5).
/// Where each parallel-extract worker should re-open the archive
/// from. Captured at `open` time so the worker closures don't need
/// to inspect [`ZipBackend`]'s state during dispatch.
#[derive(Clone)]
pub(crate) enum OpenSource {
    Single(PathBuf),
    Multi(Vec<PathBuf>),
}

pub(crate) struct ZipBackend {
    inner: RefCell<ZipArchive<ZipReader>>,
    /// Held in zeroized memory so the bytes are wiped on drop. We clone
    /// when calling into the `zip` crate's password APIs; the source-of-
    /// truth string lives here and is never logged or formatted.
    password: Option<Zeroizing<String>>,
    /// Where parallel-extract workers re-open the archive. For
    /// single-volume this is one path; for multi-volume it carries
    /// the full ordered volume list so each worker can rebuild its
    /// own `SpannedZipReader`. Per-worker file handle cost: N volumes
    /// × workers — fine for typical splits (28 vols × 8 workers = 224
    /// FDs, well under the process limit).
    source: OpenSource,
}

impl ZipBackend {
    pub(crate) fn open(path: &Path, password: Option<&Zeroizing<String>>) -> Result<Self> {
        let t_file_open = std::time::Instant::now();
        let file = open_for_sequential_read(path)?;
        tracing::debug!(
            target: "otterzip::zip",
            path = %path.display(),
            elapsed_us = t_file_open.elapsed().as_micros() as u64,
            "ZipBackend::open file opened"
        );
        log_tail_bytes(path);
        let reader = ZipReader::Single(BufReader::new(file));
        let t_zip_new = std::time::Instant::now();
        let inner = ZipArchive::new(reader).map_err(|e| {
            tracing::warn!(
                target: "otterzip::zip",
                path = %path.display(),
                elapsed_ms = t_zip_new.elapsed().as_millis() as u64,
                error = ?e,
                "ZipBackend::open ZipArchive::new returned error"
            );
            map_zip_err(e)
        })?;
        tracing::info!(
            target: "otterzip::zip",
            path = %path.display(),
            elapsed_ms = t_zip_new.elapsed().as_millis() as u64,
            entry_count = inner.len(),
            "ZipBackend::open ZipArchive::new done"
        );
        Ok(Self {
            inner: RefCell::new(inner),
            password: password.cloned(),
            source: OpenSource::Single(path.to_path_buf()),
        })
    }

    /// Open a spanned / split ZIP given its volume paths in disk order
    /// (first volume → last). The volumes are presented to the `zip`
    /// crate as a single virtual seekable stream via
    /// [`MultiVolumeReader`], so the central directory and per-entry
    /// local headers can reference absolute byte positions across the
    /// concatenation without any disk-boundary awareness in the
    /// upstream crate.
    ///
    /// Limitations: backed by the same parser as single-volume ZIP, so
    /// archives whose central-directory entries set
    /// `disk_number_start > 0` may fail with a malformed-archive error
    /// from the `zip` crate's single-disk assertion — those layouts
    /// require a v1.1 deeper integration. The "split byte-stream"
    /// shape produced by WinRAR / 7-Zip / Info-ZIP `zip -s` works.
    pub(crate) fn open_multi(volumes: &[PathBuf], password: Option<&Zeroizing<String>>) -> Result<Self> {
        if volumes.is_empty() {
            return Err(OtterzipError::InvalidArgument(
                "ZipBackend::open_multi requires at least one volume",
            ));
        }
        let szr = SpannedZipReader::open(volumes)?;
        // 64 KiB BufReader chunks coalesce the upstream `zip` crate's
        // many small reads into ~16 syscalls per volume rather than
        // one per read. Matches the Single-volume path's buffering.
        let reader = ZipReader::Multi(BufReader::with_capacity(64 * 1024, szr));
        let inner = ZipArchive::new(reader).map_err(map_zip_err)?;
        Ok(Self {
            inner: RefCell::new(inner),
            password: password.cloned(),
            source: OpenSource::Multi(volumes.to_vec()),
        })
    }

    /// Resolve a name to an open `ZipFile`, applying the stored password
    /// when the entry is encrypted. Wrapped here so both `extract_entry`
    /// and `open_entry_stream` get the same decryption discipline.
    fn read_by_name<'a>(
        archive: &'a mut ZipArchive<ZipReader>,
        name: &str,
        password: Option<&Zeroizing<String>>,
    ) -> Result<zip::read::ZipFile<'a>> {
        // Probe the entry to see whether it needs a password. We trial
        // `by_name`, classify the error, and only *then* re-borrow with
        // `by_name_decrypt`. The probe's borrow is released at the end of
        // the match block because `probe_outcome` is a `bool`, not a
        // reference back into `archive`.
        enum Outcome {
            Plain,
            NeedsPassword,
            NotFound,
            Other(OtterzipError),
        }
        let outcome = {
            match archive.by_name(name) {
                Ok(_) => Outcome::Plain,
                Err(zip::result::ZipError::FileNotFound) => Outcome::NotFound,
                Err(zip::result::ZipError::UnsupportedArchive(msg))
                    if msg_indicates_encryption(msg) =>
                {
                    Outcome::NeedsPassword
                }
                Err(other) => Outcome::Other(map_zip_err(other)),
            }
        };
        match outcome {
            Outcome::Plain => archive.by_name(name).map_err(|e| match e {
                zip::result::ZipError::FileNotFound => {
                    OtterzipError::EntryNotFound(name.to_string())
                }
                other => map_zip_err(other),
            }),
            Outcome::NeedsPassword => {
                let p = password
                    .ok_or_else(|| {
                        // Trace the *fact* of a missing password, never
                        // the value. Useful for distinguishing genuine
                        // user errors from instrumentation noise.
                        tracing::info!(
                            target: "otterzip::security",
                            event = "wrong_password",
                            reason = "no_password_supplied",
                            "encrypted entry without password"
                        );
                        OtterzipError::WrongPassword
                    })?
                    .as_bytes();
                archive.by_name_decrypt(name, p).map_err(|e| match e {
                    zip::result::ZipError::InvalidPassword => {
                        tracing::info!(
                            target: "otterzip::security",
                            event = "wrong_password",
                            reason = "invalid_password",
                            "decryption failed for entry"
                        );
                        OtterzipError::WrongPassword
                    }
                    zip::result::ZipError::FileNotFound => {
                        OtterzipError::EntryNotFound(name.to_string())
                    }
                    other => map_zip_err(other),
                })
            }
            Outcome::NotFound => Err(OtterzipError::EntryNotFound(name.to_string())),
            Outcome::Other(e) => Err(e),
        }
    }
}

/// True when an `UnsupportedArchive` message signals that the entry is
/// encrypted (zip 2.x emits "Password required to decrypt file" or similar
/// — exact wording varies between versions, so we match loosely).
fn msg_indicates_encryption(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("password") || lower.contains("encrypt")
}

impl ArchiveBackend for ZipBackend {
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>> {
        // We materialize all entries up front. `ZipArchive::by_index`
        // returns a `ZipFile` that mutably borrows the archive, so a lazy
        // iterator would need self-referential storage. Enumeration is
        // not in a hot loop (per `performance.md` §2 — hot loops are the
        // per-byte decompression paths inside backends), so the one-shot
        // Vec allocation is negligible relative to I/O.
        let mut archive = self.inner.borrow_mut();
        let count = archive.len();
        let mut collected: Vec<Result<Entry>> = Vec::with_capacity(count);
        for i in 0..count {
            collected.push(entry_at(&mut *archive, i));
        }
        Ok(Box::new(collected.into_iter()))
    }

    fn extract_entry(&self, entry_path: &str, out: &mut dyn std::io::Write) -> Result<u64> {
        let mut archive = self.inner.borrow_mut();
        let mut zf = Self::read_by_name(&mut archive, entry_path, self.password.as_ref())?;
        let written = std::io::copy(&mut zf, out)?;
        Ok(written)
    }

    fn open_entry_stream(&self, entry_path: &str) -> Result<Box<dyn Read + Send + '_>> {
        // `zip::ZipFile` is not `Send` and borrows the archive mutably.
        // Sprint 1 compromise: eagerly copy into an in-memory `Vec` and
        // hand back a `Cursor`. Large-entry streaming is deferred to
        // Sprint 3 once the backend contract grows a proper streaming
        // method that respects `RefCell` borrow scoping.
        let mut archive = self.inner.borrow_mut();
        let mut zf = Self::read_by_name(&mut archive, entry_path, self.password.as_ref())?;
        let expected = zf.size();
        let cap = usize::try_from(expected).unwrap_or(0);
        let mut buf = Vec::with_capacity(cap);
        std::io::copy(&mut zf, &mut buf)?;
        Ok(Box::new(std::io::Cursor::new(buf)))
    }

    fn extract_all_streaming(
        &self,
        ctx: &mut StreamingExtractCtx<'_>,
    ) -> Option<Result<()>> {
        // Multi-volume now participates in the parallel path: each
        // worker re-opens the archive via `open_local(&self.source)`
        // which dispatches to either `File::open` (Single) or
        // `SpannedZipReader::open` (Multi). User benchmark
        // (1.35 GB / 9-entry spanned ZIP) showed disk usage 30–40%
        // serial — workers needed to saturate the NVMe.
        //
        // Decision rule (per `performance.md` §4): only switch to the
        // parallel path when there's enough work to amortise the
        // per-thread `ZipArchive::new` cost (re-parses the central
        // directory). For small archives the serial path in
        // `archive.rs::extract_all` is faster — return None to fall back.
        let entries = match self.inner.borrow_mut().len() {
            0 => return None,
            n => n,
        };
        let total_compressed: u64 = {
            let mut a = self.inner.borrow_mut();
            (0..a.len())
                .filter_map(|i| a.by_index_raw(i).ok().map(|f| f.compressed_size()))
                .sum()
        };
        const PARALLEL_MIN_BYTES: u64 = 4 * 1024 * 1024;
        const PARALLEL_MIN_ENTRIES: usize = 8;
        // Tiny-file regression guard. The original 32 KiB threshold
        // was tuned against synthetic 1024 × 1 KiB archives where the
        // total payload was sub-megabyte and NTFS metadata
        // serialisation made parallel a loss. Real-world "many small
        // files" archives (typical user case — game asset dumps,
        // source trees, photo libraries — many GB across 100K+ tiny
        // entries) show the opposite profile: per-entry CPU+I/O is
        // small, but the *aggregate* serial time is dominated by
        // total volume × decoder throughput, which parallelises fine.
        //
        // Two-tier rule:
        //   * archives ≥ 50 MiB total bypass the avg-entry check
        //     entirely (large workloads always win from parallel,
        //     even with NTFS contention),
        //   * smaller archives keep the 32 KiB safety to avoid the
        //     synthetic-fixture regression.
        //
        // Symptom that triggered this re-tune: 7 GB / many-file ZIP
        // ran serial (CPU 14 %, disk read 75 MB/s, write 0 from the
        // user's view on the source disk), wall ~3× Bandizip.
        const PARALLEL_MIN_AVG_ENTRY_BYTES: u64 = 32 * 1024;
        const PARALLEL_LARGE_ARCHIVE_BYTES: u64 = 50 * 1024 * 1024;
        let avg = if entries == 0 {
            0
        } else {
            total_compressed / entries as u64
        };
        let large_enough_to_skip_avg_check =
            total_compressed >= PARALLEL_LARGE_ARCHIVE_BYTES;
        let go_serial = entries < PARALLEL_MIN_ENTRIES
            || total_compressed < PARALLEL_MIN_BYTES
            || (!large_enough_to_skip_avg_check && avg < PARALLEL_MIN_AVG_ENTRY_BYTES);
        tracing::info!(
            target: "otterzip::extract",
            entries,
            total_compressed,
            avg_entry = avg,
            large = large_enough_to_skip_avg_check,
            is_multi = matches!(self.source, OpenSource::Multi(_)),
            decision = if go_serial { "serial" } else { "parallel" },
            "zip extract_all_streaming threshold decision"
        );
        if go_serial {
            return None;
        }
        Some(self.extract_all_parallel(ctx))
    }
}

impl ZipBackend {
    /// rayon-driven entry-level parallel extractor. Each worker re-opens
    /// the source archive on its own file descriptor — `ZipArchive` is
    /// cheap to construct (one central-directory parse) but **not** safe
    /// to share across threads, so per-thread handles are the simplest
    /// correct path. See `performance.md` §4 for the design rationale.
    fn extract_all_parallel(&self, ctx: &mut StreamingExtractCtx<'_>) -> Result<()> {
        let dest_root = ctx.dest_root.to_path_buf();
        let opts = ctx.opts.clone();
        let start = ctx.start;
        // PR-7A: clone the borrowed motw payload into an owned Arc so
        // each worker can read it without lifetime gymnastics. None
        // when the user disabled the toggle or source has no MOTW.
        let motw_payload: Option<std::sync::Arc<Vec<u8>>> =
            ctx.motw_payload.map(|p| std::sync::Arc::new(p.to_vec()));
        // Pull the data we need into the closure without aliasing `self`
        // (which holds a `!Sync` `RefCell`). `source` and `password` are
        // cheap to clone; everything else lives in the `ctx`.
        let source = self.source.clone();
        let password = self.password.clone();

        // First pass: gather entry POD + total bytes (metadata-only — we
        // borrow `RefCell` mutably here, but it's released before we
        // hand work to rayon).
        let mut metas: Vec<Entry> = Vec::with_capacity(64);
        {
            let mut archive = self.inner.borrow_mut();
            let count = archive.len();
            for i in 0..count {
                let entry = entry_at(&mut archive, i)?;
                metas.push(entry);
            }
        }
        let total_bytes: u64 = metas.iter().map(|m| m.uncompressed_size).sum();
        let total_entries = u32::try_from(metas.len()).unwrap_or(u32::MAX);
        tracing::info!(
            target: "otterzip::extract",
            entries = total_entries,
            total_uncompressed = total_bytes,
            cd_parse_ms = start.elapsed().as_millis() as u64,
            "parallel extract metadata phase done"
        );

        // Cancellation / progress / accounting is shared across workers.
        // We update the report atomically per-entry; progress fires on a
        // best-effort basis (UI doesn't need every tick).
        let bytes_done = std::sync::atomic::AtomicU64::new(0);
        let entries_done = std::sync::atomic::AtomicU32::new(0);
        let entries_skipped = std::sync::atomic::AtomicU32::new(0);
        let canceled = std::sync::atomic::AtomicBool::new(false);
        // Errors collapse to the *first* one observed — once any worker
        // reports failure we set `canceled` to short-circuit the rest.
        let first_err: Mutex<Option<OtterzipError>> = Mutex::new(None);
        let warnings: Mutex<Vec<ExtractWarning>> = Mutex::new(Vec::new());
        let progress_lock: Mutex<&mut dyn crate::progress::ProgressSink> = Mutex::new(ctx.progress);

        // `for_each_init` gives each rayon worker its own re-opened
        // archive handle, amortising the central-directory parse over
        // every entry the worker takes — critical for small-entry
        // archives where the per-entry overhead would otherwise dominate.
        // For multi-volume this rebuilds the spanned-ZIP patch view
        // per worker (cheap — patched_tail is a few hundred bytes).
        //
        // Worker-count cap: NTFS serialises MFT updates (file create,
        // rename, set-attributes) at the volume level. With rayon's
        // default = num_cpus(), modern 12/16/24-thread CPUs spawn that
        // many workers — all queued behind the same NTFS lock when an
        // archive has thousands of entries, especially clustered into
        // a few hot directories. Observed pathology: 7 GB / many-files
        // archive, CPU 17 %, disk read 57 MB/s, write 1.6 MB/s — the
        // hardware was idling while workers waited on kernel-side
        // metadata locks. Cap at 4 to stay in NTFS's concurrency sweet
        // spot; per-entry CPU work (decompress) parallelises fine
        // within that envelope, and we stop fighting the OS.
        let source_for_init = source.clone();
        let worker_count = std::cmp::min(rayon::current_num_threads(), 4);
        let worker_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .build()
            .map_err(|e| OtterzipError::BackendError(format!(
                "rayon thread-pool build failed: {e}"
            )))?;
        let dispatch_start = std::time::Instant::now();
        tracing::info!(
            target: "otterzip::extract",
            workers = worker_count,
            "parallel extract dispatching to rayon"
        );
        worker_pool.install(|| {
        metas.par_iter().for_each_init(
            || open_local(&source_for_init).ok(),
            |local_handle, entry| {
            if canceled.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }

            // ZIP-bomb gate (same heuristic as the serial path).
            if let Some(err) = crate::archive::__check_bomb_for_streaming(entry, &opts) {
                set_first_err(&first_err, &canceled, err);
                return;
            }

            if entry.is_symlink && !opts.follow_symlinks {
                warnings.lock().unwrap().push(ExtractWarning::SymlinkSkipped {
                    path: entry.path.clone(),
                    target: String::new(),
                });
                entries_skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return;
            }

            let out_path = match resolve_output_path(&dest_root, &entry.path, &opts) {
                Ok(p) => p,
                Err(orig) => {
                    if opts.block_path_traversal {
                        set_first_err(
                            &first_err,
                            &canceled,
                            OtterzipError::PathTraversalBlocked(orig),
                        );
                    } else {
                        warnings.lock().unwrap().push(ExtractWarning::PathTraversalClamped {
                            original: orig,
                            clamped: dest_root.clone(),
                        });
                        entries_skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    return;
                }
            };

            if entry.is_directory {
                if let Err(e) = std::fs::create_dir_all(&out_path) {
                    set_first_err(&first_err, &canceled, OtterzipError::Io(e));
                    return;
                }
                entries_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return;
            }

            if let Some(parent) = out_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    set_first_err(&first_err, &canceled, OtterzipError::Io(e));
                    return;
                }
            }

            if out_path.exists() {
                match opts.overwrite {
                    OverwritePolicy::Never => {
                        set_first_err(
                            &first_err,
                            &canceled,
                            OtterzipError::Io(std::io::Error::new(
                                std::io::ErrorKind::AlreadyExists,
                                out_path.display().to_string(),
                            )),
                        );
                        return;
                    }
                    OverwritePolicy::Always => {}
                    OverwritePolicy::IfNewer | OverwritePolicy::AskCallback => {
                        entries_skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                }
            }

            // Per-worker archive handle. Each rayon thread pays the
            // central-directory parse cost once via this construction —
            // amortised across the entries it processes in this call.
            // Re-using a thread-local handle would be a future
            // optimisation but adds lifetime complexity for marginal gain.
            let local_archive = match local_handle.as_mut() {
                Some(a) => a,
                None => {
                    set_first_err(
                        &first_err,
                        &canceled,
                        OtterzipError::BackendError(
                            "worker failed to open archive handle".into(),
                        ),
                    );
                    return;
                }
            };
            let mut zf = match Self::read_by_name(
                local_archive,
                &entry.path,
                password.as_ref(),
            ) {
                Ok(zf) => zf,
                Err(e) => {
                    set_first_err(&first_err, &canceled, e);
                    return;
                }
            };
            let file = match File::create(&out_path) {
                Ok(f) => f,
                Err(e) => {
                    set_first_err(&first_err, &canceled, OtterzipError::Io(e));
                    return;
                }
            };
            let mut writer = BufWriter::new(file);
            let written = match std::io::copy(&mut zf, &mut writer) {
                Ok(n) => n,
                Err(e) => {
                    set_first_err(&first_err, &canceled, OtterzipError::Io(e));
                    return;
                }
            };
            if let Err(e) = std::io::Write::flush(&mut writer) {
                set_first_err(&first_err, &canceled, OtterzipError::Io(e));
                return;
            }
            // PR-7A: propagate Zone.Identifier from source archive.
            // Best-effort, never aborts the worker.
            if let Some(p) = motw_payload.as_ref() {
                if let Err(e) = crate::motw::write_zone_identifier(&out_path, &p[..]) {
                    tracing::warn!(
                        target: "otterzip::motw",
                        path = %out_path.display(),
                        error = %e,
                        "MOTW propagation skipped (parallel zip extract)"
                    );
                }
            }

            bytes_done.fetch_add(written, std::sync::atomic::Ordering::Relaxed);
            let entries_so_far = entries_done
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;

            // Phase 7 cumulative bomb gate. We accumulate atomically so the
            // exact ordering of worker writes doesn't matter; the moment
            // either the absolute byte cap or the aggregate ratio crosses
            // the configured threshold we fail-fast.
            let _ = (written, entry.compressed_size);
            // (Note: per-entry uncompressed bytes are already counted by
            // `bytes_done`; we additionally check the aggregate against the
            // caller-configured caps. We synthesize a `__BombMonitor`-like
            // check inline because the parallel path can't carry mutable
            // state across worker invocations.)
            if opts.max_total_output_bytes > 0
                && bytes_done.load(std::sync::atomic::Ordering::Relaxed)
                    > opts.max_total_output_bytes
            {
                set_first_err(
                    &first_err,
                    &canceled,
                    OtterzipError::ZipBombSuspected {
                        entry: "<aggregate>".to_string(),
                        ratio: 0,
                        limit: opts.max_total_compression_ratio,
                    },
                );
                return;
            }

            // Progress update: best-effort, only every 8 entries to keep
            // contention on the sink lock minimal.
            if entries_so_far % 8 == 0 || entries_so_far == total_entries {
                if let Ok(mut sink) = progress_lock.try_lock() {
                    let snapshot = Progress {
                        bytes_processed: bytes_done.load(std::sync::atomic::Ordering::Relaxed),
                        bytes_total: total_bytes,
                        entries_processed: entries_so_far,
                        entries_total: total_entries,
                        current_entry: Some(entry.path.clone()),
                        phase: ProgressPhase::Writing,
                        elapsed: start.elapsed(),
                    };
                    if !sink.update(&snapshot) {
                        canceled.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            },
        );
        }); // worker_pool.install
        tracing::info!(
            target: "otterzip::extract",
            dispatch_ms = dispatch_start.elapsed().as_millis() as u64,
            entries_done = entries_done.load(std::sync::atomic::Ordering::Relaxed),
            entries_skipped = entries_skipped.load(std::sync::atomic::Ordering::Relaxed),
            bytes_done = bytes_done.load(std::sync::atomic::Ordering::Relaxed),
            canceled = canceled.load(std::sync::atomic::Ordering::Relaxed),
            "parallel extract rayon pool returned"
        );

        if let Some(err) = first_err.into_inner().unwrap() {
            tracing::warn!(
                target: "otterzip::extract",
                error = %err,
                "parallel extract first_err set"
            );
            return Err(err);
        }
        if canceled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(OtterzipError::Canceled);
        }

        // Flush counters into the report.
        ctx.report.bytes_written += bytes_done.load(std::sync::atomic::Ordering::Relaxed);
        ctx.report.entries_extracted += entries_done.load(std::sync::atomic::Ordering::Relaxed);
        ctx.report.entries_skipped += entries_skipped.load(std::sync::atomic::Ordering::Relaxed);
        ctx.report
            .warnings
            .extend(warnings.into_inner().unwrap());
        Ok(())
    }
}

fn set_first_err(
    slot: &Mutex<Option<OtterzipError>>,
    canceled: &std::sync::atomic::AtomicBool,
    err: OtterzipError,
) {
    let mut guard = slot.lock().unwrap();
    if guard.is_none() {
        *guard = Some(err);
    }
    canceled.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Per-worker archive re-open. Dispatches on [`OpenSource`] so the
/// same closure can drive single-volume and multi-volume parallel
/// extracts. The buffering / sequential-scan hints match the primary
/// open path so worker reads enjoy the same I/O profile.
fn open_local(source: &OpenSource) -> Result<ZipArchive<ZipReader>> {
    match source {
        OpenSource::Single(path) => {
            let file = open_for_sequential_read(path)?;
            let reader = ZipReader::Single(BufReader::new(file));
            ZipArchive::new(reader).map_err(map_zip_err)
        }
        OpenSource::Multi(volumes) => {
            let szr = SpannedZipReader::open(volumes)?;
            let reader = ZipReader::Multi(BufReader::with_capacity(64 * 1024, szr));
            ZipArchive::new(reader).map_err(map_zip_err)
        }
    }
}

fn resolve_output_path(
    dest_root: &Path,
    entry_path: &str,
    opts: &crate::options::ExtractOptions,
) -> std::result::Result<PathBuf, String> {
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
                if crate::archive::__validate_component(&s).is_err() {
                    return Err(entry_path.to_string());
                }
                out.push(c);
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => {
                return Err(entry_path.to_string());
            }
        }
    }
    if !out.starts_with(dest_root) {
        return Err(entry_path.to_string());
    }
    Ok(out)
}

/// Read entry `i` and convert it to our POD. Pulling this out of the
/// iterator body keeps the enumeration loop easy to audit. We use the
/// `_raw` variant so encrypted entries don't trip the metadata pass — the
/// caller still has to provide a password to actually *read* their bytes,
/// but listing them is allowed.
fn entry_at(archive: &mut ZipArchive<ZipReader>, i: usize) -> Result<Entry> {
    let zf = archive.by_index_raw(i).map_err(map_zip_err)?;

    let path = zf.name().to_string();
    let is_directory = zf.is_dir();
    let external = zf.unix_mode();
    let is_symlink = external.is_some_and(|m| (m & 0o170_000) == 0o120_000);

    let compression = map_compression(zf.compression());
    let encryption = if zf.encrypted() {
        EncryptionMethod::ZipCrypto
    } else {
        EncryptionMethod::None
    };

    let crc32 = Some(zf.crc32());
    // `last_modified()` in zip 2.x returns `Option<DateTime>`. If a future
    // version reverts to plain `DateTime`, collapse the `.and_then(...)`
    // to a direct conversion.
    let modified = zf.last_modified().and_then(zip_datetime_to_system_time);
    let comment_raw = zf.comment();
    let comment = if comment_raw.is_empty() {
        None
    } else {
        Some(comment_raw.to_string())
    };
    let attributes = external.unwrap_or(0);
    let uncompressed_size = zf.size();
    let compressed_size = zf.compressed_size();

    Ok(Entry {
        path,
        is_directory,
        is_symlink,
        uncompressed_size,
        compressed_size,
        compression,
        encryption,
        crc32,
        modified,
        accessed: None,
        created: None,
        attributes,
        comment,
        host_os: HostOs::Unknown,
    })
}

/// Convert a `ZipError` into our error taxonomy without losing context.
/// Diagnostic helper — dump the last 96 bytes of a file as hex into the
/// log. EOCD lives at the tail of every well-formed ZIP, so a quick
/// peek tells us whether the file is even shaped like a ZIP and
/// whether ZIP64 locator bytes (0x504B0607) are present.
fn log_tail_bytes(path: &Path) {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let size = match f.metadata() {
        Ok(m) => m.len(),
        Err(_) => return,
    };
    let tail_len = u64::min(size, 96);
    if f.seek(SeekFrom::End(-(tail_len as i64))).is_err() {
        return;
    }
    let mut buf = vec![0u8; tail_len as usize];
    if f.read_exact(&mut buf).is_err() {
        return;
    }
    let hex: String = buf
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    tracing::info!(
        target: "otterzip::zip",
        path = %path.display(),
        size,
        tail_hex = %hex,
        "ZipBackend::open tail bytes (last 96) for EOCD diagnostic"
    );
}

fn map_zip_err(e: zip::result::ZipError) -> OtterzipError {
    use zip::result::ZipError as Z;
    match e {
        Z::Io(io) => OtterzipError::Io(io),
        Z::InvalidArchive(msg) => OtterzipError::Corrupted {
            reason: msg.to_string(),
            entry: None,
        },
        Z::UnsupportedArchive(msg) => OtterzipError::UnsupportedFormat(Some(msg.to_string())),
        Z::FileNotFound => OtterzipError::EntryNotFound(String::new()),
        other => OtterzipError::BackendError(other.to_string()),
    }
}

fn map_compression(m: zip::CompressionMethod) -> CompressionMethod {
    use zip::CompressionMethod as Z;
    // Only `Stored` and `Deflated` are guaranteed by our feature flags
    // (`default-features = false, features = ["deflate"]` in workspace
    // Cargo.toml). Other codec variants are feature-gated in the `zip`
    // crate; we fall through to `Unknown` rather than listing them and
    // risking compile errors under minimal features.
    match m {
        Z::Stored => CompressionMethod::Store,
        Z::Deflated => CompressionMethod::Deflate,
        _ => CompressionMethod::Unknown,
    }
}

/// Convert a `zip::DateTime` to a `SystemTime`.
///
/// ZIP stores DOS-era wall-clock with 2-second resolution and no timezone,
/// so the result is interpreted as UTC. This is lossy by design.
#[allow(clippy::needless_pass_by_value)] // DateTime is Copy-ish, trivial
fn zip_datetime_to_system_time(dt: zip::DateTime) -> Option<std::time::SystemTime> {
    let year = i32::from(dt.year());
    let month = u32::from(dt.month());
    let day = u32::from(dt.day());
    let hour = u32::from(dt.hour());
    let minute = u32::from(dt.minute());
    let second = u32::from(dt.second());

    let days = days_from_civil(year, month, day)?;
    let secs = i64::from(days) * 86_400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second);
    let secs_u64 = u64::try_from(secs).ok()?;
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs_u64))
}

/// Days since 1970-01-01 for a (proleptic Gregorian) date.
/// Howard Hinnant's civil-from-days algorithm (inverse); O(1), branchless.
fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i32> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let civil_year = if month <= 2 { year - 1 } else { year };
    let era = civil_year.div_euclid(400);
    let yoe = u32::try_from(civil_year.rem_euclid(400)).ok()?; // 0..=399
    let month_offset = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * month_offset + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era.checked_mul(146_097)?
        .checked_add(i32::try_from(doe).ok()?)?
        .checked_sub(719_468)
}
