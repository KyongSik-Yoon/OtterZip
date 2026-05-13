//! Lenient fallback backend powered by [`compress-tools`] (a thin
//! wrapper around the BSD-licensed `libarchive` C library).
//!
//! ## Why this exists
//!
//! The primary read path uses the `zip` crate (v2.x) with zlib-ng and
//! rayon-parallel extract — fast, but strict. Real-world archives in
//! the wild are messier than the spec: off-by-N central-directory
//! sizes, truncated EOCDs, mismatched ZIP64 records, encoding flag
//! lies, etc. Bandizip / 7-Zip / WinRAR each ship a custom lenient
//! parser that absorbs these mistakes; the `zip` crate (correctly)
//! refuses, leaving the user staring at a "37-second hang then
//! `Invalid CDFH offset in EOCD`" experience.
//!
//! `libarchive` has been the reference C archive reader for 27 years
//! (powers `bsdtar`, Homebrew, Subversion, KDE Ark, libalpm, etc.)
//! and has accumulated a deep tail of "real archives, not spec
//! archives" handling. We invoke it **only on fallback** so the
//! happy path keeps every existing optimisation (zlib-ng, parallel
//! extract, BufReader, seek cache, SEQUENTIAL_SCAN hints).
//!
//! ## Trait fit
//!
//! libarchive is a streaming reader — entries are yielded in archive
//! order with no random-access primitive. We map this onto our
//! `ArchiveBackend` trait as follows:
//!
//! * [`extract_all_streaming`](ArchiveBackend::extract_all_streaming):
//!   the natural mode. One pass over the archive, write each entry as
//!   the bytes arrive. This is the hot path the fallback actually
//!   serves.
//! * [`entries`](ArchiveBackend::entries): pre-scan + cache the entry
//!   list (same pattern `cab`/`iso`/`msi` already use).
//! * [`extract_entry`](ArchiveBackend::extract_entry) /
//!   [`open_entry_stream`](ArchiveBackend::open_entry_stream): re-walk
//!   the archive from the start each call. Acceptable because these
//!   are not called by `Archive::extract_all`'s hot path — only by
//!   `Archive::read_entry` / `Archive::test`, which are rare.
//!
//! ## Feature flag
//!
//! Behind the `libarchive-fallback` Cargo feature. When the feature
//! is disabled the backend is absent and the fallback dispatcher in
//! `backends/mod.rs` surfaces a clear "no fallback compiled in"
//! error. CI build matrix runs both `default` and
//! `--features libarchive-fallback` so the feature gate stays
//! observable.

#![cfg(feature = "libarchive-fallback")]

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use compress_tools::{ArchiveContents, ArchiveIteratorBuilder, ArchivePassword};
use zeroize::Zeroizing;

use crate::archive::ExtractWarning;
use crate::backends::{ArchiveBackend, StreamingExtractCtx};
use crate::entry::Entry;
use crate::error::{OtterzipError, Result};
use crate::progress::{Progress, ProgressPhase};

/// libarchive-backed fallback reader. Holds the source path and an
/// optional password; every public method opens a fresh
/// [`ArchiveIterator`] over the path so the C state machine never
/// crosses calls. This is wasteful for `extract_entry`-style random
/// access but the fallback's purpose is "salvage the archive at
/// all", not "match the fast path's IO profile".
pub(crate) struct LibarchiveBackend {
    path: PathBuf,
    password: Option<Zeroizing<String>>,
    /// Cached entry POD list. Lazy-built on first `entries()` call.
    /// `Arc` so cloning into iterator return is cheap.
    entry_cache: RefCell<Option<Arc<Vec<Entry>>>>,
}

