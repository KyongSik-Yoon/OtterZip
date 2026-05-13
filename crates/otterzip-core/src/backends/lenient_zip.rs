//! Lenient ZIP backend — custom EOCD / Central Directory parser that
//! salvages malformed archives the strict `zip` crate refuses.
//!
//! ## Status — Day 1 (commit handover from `9777acd`)
//!
//! This module currently implements **only** the metadata path:
//! locating the EOCD (with ZIP64 escalation), walking the central
//! directory, and producing one `Entry` per CDFH record. The per-entry
//! LFH parse + decompression dispatch lands on Day 2; until then
//! [`LenientZipBackend::extract_entry`] / [`open_entry_stream`]
//! short-circuit to [`OtterzipError::FeatureDisabled`].
//!
//! ## Why a custom parser
//!
//! Real-world archives drift from APPNOTE.TXT §4 in well-understood
//! ways: ZIP64 cd_size off by a few hundred bytes, truncated tails,
//! filename / extra lengths that don't sum to the next record. The
//! strict `zip` crate (correctly) refuses these; we used to hand them
//! off to `libarchive`, which works but costs us a vcpkg dependency
//! and ~25 % vs Bandizip on a 7 GB malformed corpus
//! (`docs/05-build/lenient-zip-parser-plan.md` §"What we measured").
//!
//! The lenient path mirrors what Bandizip / 7-Zip do:
//!   * Clamp CD bounds to the file size when the EOCD overshoots.
//!   * Resync to the next `PK\001\002` signature when an individual
//!     CDFH's filename / extra length is bogus.
//!   * Skip entries whose `local_header_offset` lands past EOF, log
//!     a warning, keep going.
//!
//! Day 2 will reuse `flate2` (zlib-ng backed) + `libdeflater` for the
//! actual byte decompression, so we never own deflate / lzma /
//! bzip2 codec correctness — those have been fuzz-hardened by their
//! upstreams for 20+ years and any silent data corruption would slip
//! past our Sentry hook. Parser bugs always surface as `Err` /
//! panic, which Sentry does catch.

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zeroize::Zeroizing;

use crate::backends::ArchiveBackend;
use crate::entry::{Entry, HostOs};
use crate::error::{OtterzipError, Result};
use crate::format::{CompressionMethod, EncryptionMethod};

// === Format constants ================================================

/// End-of-central-directory record signature ("PK\005\006").
const SIG_EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
/// ZIP64 end-of-central-directory locator signature ("PK\006\007").
const SIG_ZIP64_LOCATOR: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
/// ZIP64 end-of-central-directory record signature ("PK\006\006").
const SIG_ZIP64_EOCD: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
/// Central-directory file header signature ("PK\001\002").
const SIG_CDFH: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];

/// Fixed-size portion of an EOCD record (variable-length comment follows).
const EOCD_FIXED_SIZE: usize = 22;
/// ZIP64 EOCD locator — always exactly 20 bytes.
const ZIP64_LOCATOR_SIZE: usize = 20;
/// Minimum size of a ZIP64 EOCD record (extensible "v2" records grow it).
const ZIP64_EOCD_FIXED_SIZE: usize = 56;
/// Fixed-size portion of a CDFH (filename / extra / comment follow).
const CDFH_FIXED_SIZE: usize = 46;

/// EOCD comment field is bounded to 65 535 bytes by APPNOTE.TXT §4.3.16,
/// plus the 22-byte fixed record = 65 557. Bandizip / 7-Zip cap their
/// EOCD search at the same value; matching them avoids "you find it but
/// we don't" complaints on archives with maxed-out tails.
const MAX_EOCD_TAIL_SCAN: u64 = 65_557;

/// ZIP64 extra field tag (APPNOTE.TXT §4.5.3).
const EXTRA_TAG_ZIP64: u16 = 0x0001;

// === Backend ==========================================================

/// Per-entry metadata captured from the central directory. Day 1 only
/// uses the public [`Entry`] half; the sidecar struct carries the LFH
/// offset + payload sizing Day 2 will need to dispatch decompression
/// without re-parsing the CD.
struct CdRecord {
    entry: Entry,
    /// Absolute byte offset of this entry's local file header. May be
    /// `u64::MAX` to mean "lenient walk decided this entry is
    /// unrecoverable" — Day 2's extract path will surface those as
    /// per-entry errors instead of failing the whole archive.
    lfh_offset: u64,
    /// Compression method straight from the CDFH (APPNOTE.TXT §4.4.5).
    /// Day 2 dispatches on this to pick `flate2` vs `libdeflater` vs
    /// `bzip2` etc.
    raw_method: u16,
    /// Raw GP bit flag — needed for encryption detection (bit 0) and
    /// the data-descriptor case Day 2 must handle (bit 3).
    raw_gpf: u16,
}

