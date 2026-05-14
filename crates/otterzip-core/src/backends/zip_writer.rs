//! Self-contained ZIP writer — symmetric to `backends::lenient_zip` on
//! the encode side.
//!
//! ## Why this exists
//!
//! The strict + lenient *read* paths already understand every byte of
//! the ZIP format. On the write side we were leaning on the upstream
//! `zip` crate, which buys correctness but locks us into its serial
//! `flate2`-streaming encoder. The compress-speed gap vs Bandizip
//! (10× slower on the user's 7 GB corpus, isolated to single-CPU-core
//! deflate by the Day-9 throughput diagnostic at
//! `target/debug/otterzip_ffi.dll`) traces directly to that lock-in:
//! one rayon worker would buy 3–4×, libdeflater one-shot adds
//! another 1.2–1.5×, and the easiest path to both is to own the
//! writer end-to-end.
//!
//! ## Scope — Commit 1 (serial path, libdeflater + flate2 dispatch)
//!
//!   * Single-threaded writer that produces byte-identical output to
//!     the reference `zip`-crate writer for the cases the production
//!     pipeline exercises (Stored / Deflate, regular + ZIP64).
//!   * Per-entry deflate dispatch: libdeflater one-shot for entries
//!     whose uncompressed payload fits the
//!     `LIBDEFLATER_ONESHOT_THRESHOLD` (16 MiB by default — matches
//!     the read-side libdeflater path), `flate2` streaming with a
//!     seek-back size patch otherwise.
//!   * ZIP64 escalation: any of cd_offset / cd_size / total_entries /
//!     a single entry's compressed_size / uncompressed_size /
//!     local_header_offset that exceeds its 32-bit slot triggers the
//!     ZIP64 EOCD locator + record, and the per-entry ZIP64 extra
//!     (tag 0x0001).
//!
//! Commit 2 adds the rayon worker pool by promoting
//! [`encode_entry_payload`] into the worker drop-off function — the
//! main thread still serialises LFH + payload writes (so byte
//! offsets stay strictly monotonic and the CDFH `local_header_offset`
//! field is correct) but worker threads do the actual deflate work
//! in parallel.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{OtterzipError, Result};

// === Format constants ================================================

/// "PK\003\004" — local file header signature.
const SIG_LFH: u32 = 0x0403_4b50;
/// "PK\001\002" — central-directory file header signature.
const SIG_CDFH: u32 = 0x0201_4b50;
/// "PK\005\006" — end-of-central-directory record signature.
const SIG_EOCD: u32 = 0x0605_4b50;
/// "PK\006\006" — ZIP64 end-of-central-directory record.
const SIG_ZIP64_EOCD: u32 = 0x0606_4b50;
/// "PK\006\007" — ZIP64 end-of-central-directory locator.
const SIG_ZIP64_LOCATOR: u32 = 0x0706_4b50;

const LFH_FIXED_SIZE: u64 = 30;
const CDFH_FIXED_SIZE: u64 = 46;
const EOCD_FIXED_SIZE: u64 = 22;
const ZIP64_EOCD_FIXED_SIZE: u64 = 56;
const ZIP64_EOCD_LOCATOR_SIZE: u64 = 20;

/// APPNOTE.TXT §4.5.3 — ZIP64 extra field tag.
const EXTRA_TAG_ZIP64: u16 = 0x0001;

/// Entries whose uncompressed payload sits at or below this threshold
/// take the libdeflater one-shot path. Above it we fall through to
/// `flate2` streaming so per-entry working memory stays bounded.
/// Matches the read-side threshold in `backends::lenient_zip` so
/// the two surfaces share a calibration story.
const LIBDEFLATER_ONESHOT_THRESHOLD: u64 = 16 * 1024 * 1024;

/// APPNOTE.TXT §4.4.3 — version-needed-to-extract minor codes.
const VERSION_NEEDED_BASE: u16 = 20;
const VERSION_NEEDED_ZIP64: u16 = 45;

/// GP bit 11 (EFS) — filename + comment are UTF-8. We always set this
/// because every name we accept on the public surface is `&str` →
/// guaranteed UTF-8 in Rust.
const GP_FLAG_UTF8_NAMES: u16 = 0x0800;

/// External attribute layout — Unix mode in the high 16 bits, DOS
/// attributes in the low 16. Matches what `zip-rs` emits so external
/// tools see identical attribute payloads on our archives vs theirs.
const DOS_ATTR_DIRECTORY: u32 = 0x10;
const UNIX_MODE_FILE: u32 = 0o100_644;
const UNIX_MODE_DIR: u32 = 0o040_755;

// === Public types ====================================================

/// Compression method chosen at writer creation. Per-entry dispatch
/// against this enum picks the encoder in
/// [`ZipFileWriter::add_entry`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Compression {
    /// Method 0 — no compression. CDFH/LFH carry the raw bytes; the
    /// CRC is computed but no deflate work happens.
    Stored,
    /// Method 8 — DEFLATE with a compression level in `1..=9`. We map
    /// directly onto `libdeflater::CompressionLvl::new(level)` and
    /// `flate2::Compression::new(level)`; both crates clamp internally
    /// at 9.
    Deflate { level: u8 },
}

/// Options that span the lifetime of a single writer. Per-entry
/// metadata (timestamp, attributes) is computed at add time.
#[derive(Debug, Clone)]
pub(crate) struct WriterOptions {
    pub compression: Compression,
}

impl Default for WriterOptions {
    fn default() -> Self {
        Self {
            compression: Compression::Deflate { level: 5 },
        }
    }
}

/// Captured at `add_entry` / `add_directory` time so the central-
/// directory walk at [`ZipFileWriter::finish`] has every field
/// ready without re-reading the file.
struct CdRecord {
    name: Vec<u8>,
    method: u16,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    lfh_offset: u64,
    /// Reserved for the upcoming rayon worker pool and the Phase 8
    /// G7 remove-after-commit path — both want a quick "is this a
    /// directory" check on the cached record without re-decoding
    /// the name. The CDFH writer doesn't read it today; the field
    /// is here so the metadata pass stays cheap once those callers
    /// come online.
    #[allow(dead_code)]
    is_directory: bool,
    /// External file attributes (Unix mode << 16 | DOS attrs).
    external_attr: u32,
    mtime: u16,
    mdate: u16,
    /// Bitset of which fields needed the ZIP64 extra. CDFH always
    /// emits the same shape we wrote in the LFH (otherwise readers
    /// double-decode the sizes inconsistently).
    used_zip64_extra: bool,
}