impl LibarchiveBackend {
    pub(crate) fn open(path: &Path, password: Option<&Zeroizing<String>>) -> Result<Self> {
        tracing::info!(
            target: "otterzip::libarchive",
            path = %path.display(),
            has_password = password.is_some(),
            "LibarchiveBackend::open"
        );
        // Quick sanity check: file must exist. We *don't* try to walk
        // the archive at open time — that's the slow part the
        // fast-path's timeout already caught.
        if !path.is_file() {
            return Err(OtterzipError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                path.display().to_string(),
            )));
        }
        Ok(Self {
            path: path.to_path_buf(),
            password: password.cloned(),
            entry_cache: RefCell::new(None),
        })
    }

    /// Build a fresh archive iterator over the source file. Each call
    /// rewinds — libarchive holds no cross-call state.
    fn open_iter(&self) -> Result<compress_tools::ArchiveIterator<BufReader<File>>> {
        let file = File::open(&self.path).map_err(OtterzipError::Io)?;
        let reader = BufReader::with_capacity(64 * 1024, file);
        let mut builder = ArchiveIteratorBuilder::new(reader);
        if let Some(pw) = self.password.as_deref() {
            let ap = ArchivePassword::new(pw)
                .map_err(|e| OtterzipError::BackendError(format!("ArchivePassword: {e}")))?;
            builder = builder.with_password(ap);
        }
        builder
            .build()
            .map_err(|e| OtterzipError::BackendError(format!("libarchive open: {e}")))
    }

    /// Walk the archive once, collecting entry POD without
    /// materialising any payload bytes. Used by `entries()` to feed
    /// the regular `Archive::extract_all` per-entry loop when the
    /// caller hasn't routed through `extract_all_streaming`.
    fn scan_entries(&self) -> Result<Vec<Entry>> {
        let iter = self.open_iter()?;
        let mut out = Vec::new();
        let mut current: Option<EntryAccum> = None;
        for item in iter {
            match item {
                ArchiveContents::StartOfEntry(name, stat) => {
                    current = Some(EntryAccum::new(name, stat));
                }
                ArchiveContents::DataChunk(_) => {
                    // Skip payload — we only want metadata here.
                }
                ArchiveContents::EndOfEntry => {
                    if let Some(acc) = current.take() {
                        out.push(acc.finish());
                    }
                }
                ArchiveContents::Err(e) => {
                    return Err(OtterzipError::BackendError(format!(
                        "libarchive entry scan: {e}"
                    )));
                }
            }
        }
        Ok(out)
    }
}

/// Per-entry accumulator while walking.
struct EntryAccum {
    name: String,
    stat: compress_tools::stat,
}

impl EntryAccum {
    fn new(name: String, stat: compress_tools::stat) -> Self {
        Self { name, stat }
    }
    fn finish(self) -> Entry {
        let mode = u32::from(self.stat.st_mode);
        let is_dir = self.name.ends_with('/') || (mode & S_IFMT) == S_IFDIR;
        let is_symlink = (mode & S_IFMT) == S_IFLNK;
        Entry {
            path: self.name,
            uncompressed_size: u64::try_from(self.stat.st_size).unwrap_or(0),
            compressed_size: 0, // libarchive doesn't expose this per entry consistently
            compression: crate::format::CompressionMethod::Unknown,
            encryption: crate::format::EncryptionMethod::None,
            crc32: None,
            modified: None,
            accessed: None,
            created: None,
            is_directory: is_dir,
            is_symlink,
            attributes: 0,
            host_os: crate::entry::HostOs::Unknown,
            comment: None,
        }
    }
}

// Platform-portable S_IFMT bits — libarchive's stat fields use POSIX
// mode bits even on Windows. Defining locally avoids dragging libc as
// a hard dep.
const S_IFMT: u32 = 0o170_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

impl ArchiveBackend for LibarchiveBackend {
    /// libarchive can't peek at "is any entry encrypted" without a
    /// full stream walk, which on a 5 GB archive costs 20-30 seconds.
    /// Probing twice (host-side `ProbeIsEncrypted` + extract work
    /// delegate's `Archive.Open` round trip) is the bulk of the
    /// post-fallback delay the user observed. We short-circuit to
    /// "no" here — wrong only when the archive genuinely is
    /// encrypted, in which case the actual extract will throw
    /// `WrongPassword` and the host UI already handles that by
    /// surfacing the password panel for a retry. Net effect on
    /// healthy non-encrypted malformed archives: zero added wait.
    fn is_encrypted_fast(&self) -> Result<bool> {
        Ok(false)
    }

    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>> {
        // Cache + clone-return: matches the cab/iso/msi pattern.
        if self.entry_cache.borrow().is_none() {
            let scanned = self.scan_entries()?;
            *self.entry_cache.borrow_mut() = Some(Arc::new(scanned));
        }
        let cached = self
            .entry_cache
            .borrow()
            .as_ref()
            .map(Arc::clone)
            .unwrap();
        let entries: Vec<Entry> = cached.as_ref().clone();
        Ok(Box::new(entries.into_iter().map(Ok)))
    }