/// Lenient ZIP backend. Holds the source path + the materialised CD
/// records; per-entry payload reads (Day 2) re-open the file each call
/// because the metadata path doesn't need a sticky `RefCell<File>`.
pub(crate) struct LenientZipBackend {
    /// Original archive path. Day 2's worker re-open path goes through
    /// this.
    _path: PathBuf,
    /// Held in zeroized memory so the bytes are wiped on drop. Reserved
    /// for Day 2's encrypted-entry dispatch; Day 1 never touches it.
    _password: Option<Zeroizing<String>>,
    /// CD records produced by [`walk_central_directory`]. Held behind a
    /// `RefCell` to satisfy the [`ArchiveBackend`] trait's `&self`
    /// methods on [`entries`](ArchiveBackend::entries) without forcing
    /// the caller to take a mutable borrow.
    records: RefCell<Vec<CdRecord>>,
    /// Captured at open for diagnostic + Day-2 sanity checks. Total
    /// physical bytes on disk.
    _file_size: u64,
}

impl LenientZipBackend {
    /// Open `path` and produce a backend with every CDFH already parsed.
    /// Returns [`OtterzipError::Corrupted`] when even the lenient walk
    /// can't find an EOCD or any CDFH at all — the caller's dispatcher
    /// has nothing left to fall back to in that case.
    pub(crate) fn open(path: &Path, password: Option<&Zeroizing<String>>) -> Result<Self> {
        let started = std::time::Instant::now();
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();
        let mut reader = BufReader::with_capacity(64 * 1024, file);

        let cdr = find_central_directory(&mut reader, file_size)?;
        tracing::info!(
            target: "otterzip::lenient",
            path = %path.display(),
            cd_offset = cdr.cd_offset,
            cd_size = cdr.cd_size,
            declared_entries = cdr.total_entries,
            elapsed_us = started.elapsed().as_micros() as u64,
            "lenient: located central directory"
        );

        let records = walk_central_directory(&mut reader, &cdr, file_size)?;
        tracing::info!(
            target: "otterzip::lenient",
            path = %path.display(),
            recovered_entries = records.len(),
            declared_entries = cdr.total_entries,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "lenient: central directory walk complete"
        );

        Ok(Self {
            _path: path.to_path_buf(),
            _password: password.cloned(),
            records: RefCell::new(records),
            _file_size: file_size,
        })
    }
}

impl ArchiveBackend for LenientZipBackend {
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>> {
        // Materialise an owned snapshot so the returned iterator does
        // not borrow `self.records`; the strict backend does the same
        // for the same reason (see `backends/zip.rs::entries`).
        let cloned: Vec<Result<Entry>> = self
            .records
            .borrow()
            .iter()
            .map(|r| Ok(r.entry.clone()))
            .collect();
        Ok(Box::new(cloned.into_iter()))
    }

    fn extract_entry(&self, _entry_path: &str, _out: &mut dyn std::io::Write) -> Result<u64> {
        // Day 2 implements the per-entry LFH parse + decompression
        // dispatch. Until then this surface is gated so the dispatcher
        // can still produce a clean error rather than silently writing
        // zero bytes.
        Err(OtterzipError::FeatureDisabled(
            "lenient ZIP extract (Day 2 work — only metadata is wired today)",
        ))
    }

    fn open_entry_stream(&self, _entry_path: &str) -> Result<Box<dyn Read + Send + '_>> {
        Err(OtterzipError::FeatureDisabled(
            "lenient ZIP stream (Day 2 work — only metadata is wired today)",
        ))
    }

    fn is_encrypted_fast(&self) -> Result<bool> {
        // Cheap: just walk the cached records. No LFH scan, no
        // password trial — match the strict backend's behaviour.
        Ok(self
            .records
            .borrow()
            .iter()
            .any(|r| r.entry.encryption != EncryptionMethod::None))
    }
}

// === EOCD / ZIP64 location ===========================================

/// Resolved central-directory geometry produced by [`find_central_directory`].
/// `cd_size` may have been clamped lenient-style — the value is what we
/// will actually read, not the declared one.
struct CdResolved {
    cd_offset: u64,
    cd_size: u64,
    total_entries: u64,
}

