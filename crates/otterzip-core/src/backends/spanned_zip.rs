//! Spanned-ZIP byte rewriter — turns an APPNOTE.TXT §8 multi-disk
//! archive into something the upstream single-disk `zip` crate can
//! read without modifications.
//!
//! ## Why this exists
//!
//! [`MultiVolumeReader`] already presents N files as one virtual
//! byte stream, which is enough for the "raw byte-split" shape that
//! 7-Zip and Info-ZIP emit when asked to split a finished ZIP into
//! fixed-size chunks. **Real APPNOTE-spanned archives** (WinRAR's
//! split-to-volumes with ZIP format, Info-ZIP `zip -s`) carry
//! disk-relative offsets in the EOCD and every central-directory
//! entry — those references are valid only under a multi-disk
//! parser. The `zip` crate treats `cd_offset` as absolute from the
//! start of the file, so a naive concatenation lands it at random
//! bytes and the parse fails with `InvalidArchive("Could not find
//! EOCD")` or similar.
//!
//! The fix is to rewrite the tail of the virtual stream in-flight:
//!
//!   * Locate the EOCD in the last volume.
//!   * Parse `disk_with_cd_start` and `cd_offset_in_disk`.
//!   * Compute the absolute virtual position of the CD start.
//!   * Walk each CD entry, replacing `disk_number_start` with 0 and
//!     `local_header_offset_in_disk` with its absolute virtual value.
//!   * Replace EOCD fields so the `zip` crate sees a single-disk
//!     archive whose CD starts at the absolute virtual offset.
//!
//! Bytes outside the CD + EOCD region pass through unchanged, so
//! local file headers + compressed payloads work as-is — the
//! `MultiVolumeReader` handles the per-volume seek discipline.
//!
//! ## Limitations
//!
//! * **ZIP64 not yet handled.** If the EOCD's `cd_offset` or any
//!   entry's `local_header_offset` would exceed `u32::MAX` after
//!   patching, the open fails with `UnsupportedFormat` — that needs
//!   a ZIP64 EOCD synthesis path (planned for v1.1). Single-volume
//!   sums under 4 GiB cover the vast majority of real user splits.
//! * Per-volume compressed data that crosses a disk boundary is fine
//!   — `MultiVolumeReader` already handles boundary-spanning reads
//!   transparently because each volume's bytes are contiguous in
//!   the virtual stream.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;

use crate::backends::multi_volume_reader::MultiVolumeReader;

/// EOCD signature: little-endian "PK\x05\x06".
const SIG_EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
/// Central-directory entry header signature: "PK\x01\x02".
const SIG_CD_ENTRY: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
/// ZIP64 EOCD locator signature: "PK\x06\x07". When present
/// immediately before the regular EOCD, the archive is using ZIP64
/// extensions — those are not yet handled by the patcher.
const SIG_ZIP64_LOCATOR: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];

/// `Read + Seek` adapter that presents a spanned ZIP as a single-disk
/// archive. Internally either delegates straight to the raw
/// [`MultiVolumeReader`] (when the EOCD already looks single-disk and
/// no patching is needed) or overlays a patched tail covering the CD
/// + EOCD region.
pub(crate) struct SpannedZipReader {
    raw: MultiVolumeReader,
    /// First virtual byte position covered by `patched_tail`.
    /// `u64::MAX` means the overlay is disabled — every read falls
    /// through to `raw`.
    patch_start: u64,
    /// Patched bytes covering `[patch_start, patch_start + len)`.
    /// Empty when no patching is needed.
    patched_tail: Vec<u8>,
    /// Current virtual read position.
    pos: u64,
}