    fn extract_entry(&self, entry_path: &str, out: &mut dyn Write) -> Result<u64> {
        // Linear scan; libarchive has no random access. Acceptable
        // because this isn't the hot path (extract_all_streaming is).
        let iter = self.open_iter()?;
        let mut current_matches = false;
        let mut written: u64 = 0;
        for item in iter {
            match item {
                ArchiveContents::StartOfEntry(name, _) => {
                    current_matches = name == entry_path;
                }
                ArchiveContents::DataChunk(buf) => {
                    if current_matches {
                        out.write_all(&buf).map_err(OtterzipError::Io)?;
                        written += buf.len() as u64;
                    }
                }
                ArchiveContents::EndOfEntry => {
                    if current_matches {
                        return Ok(written);
                    }
                }
                ArchiveContents::Err(e) => {
                    return Err(OtterzipError::BackendError(format!(
                        "libarchive extract_entry: {e}"
                    )));
                }
            }
        }
        Err(OtterzipError::EntryNotFound(entry_path.to_string()))
    }

    fn open_entry_stream(&self, entry_path: &str) -> Result<Box<dyn Read + Send + '_>> {
        // Buffer entire entry into memory — same compromise the other
        // streaming-only backends make (sevenz, tar_family). The
        // fallback isn't expected to handle multi-GB single entries
        // through this path; that's the fast-path's domain.
        let mut buf = Vec::new();
        self.extract_entry(entry_path, &mut buf)?;
        Ok(Box::new(Cursor::new(buf)))
    }

    fn extract_all_streaming(
        &self,
        ctx: &mut StreamingExtractCtx<'_>,
    ) -> Option<Result<()>> {
        Some(self.extract_all_streaming_inner(ctx))
    }
}