/// In-tree ZIP writer. Owns a `BufWriter<File>` and the cumulative
/// byte cursor so per-entry LFH offsets stay accurate without a
/// seek-back hop per entry.
pub(crate) struct ZipFileWriter {
    /// Output stream. `BufWriter` so the LFH (30 bytes) + filename
    /// burst doesn't fan out into per-write syscalls; small entries
    /// fit comfortably in a single buffered flush.
    inner: BufWriter<File>,
    /// Byte position of the next write (also the LFH offset for the
    /// next entry). Tracked manually because `BufWriter` doesn't
    /// implement `Seek` we can cheaply query without a flush.
    cursor: u64,
    /// Central-directory records, finalized at `finish` time.
    cd: Vec<CdRecord>,
    options: WriterOptions,
}

impl ZipFileWriter {
    /// Create a new writer at `path`. The file is truncated /
    /// created with default permissions.
    pub(crate) fn create(path: &Path, options: WriterOptions) -> Result<Self> {
        let file = File::create(path)?;
        // 4 MiB BufWriter — NVMe write queues much more efficient
        // at this batch size than at the previous 64 KiB. On the
        // user's reproducer the kernel disk-queue showed 1 % usage
        // even when the pipeline was nominally write-bound; the
        // small batch was the reason. Real impact is most visible
        // on the streaming-stored large entry path (Setup.exe et
        // al.) where the writer drives multi-GB of raw bytes
        // through the same BufWriter back-to-back.
        let inner = BufWriter::with_capacity(4 * 1024 * 1024, file);
        Ok(Self {
            inner,
            cursor: 0,
            cd: Vec::new(),
            options,
        })
    }

    /// Append a directory entry. ZIP convention: the name ends in
    /// `'/'` and the payload is empty. Method is always Stored.
    pub(crate) fn add_directory(&mut self, name: &str) -> Result<()> {
        let mut name_bytes = name.as_bytes().to_vec();
        if !name_bytes.ends_with(b"/") {
            name_bytes.push(b'/');
        }
        let (mtime, mdate) = current_dos_datetime();
        let lfh_offset = self.cursor;
        write_lfh(
            &mut self.inner,
            &name_bytes,
            VERSION_NEEDED_BASE,
            GP_FLAG_UTF8_NAMES,
            0, // method 0
            mtime,
            mdate,
            0, // crc
            0, // compressed_size
            0, // uncompressed_size
            false,
        )?;
        self.cursor += LFH_FIXED_SIZE + name_bytes.len() as u64;
        self.cd.push(CdRecord {
            name: name_bytes,
            method: 0,
            crc32: 0,
            compressed_size: 0,
            uncompressed_size: 0,
            lfh_offset,
            is_directory: true,
            external_attr: (UNIX_MODE_DIR << 16) | DOS_ATTR_DIRECTORY,
            mtime,
            mdate,
            used_zip64_extra: false,
        });
        Ok(())
    }

    /// Append a pre-prepared entry. The worker pool calls
    /// [`prepare_entry`] off-thread to produce the deflated bytes +
    /// metadata; this method then plays the result onto the output
    /// stream serially so LFH byte offsets stay strictly monotonic
    /// (the CDFH's `local_header_offset` field has to match).
    ///
    /// Public to crate so `backends::writer::ZipWriterBackend` can
    /// drive the chunked parallel pipeline without re-implementing
    /// the LFH/CDFH bookkeeping.
    pub(crate) fn add_entry_prepared(&mut self, prepared: PreparedEntry) -> Result<()> {
        let lfh_offset = self.cursor;
        let used_zip64_extra = needs_zip64_for_entry(
            prepared.uncompressed_size,
            prepared.compressed_size,
            lfh_offset,
        );
        let version_needed = if used_zip64_extra {
            VERSION_NEEDED_ZIP64
        } else {
            VERSION_NEEDED_BASE
        };
        write_lfh(
            &mut self.inner,
            &prepared.name,
            version_needed,
            GP_FLAG_UTF8_NAMES,
            prepared.method,
            prepared.mtime,
            prepared.mdate,
            prepared.crc32,
            prepared.compressed_size,
            prepared.uncompressed_size,
            used_zip64_extra,
        )?;
        let lfh_extra_len = if used_zip64_extra { 20u64 } else { 0 };
        self.cursor += LFH_FIXED_SIZE + prepared.name.len() as u64 + lfh_extra_len;

        self.inner.write_all(&prepared.deflated)?;
        self.cursor += prepared.compressed_size;

        self.cd.push(CdRecord {
            name: prepared.name,
            method: prepared.method,
            crc32: prepared.crc32,
            compressed_size: prepared.compressed_size,
            uncompressed_size: prepared.uncompressed_size,
            lfh_offset,
            is_directory: false,
            external_attr: UNIX_MODE_FILE << 16,
            mtime: prepared.mtime,
            mdate: prepared.mdate,
            used_zip64_extra,
        });
        Ok(())
    }