impl SpannedZipReader {
    /// Open the volumes and decide whether tail-patching is required.
    pub(crate) fn open(paths: &[PathBuf]) -> io::Result<Self> {
        let mut raw = MultiVolumeReader::open(paths)?;
        let total = raw.total_size();
        if total < 22 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "spanned archive total size shorter than minimum EOCD record",
            ));
        }

        // Scan the last 64 KiB + a small ZIP64-locator margin for the
        // EOCD signature. APPNOTE caps the EOCD comment at 65535
        // bytes, so 65557 = 22 + 65535 is the maximum legal distance
        // from end-of-file to EOCD start.
        let scan_window = u64::min(65_557, total) as usize;
        let scan_start = total - scan_window as u64;
        raw.seek(SeekFrom::Start(scan_start))?;
        let mut tail = vec![0u8; scan_window];
        raw.read_exact(&mut tail)?;

        let eocd_in_tail = find_eocd(&tail).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "spanned ZIP: EOCD signature not present in last 64 KiB of last volume",
            )
        })?;
        let eocd_pos = scan_start + eocd_in_tail as u64;

        // Optional ZIP64 EOCD locator sits immediately before the
        // regular EOCD when ZIP64 extensions are in use. The patcher
        // doesn't handle ZIP64 yet; surface a clear error so the host
        // UI can keep the localised "v1.1 예정" card.
        if eocd_in_tail >= 20 {
            let locator = &tail[eocd_in_tail - 20..eocd_in_tail];
            if locator[..4] == SIG_ZIP64_LOCATOR {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "spanned ZIP: ZIP64 extensions not yet supported",
                ));
            }
        }

        let eocd = &tail[eocd_in_tail..];
        if eocd.len() < 22 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "spanned ZIP: EOCD truncated",
            ));
        }
        let disk_number = u16::from_le_bytes([eocd[4], eocd[5]]);
        let disk_with_cd = u16::from_le_bytes([eocd[6], eocd[7]]);
        let num_entries_this_disk = u16::from_le_bytes([eocd[8], eocd[9]]);
        let num_entries_total = u16::from_le_bytes([eocd[10], eocd[11]]);
        let cd_size = u32::from_le_bytes([eocd[12], eocd[13], eocd[14], eocd[15]]);
        let cd_offset_in_disk = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]);
        let comment_len = u16::from_le_bytes([eocd[20], eocd[21]]) as usize;

        let is_real_spanned = disk_number != 0
            || disk_with_cd != 0
            || num_entries_this_disk != num_entries_total;

        if !is_real_spanned {
            // Byte-concat shape — the upstream `zip` crate can already
            // read this without patching. Rewind and hand off the raw
            // reader.
            raw.seek(SeekFrom::Start(0))?;
            return Ok(Self {
                raw,
                patch_start: u64::MAX,
                patched_tail: Vec::new(),
                pos: 0,
            });
        }

        // Real spanned archive. Compute the CD's absolute virtual start.
        let cd_start_virtual = raw.disk_origin(disk_with_cd as usize) + cd_offset_in_disk as u64;
        if cd_start_virtual + cd_size as u64 > eocd_pos {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "spanned ZIP: CD declared larger than the bytes between CD start and EOCD",
            ));
        }

        // Read raw CD bytes, walk entries, and patch each one in place.
        raw.seek(SeekFrom::Start(cd_start_virtual))?;
        let mut cd_bytes = vec![0u8; cd_size as usize];
        raw.read_exact(&mut cd_bytes)?;
        patch_central_directory(&mut cd_bytes, &raw)?;

        // Build the patched tail buffer: patched CD || raw bytes
        // between CD end and EOCD (typically empty) || patched EOCD.
        let mut patched_tail: Vec<u8> = Vec::with_capacity(cd_bytes.len() + 22 + comment_len);
        patched_tail.extend_from_slice(&cd_bytes);

        let between_size = eocd_pos - (cd_start_virtual + cd_size as u64);
        if between_size > 0 {
            raw.seek(SeekFrom::Start(cd_start_virtual + cd_size as u64))?;
            let mut filler = vec![0u8; between_size as usize];
            raw.read_exact(&mut filler)?;
            patched_tail.extend_from_slice(&filler);
        }

        if cd_start_virtual > u32::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "spanned ZIP: CD start beyond 4 GiB (ZIP64 required, not yet supported)",
            ));
        }
        let mut eocd_patched = eocd[..22 + comment_len].to_vec();
        eocd_patched[4..6].copy_from_slice(&0u16.to_le_bytes());
        eocd_patched[6..8].copy_from_slice(&0u16.to_le_bytes());
        eocd_patched[8..10].copy_from_slice(&num_entries_total.to_le_bytes());
        // entries_total at [10..12] stays.
        // cd_size at [12..16] stays.
        eocd_patched[16..20].copy_from_slice(&(cd_start_virtual as u32).to_le_bytes());
        // comment_length at [20..22] stays.
        patched_tail.extend_from_slice(&eocd_patched);

        raw.seek(SeekFrom::Start(0))?;
        Ok(Self {
            raw,
            patch_start: cd_start_virtual,
            patched_tail,
            pos: 0,
        })
    }

    fn total_virtual_size(&self) -> u64 {
        if self.patched_tail.is_empty() {
            self.raw.total_size()
        } else {
            self.patch_start + self.patched_tail.len() as u64
        }
    }
}