/// Locate the EOCD (escalating to ZIP64 when sentinels demand it) and
/// derive the central-directory geometry. Lenient: out-of-range
/// `cd_size` is clamped to the gap between `cd_offset` and the EOCD
/// signature rather than rejected — most real-world malformations of
/// this kind have a few extra bytes claimed past EOF and the actual CD
/// itself is intact.
fn find_central_directory<R: Read + Seek>(
    reader: &mut R,
    file_size: u64,
) -> Result<CdResolved> {
    if file_size < EOCD_FIXED_SIZE as u64 {
        return Err(OtterzipError::Corrupted {
            reason: format!("file too small ({file_size} bytes) for EOCD"),
            entry: None,
        });
    }

    // Read the last (up to) 65 557 bytes and scan backwards for the
    // EOCD signature. APPNOTE caps the comment at 65 535 bytes; we add
    // the 22-byte fixed record for the upper bound.
    let tail_len = file_size.min(MAX_EOCD_TAIL_SCAN);
    let tail_start = file_size - tail_len;
    reader.seek(SeekFrom::Start(tail_start))?;
    let mut tail = vec![0u8; tail_len as usize];
    reader.read_exact(&mut tail)?;

    let eocd_in_tail = tail
        .windows(4)
        .rposition(|w| w == SIG_EOCD)
        .ok_or_else(|| OtterzipError::Corrupted {
            reason: format!(
                "EOCD signature not found in last {tail_len} bytes",
            ),
            entry: None,
        })?;
    if eocd_in_tail + EOCD_FIXED_SIZE > tail.len() {
        return Err(OtterzipError::Corrupted {
            reason: "EOCD record truncated".into(),
            entry: None,
        });
    }
    let eocd_abs = tail_start + eocd_in_tail as u64;
    let eocd = &tail[eocd_in_tail..eocd_in_tail + EOCD_FIXED_SIZE];
    let total_entries_16 = u16::from_le_bytes([eocd[10], eocd[11]]);
    let cd_size_32 = u32::from_le_bytes([eocd[12], eocd[13], eocd[14], eocd[15]]);
    let cd_offset_32 = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]);

    let needs_zip64 = cd_offset_32 == u32::MAX
        || cd_size_32 == u32::MAX
        || total_entries_16 == u16::MAX;
    if needs_zip64 {
        return resolve_zip64(
            reader,
            &tail,
            tail_start,
            eocd_in_tail,
            eocd_abs,
            file_size,
            ResolvedFallback {
                cd_offset_32,
                total_entries_16,
            },
        );
    }

    let declared_offset = u64::from(cd_offset_32);
    let declared_size = u64::from(cd_size_32);

    // Validate the declared offset. Two failure modes the user actually
    // ships archives for:
    //   1. `cd_offset >= eocd_abs` — pointer past the EOCD (the
    //      common "off-by-N ZIP64 cd_size arithmetic" malformation).
    //   2. `cd_offset` lands inside the file but the bytes there
    //      aren't a CDFH signature — pointer fell into the LFH region
    //      or random padding.
    // In both cases the actual central directory is still intact in
    // the file; we just need to find where it really starts. Scan
    // forward from the start of the file for the first
    // `PK\001\002` and use that as the CD origin.
    let (cd_offset, cd_size) = if declared_offset >= eocd_abs
        || !peek_is_cdfh(reader, declared_offset)?
    {
        let recovered = scan_for_cd_start(reader, eocd_abs)?;
        tracing::warn!(
            target: "otterzip::lenient",
            declared_offset,
            declared_size,
            recovered,
            "lenient: declared CD offset is bogus — using scanned CD start"
        );
        (recovered, eocd_abs - recovered)
    } else {
        // Happy(ish) path: declared offset points at a real CDFH.
        // Still need to clamp the size so the walk doesn't run past
        // the EOCD signature.
        let max_possible = eocd_abs - declared_offset;
        let size = if declared_size == 0 {
            tracing::warn!(
                target: "otterzip::lenient",
                "lenient: EOCD cd_size is zero — using gap to EOCD ({max_possible} bytes)"
            );
            max_possible
        } else if declared_size > max_possible {
            tracing::warn!(
                target: "otterzip::lenient",
                declared = declared_size,
                clamped = max_possible,
                "lenient: clamped CD size to fit between cd_offset and EOCD"
            );
            max_possible
        } else {
            declared_size
        };
        (declared_offset, size)
    };

    Ok(CdResolved {
        cd_offset,
        cd_size,
        total_entries: u64::from(total_entries_16),
    })
}

/// Peek 4 bytes at `offset` and confirm they spell a CDFH signature.
/// Returns `Ok(false)` when the offset is at/past EOF or the bytes
/// aren't a match — used by [`find_central_directory`] to decide
/// whether the declared `cd_offset` is trustworthy.
fn peek_is_cdfh<R: Read + Seek>(reader: &mut R, offset: u64) -> Result<bool> {
    let cur = reader.stream_position()?;
    let restore = |r: &mut R| -> Result<()> {
        r.seek(SeekFrom::Start(cur))?;
        Ok(())
    };
    if reader.seek(SeekFrom::Start(offset)).is_err() {
        let _ = reader.seek(SeekFrom::Start(cur));
        return Ok(false);
    }
    let mut sig = [0u8; 4];
    match reader.read_exact(&mut sig) {
        Ok(()) => {
            restore(reader)?;
            Ok(sig == SIG_CDFH)
        }
        Err(_) => {
            let _ = reader.seek(SeekFrom::Start(cur));
            Ok(false)
        }
    }
}

/// Maximum bytes [`scan_for_cd_start`] reads behind the EOCD when
/// hunting for a recoverable CD origin. Sized to comfortably hold the
/// central directory of a 200 000-entry archive (~16 MB worth of
/// CDFH records, each ~80 bytes); larger archives have to keep the
/// declared `cd_offset` valid (which the strict path would already
/// honour) since unbounded scanning on multi-GB malformed corpora
/// stops being lenient and starts being pathological.
const MAX_CD_SCAN_BYTES: u64 = 64 * 1024 * 1024;