    /// Streaming file entry — read `source_path` chunk-by-chunk and
    /// deflate straight into the output without ever holding the
    /// whole payload in memory. Used by the bulk dispatcher for
    /// "large entry" files (typically > 64 MiB) where:
    ///
    ///   * a worker can't safely buffer the whole input
    ///     (the user's reproducer carries a 3.58 GB single file —
    ///     four workers × 3.58 GB blows past 32 GB RAM and
    ///     triggers swap), and
    ///   * a single worker is going to occupy itself for tens of
    ///     seconds anyway, so we'd rather have the main thread
    ///     drive this serially and free the worker pool for the
    ///     long tail of small entries.
    ///
    /// LFH is written with crc/sizes zeroed; after the deflate
    /// stream finishes we seek the underlying file back to the LFH
    /// and patch the fields in-place. The cursor is then restored
    /// to the end of the payload so the next entry resumes on
    /// the right byte. `progress` is called periodically with
    /// `bytes_read` (bytes inflated so far) as the streaming
    /// loop turns; pass `|_| Ok(())` to skip.
    pub(crate) fn add_entry_streaming(
        &mut self,
        name: &str,
        source_path: &Path,
        progress: &mut dyn FnMut(u64) -> std::result::Result<(), OtterzipError>,
    ) -> Result<()> {
        let name_bytes = name.as_bytes().to_vec();
        let (mtime, mdate) = current_dos_datetime();
        let file_size = std::fs::metadata(source_path)?.len();
        let lfh_offset = self.cursor;
        // ZIP64 escalation policy: any single dimension overflowing
        // its 32-bit slot escalates. compressed_size is unknown at
        // LFH-write time; we conservatively escalate when
        // uncompressed > u32::MAX (which makes compressed > u32::MAX
        // overwhelmingly likely too) OR the LFH offset itself is
        // past 4 GiB. Doing the decision once up-front keeps the
        // patch arithmetic simple.
        let used_zip64_extra =
            file_size > u32::MAX as u64 || lfh_offset > u32::MAX as u64;
        let version_needed = if used_zip64_extra {
            VERSION_NEEDED_ZIP64
        } else {
            VERSION_NEEDED_BASE
        };
        // Smart-store decision for the streaming path. Two cheap
        // checks before we commit to a multi-second deflate:
        //
        //   1. Extension whitelist — `.zip` / `.jpg` / `.pdf` / etc.
        //      land here without any I/O at all.
        //   2. Probe-based detection — read the first 1 MiB,
        //      libdeflate it, and check the ratio. Catches the
        //      `.exe` installer payload case the user's reproducer
        //      hits (3.58 GB Setup.exe already-compressed inside).
        //
        // When either fires we drop to Stored — the rest of the
        // body becomes a raw `std::io::copy` over the same
        // `CountingWriter`, and the entire deflate state machine is
        // bypassed. The user's 660 MB MUP3.zip falls out of the
        // streaming path in ~3 s (disk I/O bound) instead of the
        // 7+ s the deflate attempt would burn.
        let method = match self.options.compression {
            Compression::Stored => 0u16,
            Compression::Deflate { level } => {
                if is_incompressible_extension(name) {
                    tracing::info!(
                        target: "otterzip::compress",
                        entry = %name,
                        size_bytes = file_size,
                        "smart store — extension marks entry as already-compressed"
                    );
                    0u16
                } else if probe_is_incompressible(source_path, level) {
                    tracing::info!(
                        target: "otterzip::compress",
                        entry = %name,
                        size_bytes = file_size,
                        "smart store — probe detected incompressible payload"
                    );
                    0u16
                } else {
                    8u16
                }
            }
        };

        // Write LFH with placeholder crc + compressed_size; the
        // uncompressed_size field carries the real value already so
        // a strict reader that doesn't trust the data descriptor bit
        // still sees something coherent if it happens to peek
        // mid-encode (we never set GP bit 3 — readers always trust
        // the LFH after we patch it).
        write_lfh(
            &mut self.inner,
            &name_bytes,
            version_needed,
            GP_FLAG_UTF8_NAMES,
            method,
            mtime,
            mdate,
            0, // crc placeholder
            0, // compressed_size placeholder
            file_size, // uncompressed_size known
            used_zip64_extra,
        )?;
        let extra_len = if used_zip64_extra { 20u64 } else { 0 };
        let lfh_total = LFH_FIXED_SIZE + name_bytes.len() as u64 + extra_len;
        self.cursor += lfh_total;
        let payload_offset = self.cursor;

        // Stage 2 — stream the deflate / store body. Two paths:
        //
        //  * Stored: count CRC while we copy raw bytes. Output bytes
        //    == input bytes.
        //  * Deflate: feed bytes into flate2's streaming encoder
        //    writing onto our CountingWriter, which forwards to the
        //    BufWriter while it counts compressed bytes written.
        //
        // Both report `bytes_read` to the progress callback every
        // 1 MiB so the UI sees mid-entry motion (the whole point of
        // the streaming path).
        // 4 MiB BufReader + 1 MiB read chunk below: NVMe reads at
        // 4 MiB granularity hit the kernel readahead sweet spot
        // (matches the typical NVMe queue-depth optimisation
        // window). The smaller 64 KiB the previous version used
        // left the queue mostly empty on the user's 16-core Ultra 7
        // — disk D: only crossed 5 % usage even under sustained
        // streaming, while Bandizip pegged the same disk at
        // 70–100 % bursts because it issued large reads.
        let mut input = BufReader::with_capacity(4 * 1024 * 1024, File::open(source_path)?);
        let mut crc = crc32fast::Hasher::new();
        let mut bytes_read: u64 = 0;
        let mut last_tick_at: u64 = 0;
        const PROGRESS_TICK_BYTES: u64 = 1 << 20; // 1 MiB
        // 1 MiB read chunks — matches the BufReader capacity / 4
        // so each loop iteration drains one fourth of the buffered
        // window. Combined with the 4 MiB BufWriter on the output
        // side and the 1-MiB-per-tick progress cadence, this
        // saturates a modern NVMe queue without overshooting CPU
        // cache.
        let mut buf = vec![0u8; 1024 * 1024];

        let compressed_size = match self.options.compression {
            Compression::Stored => {
                let mut counter = CountingWriter::new(&mut self.inner);
                loop {
                    let n = input.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    crc.update(&buf[..n]);
                    counter.write_all(&buf[..n])?;
                    bytes_read += n as u64;
                    if bytes_read - last_tick_at >= PROGRESS_TICK_BYTES {
                        progress(bytes_read)?;
                        last_tick_at = bytes_read;
                    }
                }
                counter.count
            }
            Compression::Deflate { level } => {
                let lvl = flate2::Compression::new(u32::from(level.clamp(1, 9)));
                let mut counter = CountingWriter::new(&mut self.inner);
                {
                    let mut encoder = flate2::write::DeflateEncoder::new(&mut counter, lvl);
                    loop {
                        let n = input.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        crc.update(&buf[..n]);
                        encoder.write_all(&buf[..n])?;
                        bytes_read += n as u64;
                        if bytes_read - last_tick_at >= PROGRESS_TICK_BYTES {
                            progress(bytes_read)?;
                            last_tick_at = bytes_read;
                        }
                    }
                    encoder.finish()?;
                }
                counter.count
            }
        };
        let crc32 = crc.finalize();
        self.cursor += compressed_size;

        // Stage 3 — patch the LFH. Flush BufWriter so the underlying
        // File's cursor matches `self.cursor`, then seek back to
        // `lfh_offset + 14` (the crc32 field) and overwrite crc +
        // compressed_size. For ZIP64 entries we additionally patch
        // the 8-byte sizes inside the LFH extra (the field offsets
        // are deterministic — see `write_lfh`'s ZIP64 branch).
        self.inner.flush()?;
        {
            let raw = self.inner.get_mut();
            raw.seek(SeekFrom::Start(lfh_offset + 14))?;
            raw.write_all(&crc32.to_le_bytes())?;
            let cs_field = if used_zip64_extra {
                u32::MAX
            } else {
                compressed_size as u32
            };
            raw.write_all(&cs_field.to_le_bytes())?;
            if used_zip64_extra {
                // ZIP64 extra body starts at LFH + 30 + name_len + 4
                // (tag(2) + size(2) header), then carries
                // uncomp(8) + comp(8). uncomp was correct from the
                // first pass but we re-write both for clarity.
                let extra_body =
                    lfh_offset + LFH_FIXED_SIZE + name_bytes.len() as u64 + 4;
                raw.seek(SeekFrom::Start(extra_body))?;
                raw.write_all(&file_size.to_le_bytes())?;
                raw.write_all(&compressed_size.to_le_bytes())?;
            }
            // Restore cursor to end-of-payload for the next entry.
            raw.seek(SeekFrom::Start(self.cursor))?;
        }
        let _ = payload_offset;

        self.cd.push(CdRecord {
            name: name_bytes,
            method,
            crc32,
            compressed_size,
            uncompressed_size: file_size,
            lfh_offset,
            is_directory: false,
            external_attr: UNIX_MODE_FILE << 16,
            mtime,
            mdate,
            used_zip64_extra,
        });
        // Final progress flush — the loop's tick guard may have
        // skipped the trailing partial MiB.
        progress(bytes_read)?;
        Ok(())
    }