/// Walk every CD-entry header in `cd_bytes`, replacing the per-disk
/// offset fields with absolute virtual offsets so the upstream parser
/// (which treats them as single-disk absolute offsets) reads the
/// right bytes.
fn patch_central_directory(cd_bytes: &mut [u8], raw: &MultiVolumeReader) -> io::Result<()> {
    let mut cursor = 0;
    while cursor + 46 <= cd_bytes.len() {
        if cd_bytes[cursor..cursor + 4] != SIG_CD_ENTRY {
            // Unknown record or end-of-CD padding — stop walking.
            break;
        }
        let filename_len =
            u16::from_le_bytes([cd_bytes[cursor + 28], cd_bytes[cursor + 29]]) as usize;
        let extra_len =
            u16::from_le_bytes([cd_bytes[cursor + 30], cd_bytes[cursor + 31]]) as usize;
        let comment_len =
            u16::from_le_bytes([cd_bytes[cursor + 32], cd_bytes[cursor + 33]]) as usize;
        let disk_start =
            u16::from_le_bytes([cd_bytes[cursor + 34], cd_bytes[cursor + 35]]);
        let lfh_off_in_disk = u32::from_le_bytes([
            cd_bytes[cursor + 42],
            cd_bytes[cursor + 43],
            cd_bytes[cursor + 44],
            cd_bytes[cursor + 45],
        ]);
        let lfh_absolute = raw.disk_origin(disk_start as usize) + lfh_off_in_disk as u64;
        if lfh_absolute > u32::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "spanned ZIP: local header offset beyond 4 GiB (ZIP64 required)",
            ));
        }
        cd_bytes[cursor + 34..cursor + 36].copy_from_slice(&0u16.to_le_bytes());
        cd_bytes[cursor + 42..cursor + 46]
            .copy_from_slice(&(lfh_absolute as u32).to_le_bytes());

        cursor += 46 + filename_len + extra_len + comment_len;
    }
    Ok(())
}

/// Backward scan for the EOCD signature. Returns the byte offset
/// inside `tail` where the EOCD record starts, or `None` when the
/// magic isn't present anywhere in the scanned window.
fn find_eocd(tail: &[u8]) -> Option<usize> {
    if tail.len() < 22 {
        return None;
    }
    let mut i = tail.len() - 22;
    loop {
        if tail[i..i + 4] == SIG_EOCD {
            // Verify the comment length is consistent with how much
            // tail remains — guards against accidental signature
            // matches inside compressed data.
            let comment_len = u16::from_le_bytes([tail[i + 20], tail[i + 21]]) as usize;
            if i + 22 + comment_len == tail.len() {
                return Some(i);
            }
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

impl Read for SpannedZipReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.pos >= self.total_virtual_size() {
            return Ok(0);
        }
        if self.pos < self.patch_start {
            // Outside the patched region: serve raw bytes, capped at
            // patch_start so we never spill into stale CD data.
            let max = self.patch_start - self.pos;
            let cap = max.min(out.len() as u64) as usize;
            self.raw.seek(SeekFrom::Start(self.pos))?;
            let n = self.raw.read(&mut out[..cap])?;
            self.pos = self.pos.saturating_add(n as u64);
            Ok(n)
        } else {
            // Inside the patched region: copy from our buffer.
            let patched_offset = (self.pos - self.patch_start) as usize;
            if patched_offset >= self.patched_tail.len() {
                return Ok(0);
            }
            let avail = self.patched_tail.len() - patched_offset;
            let cap = avail.min(out.len());
            out[..cap].copy_from_slice(&self.patched_tail[patched_offset..patched_offset + cap]);
            self.pos = self.pos.saturating_add(cap as u64);
            Ok(cap)
        }
    }
}

impl Seek for SpannedZipReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let total = self.total_virtual_size();
        let target: i128 = match from {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
            SeekFrom::End(d) => total as i128 + d as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SpannedZipReader: seek before start",
            ));
        }
        self.pos = (target as u64).min(total);
        Ok(self.pos)
    }
}