/// Scan the region `[max(0, eocd_abs - MAX_CD_SCAN_BYTES), eocd_abs)`
/// for the first `PK\001\002` signature. Returns the absolute byte
/// offset of that hit. Lenient recovery for archives where the EOCD
/// claims a `cd_offset` we can't trust (past EOF, inside an LFH, or
/// otherwise non-CDFH-aligned).
fn scan_for_cd_start<R: Read + Seek>(reader: &mut R, eocd_abs: u64) -> Result<u64> {
    let scan_len = eocd_abs.min(MAX_CD_SCAN_BYTES);
    let scan_start = eocd_abs - scan_len;
    reader.seek(SeekFrom::Start(scan_start))?;
    let mut buf = vec![0u8; scan_len as usize];
    reader.read_exact(&mut buf)?;
    let off_in_buf = buf
        .windows(4)
        .position(|w| w == SIG_CDFH)
        .ok_or_else(|| OtterzipError::Corrupted {
            reason: format!(
                "no CDFH signature found in last {scan_len} bytes before EOCD"
            ),
            entry: None,
        })?;
    Ok(scan_start + off_in_buf as u64)
}

/// 32-bit EOCD values kept for the lenient ZIP64 fallback path —
/// when the locator is missing but the EOCD's small fields are still
/// usable we honour them rather than failing the whole open. `cd_size_32`
/// isn't carried because the recovery path always derives size from
/// `eocd_abs - cd_offset` (the original 32-bit `cd_size` is the field
/// that was lying when we landed here).
struct ResolvedFallback {
    cd_offset_32: u32,
    total_entries_16: u16,
}

/// Follow the ZIP64 EOCD locator → ZIP64 EOCD record chain. Lenient:
/// when the locator signature is missing but the small-EOCD fields are
/// in-range, fall back to the 32-bit values rather than fail.
fn resolve_zip64<R: Read + Seek>(
    reader: &mut R,
    tail: &[u8],
    tail_start: u64,
    eocd_in_tail: usize,
    eocd_abs: u64,
    file_size: u64,
    fallback: ResolvedFallback,
) -> Result<CdResolved> {
    // The locator sits immediately before the EOCD. If our tail
    // buffer captured it, slice; otherwise issue a small targeted
    // read for those 20 bytes.
    let locator: [u8; ZIP64_LOCATOR_SIZE] = if eocd_in_tail >= ZIP64_LOCATOR_SIZE {
        let mut buf = [0u8; ZIP64_LOCATOR_SIZE];
        buf.copy_from_slice(&tail[eocd_in_tail - ZIP64_LOCATOR_SIZE..eocd_in_tail]);
        let _ = tail_start;
        buf
    } else {
        let loc_abs = eocd_abs
            .checked_sub(ZIP64_LOCATOR_SIZE as u64)
            .ok_or_else(|| OtterzipError::Corrupted {
                reason: "ZIP64 locator slot underflows file start".into(),
                entry: None,
            })?;
        reader.seek(SeekFrom::Start(loc_abs))?;
        let mut buf = [0u8; ZIP64_LOCATOR_SIZE];
        reader.read_exact(&mut buf)?;
        buf
    };

    if locator[..4] != SIG_ZIP64_LOCATOR {
        // No locator. Some toolchains emit the ZIP64 sentinels in the
        // small EOCD but never bother with the locator + ZIP64 record;
        // others have a `cd_size == u32::MAX` field that triggers the
        // ZIP64 branch here but the rest of the archive is otherwise
        // 32-bit. Try to recover via the 32-bit `cd_offset` (when it
        // points at a real CDFH) or by scanning the bytes before the
        // EOCD for the actual CD origin.
        let declared_offset = u64::from(fallback.cd_offset_32);
        let trust_declared = fallback.cd_offset_32 != u32::MAX
            && declared_offset < eocd_abs
            && peek_is_cdfh(reader, declared_offset)?;
        if trust_declared {
            let cd_size = eocd_abs - declared_offset;
            tracing::warn!(
                target: "otterzip::lenient",
                cd_offset = declared_offset,
                cd_size,
                "lenient: ZIP64 sentinel without locator — honouring 32-bit cd_offset"
            );
            return Ok(CdResolved {
                cd_offset: declared_offset,
                cd_size,
                total_entries: u64::from(fallback.total_entries_16),
            });
        }
        // Last resort: scan for the CD start. Same code path the
        // non-ZIP64 lenient branch uses for the "cd_offset bogus" case.
        let recovered = scan_for_cd_start(reader, eocd_abs)?;
        tracing::warn!(
            target: "otterzip::lenient",
            recovered,
            "lenient: ZIP64 sentinel without locator — recovered CD start via scan"
        );
        return Ok(CdResolved {
            cd_offset: recovered,
            cd_size: eocd_abs - recovered,
            total_entries: u64::from(fallback.total_entries_16),
        });
    }

    let z64_eocd_offset = u64::from_le_bytes([
        locator[8], locator[9], locator[10], locator[11],
        locator[12], locator[13], locator[14], locator[15],
    ]);
    if z64_eocd_offset >= file_size {
        return Err(OtterzipError::Corrupted {
            reason: format!(
                "ZIP64 EOCD offset {z64_eocd_offset} >= file size {file_size}"
            ),
            entry: None,
        });
    }

    reader.seek(SeekFrom::Start(z64_eocd_offset))?;
    let mut record = [0u8; ZIP64_EOCD_FIXED_SIZE];
    reader.read_exact(&mut record)?;
    if record[..4] != SIG_ZIP64_EOCD {
        return Err(OtterzipError::Corrupted {
            reason: "ZIP64 EOCD record signature missing at locator target".into(),
            entry: None,
        });
    }
    let total_entries = u64::from_le_bytes([
        record[32], record[33], record[34], record[35],
        record[36], record[37], record[38], record[39],
    ]);
    let cd_size = u64::from_le_bytes([
        record[40], record[41], record[42], record[43],
        record[44], record[45], record[46], record[47],
    ]);
    let cd_offset = u64::from_le_bytes([
        record[48], record[49], record[50], record[51],
        record[52], record[53], record[54], record[55],
    ]);

    if cd_offset > file_size {
        return Err(OtterzipError::Corrupted {
            reason: format!(
                "ZIP64 CD offset {cd_offset} > file size {file_size}"
            ),
            entry: None,
        });
    }

    let max_possible = z64_eocd_offset.saturating_sub(cd_offset);
    let cd_size = if cd_size == 0 {
        tracing::warn!(
            target: "otterzip::lenient",
            "lenient: ZIP64 EOCD cd_size is zero — using gap to ZIP64 EOCD ({max_possible} bytes)"
        );
        max_possible
    } else if cd_size > max_possible {
        tracing::warn!(
            target: "otterzip::lenient",
            declared = cd_size,
            clamped = max_possible,
            "lenient: clamped ZIP64 CD size to fit between cd_offset and ZIP64 EOCD"
        );
        max_possible
    } else {
        cd_size
    };

    Ok(CdResolved {
        cd_offset,
        cd_size,
        total_entries,
    })
}