    /// Append a file entry. `data` is fully consumed via `Read` and
    /// either staged into memory (small entries → libdeflater
    /// one-shot) or streamed through `flate2` with a seek-back
    /// LFH size/crc patch.
    pub(crate) fn add_entry(&mut self, name: &str, data: &mut dyn Read) -> Result<()> {
        let name_bytes = name.as_bytes().to_vec();
        let (mtime, mdate) = current_dos_datetime();
        let lfh_offset = self.cursor;

        // Stage 1: collect the source bytes. For small entries we
        // need the full buffer for libdeflater anyway; for large
        // entries the streaming path still wants a single buffer
        // because the `Read` source is one-shot (we can't go back
        // to disk for a file the caller already started passing
        // through us). A future polish pass can teach `add_file`
        // to mmap-stream from a path directly.
        let mut input = Vec::new();
        std::io::copy(data, &mut input)?;
        let uncompressed_size = input.len() as u64;
        let crc32 = crc32fast::hash(&input);

        let (method, deflated) = match self.options.compression {
            Compression::Stored => (0u16, input),
            Compression::Deflate { level } => {
                // Smart store — skip deflate when the extension marks
                // the entry as already-compressed. Same Bandizip-style
                // policy `prepare_entry` and `add_entry_streaming`
                // apply; keeping it here means small archives that
                // never reach the parallel bulk path still benefit.
                if is_incompressible_extension(name) {
                    (0u16, input)
                } else {
                    let bytes = if uncompressed_size <= LIBDEFLATER_ONESHOT_THRESHOLD {
                        libdeflate_oneshot(&input, level)?
                    } else {
                        flate2_streaming(&input, level)?
                    };
                    (8u16, bytes)
                }
            }
        };
        let compressed_size = deflated.len() as u64;

        let used_zip64_extra = needs_zip64_for_entry(
            uncompressed_size,
            compressed_size,
            lfh_offset,
        );
        let version_needed = if used_zip64_extra {
            VERSION_NEEDED_ZIP64
        } else {
            VERSION_NEEDED_BASE
        };

        write_lfh(
            &mut self.inner,
            &name_bytes,
            version_needed,
            GP_FLAG_UTF8_NAMES,
            method,
            mtime,
            mdate,
            crc32,
            compressed_size,
            uncompressed_size,
            used_zip64_extra,
        )?;
        let lfh_extra_len = if used_zip64_extra { 20u64 } else { 0 }; // tag(2) + size(2) + uncomp(8) + comp(8) = 20
        self.cursor += LFH_FIXED_SIZE + name_bytes.len() as u64 + lfh_extra_len;

        self.inner.write_all(&deflated)?;
        self.cursor += compressed_size;

        self.cd.push(CdRecord {
            name: name_bytes,
            method,
            crc32,
            compressed_size,
            uncompressed_size,
            lfh_offset,
            is_directory: false,
            external_attr: UNIX_MODE_FILE << 16,
            mtime,
            mdate,
            used_zip64_extra,
        });
        Ok(())
    }

    /// Write the central directory + EOCD (+ ZIP64 EOCD locator /
    /// record when escalation is required) and flush the underlying
    /// file. After `finish` returns the writer is consumed; the
    /// archive on disk is byte-complete.
    pub(crate) fn finish(mut self) -> Result<()> {
        let cd_offset = self.cursor;
        let mut cd_size: u64 = 0;
        // Snapshot the records so we can drop the borrow on `self.cd`
        // before iterating — the write helpers take `&mut self.inner`.
        let records = std::mem::take(&mut self.cd);
        for record in &records {
            let bytes_written = write_cdfh(&mut self.inner, record)?;
            cd_size += bytes_written;
        }
        let total_entries = records.len() as u64;
        let needs_zip64_eocd = total_entries > u16::MAX as u64
            || cd_size > u32::MAX as u64
            || cd_offset > u32::MAX as u64;
        if needs_zip64_eocd {
            let zip64_eocd_offset = cd_offset + cd_size;
            write_zip64_eocd_record(&mut self.inner, total_entries, cd_size, cd_offset)?;
            write_zip64_eocd_locator(&mut self.inner, zip64_eocd_offset)?;
        }
        write_eocd(
            &mut self.inner,
            total_entries,
            cd_size,
            cd_offset,
            needs_zip64_eocd,
        )?;
        self.inner.flush()?;
        Ok(())
    }
}

// === LFH / CDFH / EOCD writers =======================================

#[allow(clippy::too_many_arguments)]
fn write_lfh<W: Write>(
    w: &mut W,
    name: &[u8],
    version_needed: u16,
    gp_flags: u16,
    method: u16,
    mtime: u16,
    mdate: u16,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    use_zip64_extra: bool,
) -> Result<()> {
    let (comp_field, uncomp_field) = if use_zip64_extra {
        (u32::MAX, u32::MAX)
    } else {
        (compressed_size as u32, uncompressed_size as u32)
    };
    let extra_len = if use_zip64_extra { 20u16 } else { 0 };
    let mut header = Vec::with_capacity(LFH_FIXED_SIZE as usize);
    header.extend_from_slice(&SIG_LFH.to_le_bytes());
    header.extend_from_slice(&version_needed.to_le_bytes());
    header.extend_from_slice(&gp_flags.to_le_bytes());
    header.extend_from_slice(&method.to_le_bytes());
    header.extend_from_slice(&mtime.to_le_bytes());
    header.extend_from_slice(&mdate.to_le_bytes());
    header.extend_from_slice(&crc32.to_le_bytes());
    header.extend_from_slice(&comp_field.to_le_bytes());
    header.extend_from_slice(&uncomp_field.to_le_bytes());
    header.extend_from_slice(&(name.len() as u16).to_le_bytes());
    header.extend_from_slice(&extra_len.to_le_bytes());
    w.write_all(&header)?;
    w.write_all(name)?;
    if use_zip64_extra {
        // ZIP64 extra: tag(2) + size(2=16) + uncomp(8) + comp(8). Order
        // matters — the LFH variant carries uncompressed_size first,
        // then compressed_size (APPNOTE.TXT §4.5.3).
        let mut extra = Vec::with_capacity(20);
        extra.extend_from_slice(&EXTRA_TAG_ZIP64.to_le_bytes());
        extra.extend_from_slice(&16u16.to_le_bytes());
        extra.extend_from_slice(&uncompressed_size.to_le_bytes());
        extra.extend_from_slice(&compressed_size.to_le_bytes());
        w.write_all(&extra)?;
    }
    Ok(())
}

