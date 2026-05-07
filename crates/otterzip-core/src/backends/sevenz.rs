//! 7z backend (Sprint 3 read path).
//!
//! Wraps `sevenz-rust` 0.6. Like tar, 7z is best consumed in a single
//! forward pass — the library exposes random access only via `for_each_entries`
//! which streams sequentially. We therefore implement
//! [`ArchiveBackend::extract_all_streaming`] to avoid re-decompressing the
//! whole archive once per entry on `extract_all`.
//!
//! Solid-block constraint (per `performance.md` §4): when `solid=true`,
//! entries within the same block must be decompressed in order. We let
//! `sevenz-rust` handle that internally — our streaming extractor visits
//! entries in archive order, which is always valid.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufWriter, Read};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use sevenz_rust::{Password, SevenZArchiveEntry, SevenZReader};
use zeroize::Zeroizing;

use crate::archive::ExtractWarning;
use crate::backends::{ArchiveBackend, StreamingExtractCtx};
use crate::entry::{Entry, HostOs};
use crate::error::{Result, OtterzipError};
use crate::format::{CompressionMethod, EncryptionMethod};
use crate::options::OverwritePolicy;
use crate::progress::{Progress, ProgressPhase};

pub(crate) struct SevenZBackend {
    path: PathBuf,
    password: Option<Zeroizing<String>>,
    /// Cached metadata snapshot — sevenz-rust's `Archive` already lists
    /// every entry without decompressing data, so this is cheap to build
    /// once and reuse.
    cache: RefCell<Option<Vec<Entry>>>,
}

impl SevenZBackend {
    pub(crate) fn open(path: &Path, password: Option<&Zeroizing<String>>) -> Result<Self> {
        // Verify openability + password up front. Headers themselves can
        // be encrypted in 7z; mismatched password surfaces here.
        let _ = open_reader(path, password)?;
        Ok(Self {
            path: path.to_path_buf(),
            password: password.cloned(),
            cache: RefCell::new(None),
        })
    }

    fn metadata_pass(&self) -> Result<Vec<Entry>> {
        let reader = open_reader(&self.path, self.password.as_ref())?;
        let archive = reader.archive();
        let mut out = Vec::with_capacity(archive.files.len());
        for f in &archive.files {
            out.push(meta_to_entry(f));
        }
        Ok(out)
    }
}

impl ArchiveBackend for SevenZBackend {
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>> {
        if self.cache.borrow().is_none() {
            let v = self.metadata_pass()?;
            *self.cache.borrow_mut() = Some(v);
        }
        let snapshot = self.cache.borrow().as_ref().unwrap().clone();
        Ok(Box::new(snapshot.into_iter().map(Ok)))
    }

    fn extract_entry(&self, entry_path: &str, out: &mut dyn std::io::Write) -> Result<u64> {
        let mut reader = open_reader(&self.path, self.password.as_ref())?;
        let mut written: u64 = 0;
        let target = entry_path.to_string();
        let mut found = false;
        reader
            .for_each_entries(|entry, body| {
                if entry.name == target {
                    found = true;
                    let n = io::copy(body, out).map_err(map_sevenz_err_io)?;
                    written = n;
                    // Returning Ok(false) tells sevenz-rust to stop iteration.
                    return Ok(false);
                }
                Ok(true)
            })
            .map_err(map_sevenz_err)?;
        if !found {
            return Err(OtterzipError::EntryNotFound(entry_path.to_string()));
        }
        Ok(written)
    }

    fn open_entry_stream(&self, entry_path: &str) -> Result<Box<dyn Read + Send + '_>> {
        let mut buf = Vec::new();
        self.extract_entry(entry_path, &mut buf)?;
        Ok(Box::new(io::Cursor::new(buf)))
    }

    fn extract_all_streaming(
        &self,
        ctx: &mut StreamingExtractCtx<'_>,
    ) -> Option<Result<()>> {
        Some(self.extract_all_inner(ctx))
    }
}