// === Central directory walk ==========================================

/// Read `cd_size` bytes at `cd_offset` and split them into one CdRecord
/// per CDFH signature. Lenient: malformed records that don't fit the
/// header arithmetic get logged + skipped via a resync to the next
/// `PK\001\002`. Returns the recovered records — possibly fewer than
/// the EOCD's declared count, which is the whole point of this path.
fn walk_central_directory<R: Read + Seek>(
    reader: &mut R,
    cdr: &CdResolved,
    file_size: u64,
) -> Result<Vec<CdRecord>> {
    if cdr.cd_size == 0 {
        return Ok(Vec::new());
    }

    reader.seek(SeekFrom::Start(cdr.cd_offset))?;
    let mut cd_buf = vec![0u8; cdr.cd_size as usize];
    // The lenient path doesn't insist on `read_exact` — a truncated CD
    // is one of the malformations we want to handle. Read whatever
    // fits and operate on the prefix.
    let mut filled = 0usize;
    while filled < cd_buf.len() {
        match reader.read(&mut cd_buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(OtterzipError::Io(e)),
        }
    }
    cd_buf.truncate(filled);

    // Pre-allocate roughly the declared count, but cap to a sane number
    // to avoid OOM on EOCDs that lie about entry counts (a known
    // adversarial pattern in fuzz corpora).
    let cap = usize::try_from(cdr.total_entries.min(1_048_576)).unwrap_or(0);
    let mut out: Vec<CdRecord> = Vec::with_capacity(cap);
    let mut skipped = 0usize;
    let mut cursor = 0usize;

    while cursor + CDFH_FIXED_SIZE <= cd_buf.len() {
        if cd_buf[cursor..cursor + 4] != SIG_CDFH {
            match find_next_cdfh(&cd_buf, cursor + 1) {
                Some(next) => {
                    tracing::warn!(
                        target: "otterzip::lenient",
                        from = cursor,
                        to = next,
                        "lenient: resynchronising to next CDFH signature"
                    );
                    cursor = next;
                    continue;
                }
                None => break,
            }
        }

        match parse_cdfh(&cd_buf[cursor..], file_size) {
            CdfhParse::Ok { record, consumed } => {
                out.push(record);
                cursor += consumed;
            }
            CdfhParse::Resync => {
                skipped += 1;
                match find_next_cdfh(&cd_buf, cursor + 4) {
                    Some(next) => cursor = next,
                    None => break,
                }
            }
        }
    }

    if skipped > 0 {
        tracing::warn!(
            target: "otterzip::lenient",
            skipped,
            kept = out.len(),
            "lenient: skipped malformed CDFH records"
        );
    }

    Ok(out)
}