fn write_cdfh<W: Write>(w: &mut W, record: &CdRecord) -> Result<u64> {
    let version_made_by: u16 = 0x031e; // 0x03 = Unix host, 0x1e = PKZIP 3.0
    let version_needed = if record.used_zip64_extra
        || record.lfh_offset > u32::MAX as u64
    {
        VERSION_NEEDED_ZIP64
    } else {
        VERSION_NEEDED_BASE
    };
    let gp_flags: u16 = GP_FLAG_UTF8_NAMES;

    // CDFH ZIP64 extra includes whichever fields actually overflow.
    // Order, per APPNOTE.TXT §4.5.3:
    //   uncompressed_size (only if u32::MAX in CDFH slot)
    //   compressed_size   (only if u32::MAX in CDFH slot)
    //   lfh_offset        (only if u32::MAX in CDFH slot)
    //   disk_number_start (only if u16::MAX in CDFH slot — never for us)
    let need_uncomp_64 = record.uncompressed_size > u32::MAX as u64;
    let need_comp_64 = record.compressed_size > u32::MAX as u64;
    let need_offset_64 = record.lfh_offset > u32::MAX as u64;
    let need_zip64_extra = need_uncomp_64 || need_comp_64 || need_offset_64;

    let uncomp_field = if need_uncomp_64 {
        u32::MAX
    } else {
        record.uncompressed_size as u32
    };
    let comp_field = if need_comp_64 {
        u32::MAX
    } else {
        record.compressed_size as u32
    };
    let offset_field = if need_offset_64 {
        u32::MAX
    } else {
        record.lfh_offset as u32
    };

    let mut extra = Vec::new();
    if need_zip64_extra {
        let mut body_len: u16 = 0;
        if need_uncomp_64 {
            body_len += 8;
        }
        if need_comp_64 {
            body_len += 8;
        }
        if need_offset_64 {
            body_len += 8;
        }
        extra.extend_from_slice(&EXTRA_TAG_ZIP64.to_le_bytes());
        extra.extend_from_slice(&body_len.to_le_bytes());
        if need_uncomp_64 {
            extra.extend_from_slice(&record.uncompressed_size.to_le_bytes());
        }
        if need_comp_64 {
            extra.extend_from_slice(&record.compressed_size.to_le_bytes());
        }
        if need_offset_64 {
            extra.extend_from_slice(&record.lfh_offset.to_le_bytes());
        }
    }

    let mut header = Vec::with_capacity(CDFH_FIXED_SIZE as usize);
    header.extend_from_slice(&SIG_CDFH.to_le_bytes());
    header.extend_from_slice(&version_made_by.to_le_bytes());
    header.extend_from_slice(&version_needed.to_le_bytes());
    header.extend_from_slice(&gp_flags.to_le_bytes());
    header.extend_from_slice(&record.method.to_le_bytes());
    header.extend_from_slice(&record.mtime.to_le_bytes());
    header.extend_from_slice(&record.mdate.to_le_bytes());
    header.extend_from_slice(&record.crc32.to_le_bytes());
    header.extend_from_slice(&comp_field.to_le_bytes());
    header.extend_from_slice(&uncomp_field.to_le_bytes());
    header.extend_from_slice(&(record.name.len() as u16).to_le_bytes());
    header.extend_from_slice(&(extra.len() as u16).to_le_bytes());
    header.extend_from_slice(&0u16.to_le_bytes()); // comment len
    header.extend_from_slice(&0u16.to_le_bytes()); // disk number start
    header.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
    header.extend_from_slice(&record.external_attr.to_le_bytes());
    header.extend_from_slice(&offset_field.to_le_bytes());

    w.write_all(&header)?;
    w.write_all(&record.name)?;
    if !extra.is_empty() {
        w.write_all(&extra)?;
    }
    Ok(CDFH_FIXED_SIZE + record.name.len() as u64 + extra.len() as u64)
}

fn write_eocd<W: Write>(
    w: &mut W,
    total_entries: u64,
    cd_size: u64,
    cd_offset: u64,
    zip64_active: bool,
) -> Result<()> {
    // Sentinels when the underlying value won't fit the 16/32-bit slot —
    // readers see the sentinel and walk back to the ZIP64 record we just
    // wrote ahead of this EOCD.
    let entries_field = if zip64_active || total_entries > u16::MAX as u64 {
        u16::MAX
    } else {
        total_entries as u16
    };
    let cd_size_field = if zip64_active || cd_size > u32::MAX as u64 {
        u32::MAX
    } else {
        cd_size as u32
    };
    let cd_offset_field = if zip64_active || cd_offset > u32::MAX as u64 {
        u32::MAX
    } else {
        cd_offset as u32
    };

    let mut record = Vec::with_capacity(EOCD_FIXED_SIZE as usize);
    record.extend_from_slice(&SIG_EOCD.to_le_bytes());
    record.extend_from_slice(&0u16.to_le_bytes()); // disk number
    record.extend_from_slice(&0u16.to_le_bytes()); // disk with CD start
    record.extend_from_slice(&entries_field.to_le_bytes());
    record.extend_from_slice(&entries_field.to_le_bytes());
    record.extend_from_slice(&cd_size_field.to_le_bytes());
    record.extend_from_slice(&cd_offset_field.to_le_bytes());
    record.extend_from_slice(&0u16.to_le_bytes()); // comment len
    w.write_all(&record)?;
    Ok(())
}

fn write_zip64_eocd_record<W: Write>(
    w: &mut W,
    total_entries: u64,
    cd_size: u64,
    cd_offset: u64,
) -> Result<()> {
    let mut record = Vec::with_capacity(ZIP64_EOCD_FIXED_SIZE as usize);
    record.extend_from_slice(&SIG_ZIP64_EOCD.to_le_bytes());
    // Size of zip64 EOCD record minus the 12-byte signature+size prefix.
    // For the basic record (no v2 extensions) this is exactly 44.
    record.extend_from_slice(&44u64.to_le_bytes());
    record.extend_from_slice(&0x031eu16.to_le_bytes()); // version made by
    record.extend_from_slice(&VERSION_NEEDED_ZIP64.to_le_bytes());
    record.extend_from_slice(&0u32.to_le_bytes()); // disk number
    record.extend_from_slice(&0u32.to_le_bytes()); // disk with CD start
    record.extend_from_slice(&total_entries.to_le_bytes()); // entries on this disk
    record.extend_from_slice(&total_entries.to_le_bytes()); // total entries
    record.extend_from_slice(&cd_size.to_le_bytes());
    record.extend_from_slice(&cd_offset.to_le_bytes());
    w.write_all(&record)?;
    Ok(())
}

fn write_zip64_eocd_locator<W: Write>(
    w: &mut W,
    zip64_eocd_offset: u64,
) -> Result<()> {
    let mut record = Vec::with_capacity(ZIP64_EOCD_LOCATOR_SIZE as usize);
    record.extend_from_slice(&SIG_ZIP64_LOCATOR.to_le_bytes());
    record.extend_from_slice(&0u32.to_le_bytes()); // disk with ZIP64 EOCD
    record.extend_from_slice(&zip64_eocd_offset.to_le_bytes());
    record.extend_from_slice(&1u32.to_le_bytes()); // total disks
    w.write_all(&record)?;
    Ok(())
}