impl SevenZBackend {
    fn extract_all_inner(&self, ctx: &mut StreamingExtractCtx<'_>) -> Result<()> {
        // Cheap metadata pass to size the progress totals. sevenz-rust's
        // archive() does not require body decompression so the cost is
        // bounded by header size — typically <1% of full extract time.
        let entries_meta = self.metadata_pass()?;
        let total_bytes: u64 = entries_meta.iter().map(|e| e.uncompressed_size).sum();
        let total_entries = u32::try_from(entries_meta.len()).unwrap_or(u32::MAX);

        let mut reader = open_reader(&self.path, self.password.as_ref())?;

        // Buffers borrowed across closure invocations — we hand a
        // `RefCell` into sevenz-rust's `for_each_entries` callback so the
        // shared mutable state (report/progress/start) doesn't run afoul
        // of the FnMut requirement plus the read-from-body needs.
        let report_cell: RefCell<&mut crate::archive::ExtractReport> =
            RefCell::new(ctx.report);
        let progress_cell: RefCell<&mut dyn crate::progress::ProgressSink> =
            RefCell::new(ctx.progress);
        let dest_root = ctx.dest_root;
        let opts = ctx.opts;
        let start = ctx.start;
        let motw_payload = ctx.motw_payload;

        let mut idx: u32 = 0;
        let mut canceled = false;
        let mut traversal_err: Option<String> = None;

        reader
            .for_each_entries(|entry, body| {
                if canceled {
                    return Ok(false);
                }
                let path_str = entry.name.clone();
                let pod = meta_to_entry(entry);

                // Progress tick.
                let bytes_processed = report_cell.borrow().bytes_written;
                let snapshot = Progress {
                    bytes_processed,
                    bytes_total: total_bytes,
                    entries_processed: idx,
                    entries_total: total_entries,
                    current_entry: Some(path_str.clone()),
                    phase: ProgressPhase::Writing,
                    elapsed: start.elapsed(),
                };
                if !progress_cell.borrow_mut().update(&snapshot) {
                    canceled = true;
                    return Ok(false);
                }
                idx = idx.saturating_add(1);

                // Bomb gate.
                if let Some(err) =
                    crate::archive::__check_bomb_for_streaming(&pod, opts)
                {
                    return Err(sevenz_rust::Error::other(err.to_string()));
                }

                if pod.is_symlink && !opts.follow_symlinks {
                    let mut r = report_cell.borrow_mut();
                    r.warnings.push(ExtractWarning::SymlinkSkipped {
                        path: path_str,
                        target: String::new(),
                    });
                    r.entries_skipped += 1;
                    return Ok(true);
                }

                let out_path =
                    match resolve_output_path_streaming(dest_root, &path_str, opts) {
                        Ok(p) => p,
                        Err(orig) => {
                            if opts.block_path_traversal {
                                traversal_err = Some(orig);
                                return Ok(false);
                            }
                            let mut r = report_cell.borrow_mut();
                            r.warnings.push(ExtractWarning::PathTraversalClamped {
                                original: orig,
                                clamped: dest_root.to_path_buf(),
                            });
                            r.entries_skipped += 1;
                            return Ok(true);
                        }
                    };

                if pod.is_directory {
                    std::fs::create_dir_all(&out_path).map_err(map_sevenz_err_io)?;
                    report_cell.borrow_mut().entries_extracted += 1;
                    return Ok(true);
                }

                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent).map_err(map_sevenz_err_io)?;
                }

                if out_path.exists() {
                    match opts.overwrite {
                        OverwritePolicy::Never => {
                            return Err(sevenz_rust::Error::other(format!(
                                "destination exists: {}",
                                out_path.display()
                            )));
                        }
                        OverwritePolicy::Always => {}
                        OverwritePolicy::IfNewer | OverwritePolicy::AskCallback => {
                            report_cell.borrow_mut().entries_skipped += 1;
                            return Ok(true);
                        }
                    }
                }

                let file = File::create(&out_path).map_err(map_sevenz_err_io)?;
                let mut writer = BufWriter::new(file);
                let written = io::copy(body, &mut writer).map_err(map_sevenz_err_io)?;
                use std::io::Write as _;
                writer.flush().map_err(map_sevenz_err_io)?;
                // PR-7A: MOTW propagation. Best-effort.
                if let Some(payload) = motw_payload {
                    if let Err(e) = crate::motw::write_zone_identifier(&out_path, payload) {
                        tracing::warn!(
                            target: "otterzip::motw",
                            path = %out_path.display(),
                            error = %e,
                            "MOTW propagation skipped (7z extract)"
                        );
                    }
                }
                let mut r = report_cell.borrow_mut();
                r.bytes_written += written;
                r.entries_extracted += 1;
                Ok(true)
            })
            .map_err(map_sevenz_err)?;

        if let Some(orig) = traversal_err {
            return Err(OtterzipError::PathTraversalBlocked(orig));
        }
        if canceled {
            return Err(OtterzipError::Canceled);
        }
        Ok(())
    }
}

fn open_reader(path: &Path, password: Option<&Zeroizing<String>>) -> Result<SevenZReader<File>> {
    let pw = match password {
        Some(p) => Password::from(p.as_str()),
        None => Password::empty(),
    };
    SevenZReader::open(path, pw).map_err(map_sevenz_err)
}

fn meta_to_entry(entry: &SevenZArchiveEntry) -> Entry {
    let modified = unix_seconds_from_filetime(entry.last_modified_date.into())
        .and_then(|s| UNIX_EPOCH.checked_add(Duration::from_secs(s)));
    Entry {
        path: entry.name.clone(),
        is_directory: entry.is_directory,
        is_symlink: false, // 7z does not encode POSIX symlinks in metadata.
        uncompressed_size: entry.size,
        compressed_size: entry.compressed_size,
        compression: CompressionMethod::Lzma2,
        encryption: EncryptionMethod::None,
        crc32: if entry.has_crc {
            Some(entry.crc as u32)
        } else {
            None
        },
        modified,
        accessed: None,
        created: None,
        attributes: 0,
        comment: None,
        host_os: HostOs::Unknown,
    }
}

/// 7z stores timestamps as Windows FILETIME (100-ns ticks since 1601-01-01).
/// We convert to Unix epoch seconds; values before 1970 are clamped to 0.
fn unix_seconds_from_filetime(ft: u64) -> Option<u64> {
    // 1601-01-01 → 1970-01-01 = 11_644_473_600 seconds.
    const FT_TO_UNIX_DELTA_SECS: u64 = 11_644_473_600;
    const FT_TICKS_PER_SEC: u64 = 10_000_000;
    let secs = ft / FT_TICKS_PER_SEC;
    secs.checked_sub(FT_TO_UNIX_DELTA_SECS)
}

fn map_sevenz_err(e: sevenz_rust::Error) -> OtterzipError {
    use sevenz_rust::Error as E;
    match e {
        E::Io(io, _) => OtterzipError::Io(io),
        E::PasswordRequired | E::ChecksumVerificationFailed => OtterzipError::WrongPassword,
        E::MaybeBadPassword(_) => OtterzipError::WrongPassword,
        other => OtterzipError::BackendError(other.to_string()),
    }
}

fn map_sevenz_err_io(e: io::Error) -> sevenz_rust::Error {
    sevenz_rust::Error::other(e.to_string())
}

fn resolve_output_path_streaming(
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