/// Locate the next CDFH signature in `buf` starting from `from`. Used
/// when the current cursor lands on garbage and we need to resync.
fn find_next_cdfh(buf: &[u8], from: usize) -> Option<usize> {
    if from >= buf.len() {
        return None;
    }
    buf[from..]
        .windows(4)
        .position(|w| w == SIG_CDFH)
        .map(|rel| from + rel)
}

enum CdfhParse {
    Ok { record: CdRecord, consumed: usize },
    /// Header arithmetic was bogus; caller should resync to the next
    /// CDFH signature.
    Resync,
}

/// Parse one CDFH starting at `buf[0]` (signature already verified by
/// the caller). On success, returns the produced [`CdRecord`] and the
/// total bytes consumed (fixed header + name + extra + comment).
fn parse_cdfh(buf: &[u8], file_size: u64) -> CdfhParse {
    if buf.len() < CDFH_FIXED_SIZE {
        return CdfhParse::Resync;
    }

    let gpf = u16::from_le_bytes([buf[8], buf[9]]);
    let method = u16::from_le_bytes([buf[10], buf[11]]);
    let mtime = u16::from_le_bytes([buf[12], buf[13]]);
    let mdate = u16::from_le_bytes([buf[14], buf[15]]);
    let crc32 = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
    let comp_size_32 = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
    let uncomp_size_32 = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
    let name_len = u16::from_le_bytes([buf[28], buf[29]]) as usize;
    let extra_len = u16::from_le_bytes([buf[30], buf[31]]) as usize;
    let comment_len = u16::from_le_bytes([buf[32], buf[33]]) as usize;
    let external_attr = u32::from_le_bytes([buf[38], buf[39], buf[40], buf[41]]);
    let lfh_offset_32 = u32::from_le_bytes([buf[42], buf[43], buf[44], buf[45]]);

    let consumed = CDFH_FIXED_SIZE + name_len + extra_len + comment_len;
    if consumed > buf.len() {
        // Lengths overshoot the remaining CD bytes — typical resync
        // trigger. Caller will skip past this signature byte and
        // hunt for the next one.
        return CdfhParse::Resync;
    }

    let name_start = CDFH_FIXED_SIZE;
    let extra_start = name_start + name_len;
    let comment_start = extra_start + extra_len;
    let name_bytes = &buf[name_start..name_start + name_len];
    let extra_bytes = &buf[extra_start..extra_start + extra_len];
    let comment_bytes = &buf[comment_start..comment_start + comment_len];

    // ZIP64 escalation: any of these slots may carry a 0xFFFF...FF
    // sentinel meaning "look in the 0x0001 extra field for the real
    // value". Order inside the extra is fixed: uncomp_size, comp_size,
    // lfh_offset, disk_number — each present only when its 32-bit
    // slot is the sentinel.
    let mut uncompressed_size = u64::from(uncomp_size_32);
    let mut compressed_size = u64::from(comp_size_32);
    let mut lfh_offset = u64::from(lfh_offset_32);
    let needs_zip64_uncomp = uncomp_size_32 == u32::MAX;
    let needs_zip64_comp = comp_size_32 == u32::MAX;
    let needs_zip64_lfh = lfh_offset_32 == u32::MAX;
    if needs_zip64_uncomp || needs_zip64_comp || needs_zip64_lfh {
        if let Some((u_v, c_v, lfh_v)) = read_zip64_extra(
            extra_bytes,
            needs_zip64_uncomp,
            needs_zip64_comp,
            needs_zip64_lfh,
        ) {
            if let Some(v) = u_v {
                uncompressed_size = v;
            }
            if let Some(v) = c_v {
                compressed_size = v;
            }
            if let Some(v) = lfh_v {
                lfh_offset = v;
            }
        }
        // If the extra didn't carry a sentinel-required field we keep
        // the 32-bit value — that's still useful for partial recovery
        // and Day 2's read path will surface a per-entry error if it
        // actually overshoots the file.
    }

    // Lenient: an LFH offset past EOF means we can't read this entry,
    // but other entries in the CD may still be salvageable. Mark the
    // record with u64::MAX so Day 2 fails this one cleanly instead of
    // mis-seeking into garbage.
    let lfh_offset_marker = if lfh_offset >= file_size {
        tracing::warn!(
            target: "otterzip::lenient",
            lfh_offset,
            file_size,
            "lenient: CDFH points past EOF — entry marked unreadable"
        );
        u64::MAX
    } else {
        lfh_offset
    };

    let path = decode_name(name_bytes, gpf);
    let is_directory = path.ends_with('/');
    let is_symlink = ((external_attr >> 16) & 0o170_000) == 0o120_000;
    let compression = map_method(method);
    let encryption = if gpf & 0x0001 != 0 {
        // GP bit 0 set just means "this entry is encrypted". The exact
        // algorithm (ZipCrypto / AES-128 / AES-256) lives in the
        // optional 0x9901 WinZip AES extra; Day 2 will refine the
        // detection. For Day 1 we surface ZipCrypto as the conservative
        // "encrypted, unknown variant" placeholder — `is_encrypted_fast`
        // only cares whether the variant is non-`None`.
        EncryptionMethod::ZipCrypto
    } else {
        EncryptionMethod::None
    };
    let modified = dos_to_systime(mdate, mtime);
    let comment = if comment_bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(comment_bytes).into_owned())
    };

    let entry = Entry {
        path,
        is_directory,
        is_symlink,
        uncompressed_size,
        compressed_size,
        compression,
        encryption,
        crc32: Some(crc32),
        modified,
        accessed: None,
        created: None,
        attributes: external_attr,
        comment,
        host_os: HostOs::Unknown,
    };

    CdfhParse::Ok {
        record: CdRecord {
            entry,
            lfh_offset: lfh_offset_marker,
            raw_method: method,
            raw_gpf: gpf,
        },
        consumed,
    }
}