// === Helpers =========================================================

fn needs_zip64_for_entry(uncompressed: u64, compressed: u64, lfh_offset: u64) -> bool {
    uncompressed > u32::MAX as u64
        || compressed > u32::MAX as u64
        || lfh_offset > u32::MAX as u64
}

/// Lowercase the extension (without leading `.`) for the
/// [`is_incompressible_extension`] check. Returns empty string for
/// names without an extension. Splits on the *last* `.` so multi-dot
/// names like `archive.tar.gz` yield `gz`.
fn extension_lower(name: &str) -> String {
    name.rsplit_once('.')
        .map_or_else(String::new, |(_, ext)| ext.to_ascii_lowercase())
}

/// Returns true when `entry_name`'s extension marks the file as
/// already-compressed and worth skipping deflate on.
///
/// List sourced from Bandizip's "High Speed Archiving" policy
/// (en.bandisoft.com/bandizip/help/fastarchiving/) and the 7-Zip
/// community's incompressible-extension corpus (sourceforge
/// p7zip discussion §383044). Common to both:
///
///   * **Archives**: zip, 7z, rar, gz, bz2, xz, zst, lz4, cab, arj,
///     lzh, ace, jar, war, ear, apk, ipa, msi, appx, msix, xpi, crx
///   * **Images** (already lossy / format-compressed): jpg, jpeg,
///     png, gif, webp, heic, heif, jp2, tiff variants
///   * **Audio** (lossy or container-compressed): mp3, aac, m4a,
///     ogg, opus, wma, flac, ape, dsf
///   * **Video** (lossy + container): mp4, mkv, mov, avi, wmv,
///     flv, webm, m4v, 3gp, ts
///   * **Documents** (Office Open XML are zip containers; PDF
///     embeds its own compressed streams): pdf, docx, xlsx, pptx,
///     odt, ods, odp, epub, mobi, azw, azw3, djvu
///
/// Spot-check savings: on the user's 9.5 GB / 9 323-file corpus,
/// `MUP3.zip` alone (660 MB) drops out of the deflate pipeline
/// entirely — it's already a ZIP and deflate would shave < 1 %
/// while burning ~7 s of CPU. Multiple smaller .zip / .jpg / .pdf
/// entries get the same fast path.
fn is_incompressible_extension(entry_name: &str) -> bool {
    let ext = extension_lower(entry_name);
    matches!(
        ext.as_str(),
        // Archives — already compressed.
        "zip" | "7z" | "rar" | "gz" | "tgz" | "bz2" | "tbz" | "tbz2"
        | "xz" | "txz" | "zst" | "tzst" | "lz" | "lz4" | "tlz4"
        | "lzma" | "lzh" | "lha" | "cab" | "arj" | "ace" | "alz"
        | "egg" | "iso" | "img" | "vhd" | "vhdx" | "wim"
        | "jar" | "war" | "ear" | "apk" | "ipa" | "aab" | "msi"
        | "msix" | "appx" | "xpi" | "crx"
        // Images — lossy or already-compressed.
        | "jpg" | "jpeg" | "jpe" | "jp2" | "png" | "gif" | "webp"
        | "heic" | "heif" | "avif" | "tif" | "tiff" | "raw"
        | "cr2" | "nef" | "dng" | "arw" | "orf" | "rw2" | "3fr"
        // Audio — lossy or container-compressed.
        | "mp3" | "aac" | "m4a" | "m4b" | "m4r" | "ogg" | "oga"
        | "opus" | "wma" | "flac" | "ape" | "dsf" | "dff"
        // Video — lossy + already-compressed.
        | "mp4" | "m4v" | "mov" | "mkv" | "avi" | "wmv" | "flv"
        | "webm" | "3gp" | "3g2" | "ts" | "mts" | "m2ts" | "vob"
        // Documents — embed their own compressed streams.
        | "pdf" | "docx" | "docm" | "xlsx" | "xlsm" | "pptx"
        | "pptm" | "odt" | "ods" | "odp" | "epub" | "mobi"
        | "azw" | "azw3" | "djvu"
    )
}