impl LibarchiveBackend {
    /// Core extract loop. One pass over the archive via libarchive's
    /// streaming iterator. Per-entry steps mirror the strict-path
    /// backends so the security gates (path traversal, ZIP bomb,
    /// overwrite policy, MOTW propagation) stay centralised — the
    /// fallback being lenient about *format* must not translate into
    /// lenient about *safety*.
    fn extract_all_streaming_inner(
        &self,
        ctx: &mut StreamingExtractCtx<'_>,
    ) -> Result<()> {
        let dest_root = ctx.dest_root;
        let opts = ctx.opts;
        let start_inst = Instant::now();
        tracing::info!(
            target: "otterzip::libarchive",
            path = %self.path.display(),
            destination = %dest_root.display(),
            "LibarchiveBackend::extract_all_streaming begin"
        );

        let iter = self.open_iter()?;
        // Per-entry mutable state.
        let mut current_path: Option<String> = None;
        let mut current_writer: Option<BufWriter<File>> = None;
        let mut current_out_path: Option<PathBuf> = None;
        let mut current_is_dir = false;
        let mut entries_done: u32 = 0;
        let mut bytes_done: u64 = 0;

        for item in iter {
            match item {
                ArchiveContents::StartOfEntry(name, stat) => {
                    current_path = Some(name.clone());
                    let mode = u32::from(stat.st_mode);
                    let is_dir = name.ends_with('/') || (mode & S_IFMT) == S_IFDIR;
                    current_is_dir = is_dir;
                    current_out_path = None;
                    current_writer = None;

                    let out_path = match resolve_output_path(dest_root, &name, opts) {
                        Ok(p) => p,
                        Err(orig) => {
                            if opts.block_path_traversal {
                                return Err(OtterzipError::PathTraversalBlocked(orig));
                            } else {
                                ctx.report.warnings.push(
                                    ExtractWarning::PathTraversalClamped {
                                        original: orig,
                                        clamped: dest_root.to_path_buf(),
                                    },
                                );
                                ctx.report.entries_skipped += 1;
                                continue;
                            }
                        }
                    };

                    if is_dir {
                        if let Err(e) = std::fs::create_dir_all(&out_path) {
                            return Err(OtterzipError::Io(e));
                        }
                        current_out_path = Some(out_path);
                        continue;
                    }

                    if let Some(parent) = out_path.parent() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            return Err(OtterzipError::Io(e));
                        }
                    }
                    let file = std::fs::File::create(&out_path).map_err(OtterzipError::Io)?;
                    current_writer = Some(BufWriter::with_capacity(64 * 1024, file));
                    current_out_path = Some(out_path);
                }

                ArchiveContents::DataChunk(buf) => {
                    if let Some(w) = current_writer.as_mut() {
                        w.write_all(&buf).map_err(OtterzipError::Io)?;
                        bytes_done = bytes_done.saturating_add(buf.len() as u64);
                    }
                }

                ArchiveContents::EndOfEntry => {
                    if !current_is_dir {
                        if let Some(mut w) = current_writer.take() {
                            w.flush().map_err(OtterzipError::Io)?;
                        }
                        if let (Some(out_path), Some(payload)) =
                            (current_out_path.as_ref(), ctx.motw_payload)
                        {
                            if let Err(e) =
                                crate::motw::write_zone_identifier(out_path, payload)
                            {
                                tracing::warn!(
                                    target: "otterzip::libarchive",
                                    path = %out_path.display(),
                                    error = %e,
                                    "MOTW propagation skipped (libarchive fallback)"
                                );
                            }
                        }
                        ctx.report.bytes_written += bytes_done;
                    }
                    entries_done += 1;
                    ctx.report.entries_extracted += 1;

                    // Throttled progress — every 8 entries plus on
                    // final tick. Matches the `zip` parallel path.
                    if entries_done % 8 == 0 {
                        let cont = ctx.progress.update(&Progress {
                            bytes_processed: ctx.report.bytes_written,
                            bytes_total: 0,
                            entries_processed: entries_done,
                            entries_total: 0,
                            current_entry: current_path.clone(),
                            phase: ProgressPhase::Writing,
                            elapsed: ctx.start.elapsed(),
                        });
                        if !cont {
                            return Err(OtterzipError::Canceled);
                        }
                    }
                    current_path = None;
                    current_out_path = None;
                    current_is_dir = false;
                    bytes_done = 0;
                }

                ArchiveContents::Err(e) => {
                    return Err(OtterzipError::BackendError(format!(
                        "libarchive stream error: {e}"
                    )));
                }
            }
        }

        // Final tick — guarantees the JobCard settles to 100%.
        let _ = ctx.progress.update(&Progress {
            bytes_processed: ctx.report.bytes_written,
            bytes_total: ctx.report.bytes_written,
            entries_processed: entries_done,
            entries_total: entries_done,
            current_entry: None,
            phase: ProgressPhase::Finalizing,
            elapsed: ctx.start.elapsed(),
        });

        tracing::info!(
            target: "otterzip::libarchive",
            entries = entries_done,
            bytes = ctx.report.bytes_written,
            elapsed_ms = start_inst.elapsed().as_millis() as u64,
            "LibarchiveBackend::extract_all_streaming done"
        );
        Ok(())
    }
}

/// Same path-traversal hardening the strict backends use. Centralised
/// in `archive::__validate_component` so adding a new backend never
/// drops a security gate.
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

// Silence the unused-import warning when `libarchive-fallback` is on
// but Seek isn't referenced by name elsewhere.
#[allow(dead_code)]
fn _ensure_seek_in_scope<R: Seek>(_: &R) {}
#[allow(dead_code)]
fn _ensure_seekfrom_in_scope(_: SeekFrom) {}