/// Pull ZIP64-escalated values from a CDFH's extra field. Returns
/// `Some((uncomp, comp, lfh))` when the 0x0001 tag was found — each
/// inner `Option` is populated only when the caller asked for it (i.e.
/// the matching 32-bit slot held the sentinel). Returns `None` when
/// no ZIP64 extra is present at all.
fn read_zip64_extra(
    extra: &[u8],
    want_uncomp: bool,
    want_comp: bool,
    want_lfh: bool,
) -> Option<(Option<u64>, Option<u64>, Option<u64>)> {
    let mut cursor = 0usize;
    while cursor + 4 <= extra.len() {
        let tag = u16::from_le_bytes([extra[cursor], extra[cursor + 1]]);
        let len = u16::from_le_bytes([extra[cursor + 2], extra[cursor + 3]]) as usize;
        cursor += 4;
        if cursor + len > extra.len() {
            return None;
        }
        if tag != EXTRA_TAG_ZIP64 {
            cursor += len;
            continue;
        }
        // Found the ZIP64 extra. Pull the 8-byte fields in order, only
        // for the slots the caller flagged.
        let body = &extra[cursor..cursor + len];
        let mut inner = 0usize;
        let mut out_uncomp = None;
        let mut out_comp = None;
        let mut out_lfh = None;
        if want_uncomp {
            if inner + 8 > body.len() {
                return Some((out_uncomp, out_comp, out_lfh));
            }
            out_uncomp = Some(u64::from_le_bytes([
                body[inner], body[inner + 1], body[inner + 2], body[inner + 3],
                body[inner + 4], body[inner + 5], body[inner + 6], body[inner + 7],
            ]));
            inner += 8;
        }
        if want_comp {
            if inner + 8 > body.len() {
                return Some((out_uncomp, out_comp, out_lfh));
            }
            out_comp = Some(u64::from_le_bytes([
                body[inner], body[inner + 1], body[inner + 2], body[inner + 3],
                body[inner + 4], body[inner + 5], body[inner + 6], body[inner + 7],
            ]));
            inner += 8;
        }
        if want_lfh {
            if inner + 8 > body.len() {
                return Some((out_uncomp, out_comp, out_lfh));
            }
            out_lfh = Some(u64::from_le_bytes([
                body[inner], body[inner + 1], body[inner + 2], body[inner + 3],
                body[inner + 4], body[inner + 5], body[inner + 6], body[inner + 7],
            ]));
        }
        return Some((out_uncomp, out_comp, out_lfh));
    }
    None
}

// === Field decoders ==================================================

/// Decode an entry name. Tier-1 only for Day 1: trust GP bit 11 →
/// UTF-8; otherwise fall back to `from_utf8_lossy` which preserves
/// ASCII (the regression-test fixtures we ship today are ASCII-only).
/// The full CP949 / chardetng cascade from `src/encoding.rs` lands
/// when Day 2 wires the read path — the archive-level decision needs
/// every name at once and is too heavyweight for the metadata-only
/// path.
fn decode_name(bytes: &[u8], gpf: u16) -> String {
    let utf8_flag = gpf & 0x0800 != 0;
    if utf8_flag {
        if let Ok(s) = std::str::from_utf8(bytes) {
            return s.to_string();
        }
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Map a CDFH compression method code (APPNOTE.TXT §4.4.5) to our
/// public enum. Codes we don't ship support for collapse to `Unknown`;
/// Day 2's dispatch surfaces those as `FeatureDisabled` per-entry
/// rather than failing the whole open.
fn map_method(method: u16) -> CompressionMethod {
    match method {
        0 => CompressionMethod::Store,
        8 => CompressionMethod::Deflate,
        9 => CompressionMethod::Deflate64,
        12 => CompressionMethod::Bzip2,
        14 => CompressionMethod::Lzma,
        93 => CompressionMethod::Zstd,
        98 => CompressionMethod::Ppmd,
        _ => CompressionMethod::Unknown,
    }
}

/// Convert an MS-DOS date + time pair into a `SystemTime`. DOS time is
/// the same format the `zip` crate decodes — using the same Howard
/// Hinnant civil-from-days algorithm keeps the lenient backend's
/// `Entry.modified` byte-identical to the strict backend's for the
/// regression-test parity assertion.
fn dos_to_systime(dos_date: u16, dos_time: u16) -> Option<SystemTime> {
    if dos_date == 0 && dos_time == 0 {
        return None;
    }
    let year = ((dos_date >> 9) as i32) + 1980;
    let month = u32::from((dos_date >> 5) & 0x0f);
    let day = u32::from(dos_date & 0x1f);
    let hour = u32::from(dos_time >> 11);
    let minute = u32::from((dos_time >> 5) & 0x3f);
    let second = u32::from((dos_time & 0x1f) * 2);

    let days = days_from_civil(year, month, day)?;
    let secs = i64::from(days) * 86_400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second);
    let secs_u64 = u64::try_from(secs).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(secs_u64))
}