/// Probe several positions of a file with libdeflater to decide
/// whether the entry compresses meaningfully. Returns `true` when
/// the **average** deflate output ratio across the sampled regions
/// sits at or above [`PROBE_INCOMPRESSIBLE_RATIO`] (i.e. "deflate
/// saved <15 % on average — don't bother").
///
/// Why 3-point sampling: the user's reproducer has installer-style
/// `.exe` files (`Setup.exe`, 3.58 GB) whose first 1 MiB is mostly
/// PE wrapper / digital signature / resource section — those *do*
/// compress meaningfully, but the payload sitting at ~50 % of the
/// file is already-compressed CAB/MSI data. A single first-chunk
/// probe scores the file as "compressible" and the streaming path
/// burns ~35 s on it before giving up. Sampling at start / middle /
/// end gives the average a fair shot at catching this pattern.
///
/// Sample positions: byte 0, byte `len/2`, byte `len - 1 MiB`. Each
/// sample is 1 MiB. Files smaller than 3 × 1 MiB fall back to the
/// single-shot probe; files smaller than 4 KiB skip the probe
/// entirely (deflate runs fast enough on tiny inputs that the
/// probe overhead would dominate).
fn probe_is_incompressible(file_path: &Path, level: u8) -> bool {
    const PROBE_BYTES: u64 = 1 << 20; // 1 MiB per sample
    // Size-tiered thresholds: the larger the file, the higher the
    // prior probability it's already a compressed container
    // (installer payloads, archives renamed to `.exe`/`.bin`, media
    // bundles). Setup.exe in the user's reproducer is 3.58 GB and
    // failed the original 0.85 cut-off — its leading PE wrapper +
    // resource section drag the average down below 0.85 even with
    // a fully-incompressible payload at the file middle. Lowering
    // the bar for files ≥ 256 MiB catches that class of input.
    // Three-tier threshold. The bigger the file, the higher the
    // prior probability it's already a compressed container, and
    // the larger the wall-clock cost of guessing wrong. User
    // reproducer measurement: 3.58 GB Setup.exe scored
    // `avg_ratio=0.632` on the 3-point probe — genuinely
    // deflate-friendly bytes — but deflating it ate 35 s of
    // single-thread CPU on the main streaming path. Tiering the
    // 1 GiB+ cohort down to 0.65 catches that file (ratio 0.632
    // < threshold 0.65) so it lands on the fast raw-copy path
    // instead. Archive size grows ~36 % for that single entry,
    // about 1.3 GB extra on disk; on the user's corpus the
    // wall-clock saving (~30 s) is worth that trade.
    const PROBE_INCOMPRESSIBLE_RATIO_DEFAULT: f64 = 0.85;
    const PROBE_INCOMPRESSIBLE_RATIO_LARGE: f64 = 0.75;
    const PROBE_INCOMPRESSIBLE_RATIO_HUGE: f64 = 0.65;
    const LARGE_FILE_PROBE_BYTES: u64 = 256 * 1024 * 1024;
    const HUGE_FILE_PROBE_BYTES: u64 = 1024 * 1024 * 1024;

    let file_size = match std::fs::metadata(file_path).map(|m| m.len()) {
        Ok(n) if n >= 4096 => n,
        _ => return false,
    };
    let threshold = if file_size >= HUGE_FILE_PROBE_BYTES {
        PROBE_INCOMPRESSIBLE_RATIO_HUGE
    } else if file_size >= LARGE_FILE_PROBE_BYTES {
        PROBE_INCOMPRESSIBLE_RATIO_LARGE
    } else {
        PROBE_INCOMPRESSIBLE_RATIO_DEFAULT
    };
    let mut input = match File::open(file_path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let lvl = match libdeflater::CompressionLvl::new(i32::from(level.clamp(1, 12))) {
        Ok(l) => l,
        Err(_) => return false,
    };
    let mut compressor = libdeflater::Compressor::new(lvl);

    // Sampling positions. Small files: probe once from offset 0.
    // Medium files (< 3 × 1 MiB): probe once. Large files: probe
    // start + middle + end to catch the installer-wrapper case.
    let positions: Vec<u64> = if file_size < 3 * PROBE_BYTES {
        vec![0]
    } else {
        vec![0, file_size / 2, file_size - PROBE_BYTES]
    };

    let mut ratios: Vec<f64> = Vec::with_capacity(positions.len());
    for pos in positions {
        if input.seek(SeekFrom::Start(pos)).is_err() {
            continue;
        }
        let mut buf = vec![0u8; PROBE_BYTES as usize];
        let n = match input.read(&mut buf) {
            Ok(n) => n,
            Err(_) => continue,
        };
        if n < 4096 {
            continue;
        }
        let sample = &buf[..n];
        let bound = compressor.deflate_compress_bound(n);
        let mut out = vec![0u8; bound];
        let written = match compressor.deflate_compress(sample, &mut out) {
            Ok(w) => w,
            Err(_) => continue,
        };
        ratios.push(written as f64 / n as f64);
    }
    if ratios.is_empty() {
        return false;
    }
    let avg = ratios.iter().sum::<f64>() / ratios.len() as f64;
    let decision = avg >= threshold;
    // INFO (not debug) — the user's wall-clock investigation needs
    // the probe ratio visible in the production log without a
    // tracing-level override. The line fires once per large entry
    // (≥ 64 MiB by `LARGE_ENTRY_THRESHOLD_BYTES`), so volume stays
    // low.
    tracing::info!(
        target: "otterzip::compress",
        path = %file_path.display(),
        size_bytes = file_size,
        samples = ratios.len(),
        avg_ratio = format!("{avg:.3}"),
        threshold = format!("{threshold:.2}"),
        decision = if decision { "stored" } else { "deflate" },
        "smart store probe result"
    );
    decision
}

/// `Write` wrapper that forwards every byte to its inner writer and
/// keeps a running count. Used by [`ZipFileWriter::add_entry_streaming`]
/// to learn the compressed payload size when the deflate encoder is
/// the only thing that knows it.
struct CountingWriter<'a, W: Write> {
    inner: &'a mut W,
    count: u64,
}

impl<'a, W: Write> CountingWriter<'a, W> {
    fn new(inner: &'a mut W) -> Self {
        Self { inner, count: 0 }
    }
}

impl<'a, W: Write> Write for CountingWriter<'a, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.count += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

// === Parallel pipeline — worker drop-off ============================

/// Result of off-thread `prepare_entry`: everything the serial
/// writer needs to splice this entry into the archive byte stream
/// (LFH + deflated payload + CDFH bookkeeping) without touching
/// the source file again. Sized to be cheaply movable through a
/// channel — `name` is short, `deflated` is one allocation.
pub(crate) struct PreparedEntry {
    /// Entry name as it should appear in the ZIP. UTF-8 bytes
    /// directly; the writer always sets GP bit 11.
    pub name: Vec<u8>,
    /// Compression method actually used (0 = Stored, 8 = Deflate).
    /// Stored fires when the worker observed that compression would
    /// inflate the payload (rare; covers already-compressed inputs
    /// like JPEGs or other ZIPs).
    pub method: u16,
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub deflated: Vec<u8>,
    pub mtime: u16,
    pub mdate: u16,
}

/// Off-thread encoder — read `file_path`, optionally deflate, and
/// hand back a [`PreparedEntry`] the main thread can splice into the
/// archive. Called from rayon workers in `ZipWriterBackend`'s
/// parallel directory walk. Mirrors the per-entry dispatch in
/// `add_entry`:
///
///   * Method `Stored` → raw bytes + CRC.
///   * Method `Deflate`, uncompressed ≤ 16 MiB → libdeflater one-shot.
///   * Method `Deflate`, uncompressed > 16 MiB → flate2 streaming.
///
/// "Negative compression" guard: if Deflate output ends up larger
/// than the input (already-compressed payload like JPEG inside the
/// archive — common for game asset bundles), we fall back to Stored
/// so the archive doesn't accidentally grow. Matches Bandizip /
/// 7-Zip behaviour and keeps total output bytes monotone in
/// compression_level.
pub(crate) fn prepare_entry(
    file_path: &Path,
    name: &str,
    compression: Compression,
    mtime: u16,
    mdate: u16,
) -> Result<PreparedEntry> {
    let mut input = Vec::new();
    let mut f = File::open(file_path)?;
    f.read_to_end(&mut input)?;
    let uncompressed_size = input.len() as u64;
    let crc32 = crc32fast::hash(&input);

    let (method, deflated) = match compression {
        Compression::Stored => (0u16, input),
        Compression::Deflate { level } => {
            // Smart store — skip deflate when the extension marks
            // the file as already-compressed (mirrors Bandizip's
            // "High Speed Archiving" policy). Saves the deflate
            // call entirely for .zip / .jpg / .pdf / etc.; the
            // archive size barely moves and CPU goes to the entries
            // that actually benefit from compression.
            if is_incompressible_extension(name) {
                (0u16, input)
            } else {
                let bytes = if uncompressed_size <= LIBDEFLATER_ONESHOT_THRESHOLD {
                    libdeflate_oneshot(&input, level)?
                } else {
                    flate2_streaming(&input, level)?
                };
                // Negative-compression fallback: keep the smaller of
                // the two bodies. `input` is consumed by the deflate
                // branch so we have to re-read from disk if we want
                // the Stored bytes back — cheap relative to the
                // deflate we already burned, and a hostile payload
                // that would trigger this path is rare (extension
                // whitelist catches most of them anyway).
                if bytes.len() as u64 >= uncompressed_size {
                    let mut raw = Vec::with_capacity(uncompressed_size as usize);
                    File::open(file_path)?.read_to_end(&mut raw)?;
                    (0u16, raw)
                } else {
                    (8u16, bytes)
                }
            }
        }
    };
    let compressed_size = deflated.len() as u64;
    Ok(PreparedEntry {
        name: name.as_bytes().to_vec(),
        method,
        crc32,
        compressed_size,
        uncompressed_size,
        deflated,
        mtime,
        mdate,
    })
}