/// Days since 1970-01-01 for a (proleptic Gregorian) date. Copied from
/// `backends/zip.rs` so the lenient and strict backends agree on the
/// epoch arithmetic (the parity test wants byte-identical `Entry`s,
/// including `modified`).
fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i32> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let civil_year = if month <= 2 { year - 1 } else { year };
    let era = civil_year.div_euclid(400);
    let yoe = u32::try_from(civil_year.rem_euclid(400)).ok()?;
    let month_offset = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * month_offset + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era.checked_mul(146_097)?
        .checked_add(i32::try_from(doe).ok()?)?
        .checked_sub(719_468)
}

// === Internal probe (Day 1 test surface) ==============================

/// Test-only entry point — opens a lenient backend and returns the
/// `(path, uncompressed_size)` pairs the CD walk produced. The Day 1
/// regression test in `tests/lenient_zip.rs` uses this to compare
/// lenient vs strict parity without depending on the
/// `libarchive-fallback` feature being on (the dispatcher's fallback
/// arm only fires for malformed archives, but parity should hold for
/// healthy ones too).
#[doc(hidden)]
pub fn __probe_entries(path: &Path) -> Result<Vec<(String, u64)>> {
    let backend = LenientZipBackend::open(path, None)?;
    let iter = backend.entries()?;
    iter.map(|r| r.map(|e| (e.path, e.uncompressed_size))).collect()
}

/// Suppress dead-code warnings on Day-2 fields that the metadata path
/// doesn't read. Once Day 2 wires the extract path these go away.
#[allow(dead_code)]
fn _day2_field_uses(r: &CdRecord) {
    let _ = r.lfh_offset;
    let _ = r.raw_method;
    let _ = r.raw_gpf;
}

// === Tests ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;

    fn build_simple_zip(path: &Path) {
        let f = fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        w.start_file("hello.txt", opts).unwrap();
        w.write_all(b"hello world").unwrap();
        w.add_directory("sub/", opts).unwrap();
        w.start_file("sub/nested.bin", opts).unwrap();
        w.write_all(&[0u8; 1024]).unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn open_healthy_zip_lists_three_entries() {
        let td = tempdir().unwrap();
        let p = td.path().join("a.zip");
        build_simple_zip(&p);
        let pairs = __probe_entries(&p).unwrap();
        assert_eq!(pairs.len(), 3);
        assert!(pairs.iter().any(|(n, _)| n == "hello.txt"));
        assert!(pairs.iter().any(|(n, _)| n == "sub/"));
        assert!(pairs.iter().any(|(n, sz)| n == "sub/nested.bin" && *sz == 1024));
    }

    #[test]
    fn empty_archive_too_small_returns_corrupted() {
        let td = tempdir().unwrap();
        let p = td.path().join("tiny.zip");
        fs::write(&p, b"\x00\x00\x00").unwrap();
        // `LenientZipBackend` doesn't impl Debug (no need on the
        // public surface), so the canonical `unwrap_err()` doesn't
        // type-check here — match on the result instead.
        match LenientZipBackend::open(&p, None) {
            Err(OtterzipError::Corrupted { .. }) => {}
            Err(other) => panic!("expected Corrupted, got {other:?}"),
            Ok(_) => panic!("3-byte file should not parse as a ZIP"),
        }
    }

    #[test]
    fn dos_time_round_trip_is_sane() {
        // 1990-06-15 12:30:00 UTC. Hand-encode the DOS date/time bit
        // layout (APPNOTE.TXT §4.4.6) and decode it; the result must
        // round-trip to the matching epoch second.
        let dos_date = ((1990u16 - 1980) << 9) | (6u16 << 5) | 15;
        let dos_time = (12u16 << 11) | (30u16 << 5);
        let st = dos_to_systime(dos_date, dos_time).unwrap();
        let secs = st.duration_since(UNIX_EPOCH).unwrap().as_secs();
        // 1990-01-01 00:00:00 UTC = 631_152_000 epoch seconds; add 165
        // days (Jan 31 + Feb 28 + Mar 31 + Apr 30 + May 31 + 14) for
        // 1990-06-15 00:00:00 = 645_408_000, then +12h30m = 645_453_000.
        assert_eq!(secs, 645_453_000);
    }
}