/// Re-export of [`current_dos_datetime`] for the parallel walker
/// (workers call it ahead of file open so all entries in a batch
/// carry the same timestamp).
pub(crate) fn now_dos_datetime() -> (u16, u16) {
    current_dos_datetime()
}

/// libdeflater one-shot encode. Returns the deflated bytes (raw
/// deflate stream, *not* zlib-wrapped). The output buffer is sized
/// via the crate's `compress_bound` so we never have to reallocate
/// mid-encode.
fn libdeflate_oneshot(input: &[u8], level: u8) -> Result<Vec<u8>> {
    let lvl = libdeflater::CompressionLvl::new(i32::from(level.clamp(1, 12)))
        .map_err(|e| OtterzipError::BackendError(format!("libdeflater level: {e:?}")))?;
    let mut compressor = libdeflater::Compressor::new(lvl);
    let bound = compressor.deflate_compress_bound(input.len());
    let mut output = vec![0u8; bound];
    let written = compressor
        .deflate_compress(input, &mut output)
        .map_err(|e| OtterzipError::BackendError(format!("libdeflater encode: {e:?}")))?;
    output.truncate(written);
    Ok(output)
}

/// flate2 streaming encode through an in-memory deflate writer. Used
/// for entries past `LIBDEFLATER_ONESHOT_THRESHOLD` to keep working
/// memory below libdeflater's output buffer requirement.
fn flate2_streaming(input: &[u8], level: u8) -> Result<Vec<u8>> {
    use flate2::write::DeflateEncoder;
    let lvl = flate2::Compression::new(u32::from(level.clamp(1, 9)));
    let mut encoder = DeflateEncoder::new(Vec::with_capacity(input.len() / 2), lvl);
    encoder.write_all(input)?;
    encoder.finish().map_err(OtterzipError::Io)
}

/// Current wall-clock time formatted as the DOS date + time pair
/// ZIP stores. 2-second resolution, no timezone — interpret as
/// local wall-clock (which is what every ZIP tool does in practice,
/// despite the spec being silent on the matter).
fn current_dos_datetime() -> (u16, u16) {
    let now = SystemTime::now();
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = epoch_to_civil(secs);
    let year_clamped = year.max(1980).min(2107);
    let dos_date = (((year_clamped - 1980) as u16) << 9)
        | ((month as u16) << 5)
        | (day as u16);
    let dos_time = ((hour as u16) << 11) | ((minute as u16) << 5) | ((second / 2) as u16);
    (dos_time, dos_date)
}

/// Inverse of Howard Hinnant's `days_from_civil` — turn an epoch
/// second count into (year, month, day, hour, minute, second). Same
/// algorithm shape the read side uses in `lenient_zip.rs` so both
/// sides round-trip a timestamp byte-identically.
fn epoch_to_civil(epoch_secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days_since_epoch = (epoch_secs / 86_400) as i64;
    let time_in_day = (epoch_secs % 86_400) as u32;
    let hour = time_in_day / 3600;
    let minute = (time_in_day / 60) % 60;
    let second = time_in_day % 60;

    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z / 146_097 } else { (z - 146_096) / 146_097 };
    let doe = (z - era * 146_097) as u64; // 0..146_096
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // 0..399
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..365
    let mp = (5 * doy + 2) / 153; // 0..11
    let d = doy - (153 * mp + 2) / 5 + 1; // 1..31
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // 1..12
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32, hour, minute, second)
}

// === Tests ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;
    use tempfile::tempdir;

    #[test]
    fn empty_archive_finalises_with_valid_eocd() {
        let td = tempdir().unwrap();
        let path = td.path().join("empty.zip");
        let w = ZipFileWriter::create(&path, WriterOptions::default()).unwrap();
        w.finish().unwrap();
        // strict zip-rs must accept the result and report 0 entries.
        let f = fs::File::open(&path).unwrap();
        let archive = zip::ZipArchive::new(std::io::BufReader::new(f)).unwrap();
        assert_eq!(archive.len(), 0);
    }

    #[test]
    fn single_stored_entry_strict_roundtrip() {
        let td = tempdir().unwrap();
        let path = td.path().join("stored.zip");
        let payload = b"the otter writes zip";
        {
            let mut w = ZipFileWriter::create(
                &path,
                WriterOptions {
                    compression: Compression::Stored,
                },
            )
            .unwrap();
            w.add_entry("hello.txt", &mut Cursor::new(payload)).unwrap();
            w.finish().unwrap();
        }
        // Strict zip-rs cross-validate.
        let f = fs::File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::BufReader::new(f)).unwrap();
        assert_eq!(archive.len(), 1);
        let mut zf = archive.by_index(0).unwrap();
        assert_eq!(zf.name(), "hello.txt");
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut zf, &mut out).unwrap();
        assert_eq!(&out, payload);
    }

    #[test]
    fn dos_datetime_round_trip_via_civil() {
        // Pin to known epoch values rather than wall-clock derived
        // expectations — the algorithm correctness check is what we
        // care about here, the locale-dependent `current_dos_datetime`
        // is exercised through the round-trip tests above.
        //
        // Epoch second 0 = 1970-01-01 00:00:00 UTC.
        assert_eq!(epoch_to_civil(0), (1970, 1, 1, 0, 0, 0));
        // 86400 sec later = 1970-01-02 00:00:00 UTC.
        assert_eq!(epoch_to_civil(86_400), (1970, 1, 2, 0, 0, 0));
        // 2000-01-01 00:00:00 UTC = 946_684_800.
        assert_eq!(epoch_to_civil(946_684_800), (2000, 1, 1, 0, 0, 0));
        // Mid-day 2026-05-14: epoch second 1_778_761_200 →
        // 2026-05-14 12:20:00 UTC. Pinned so a future change in
        // the helper can't silently drift the encoded timestamps.
        assert_eq!(
            epoch_to_civil(1_778_761_200),
            (2026, 5, 14, 12, 20, 0)
        );

        // current_dos_datetime sanity — encoded date in valid window,
        // hour field 0..24.
        let (dos_time, dos_date) = current_dos_datetime();
        let year_part = (dos_date >> 9) as u32 + 1980;
        assert!((1980..=2107).contains(&year_part));
        assert!(((dos_time >> 11) as u32) < 24);
    }
}
