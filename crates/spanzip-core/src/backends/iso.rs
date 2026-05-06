//! ISO9660 disk image backend (read-only).
//!
//! PR-F5 (Phase 7+ option Y). The original plan slot was the `cdfs`
//! crate, but the F5 PoC gate caught its hard dependency on `fuser`,
//! which doesn't build on Windows. We swapped to `iso9660-rs` 1.0.2
//! -- `no_std`, cross-platform, with explicit `joliet` and
//! `rock-ridge` feature flags. See `docs/05-build/phase-7-plus-plan.md`
//! §7 for the swap log.
//!
//! Architecture
//! -----------
//! `iso9660-rs` consumes a `gpt_disk_io::BlockIo` impl rather than a
//! plain `Read + Seek`. We provide a thin [`FileBlockIo`] adapter
//! that maps each 2048-byte logical block to a seek + read on the
//! source `File`, plus a small in-memory cache keyed by the last
//! sector LBA so the walker doesn't re-read the same sector when it
//! consumes a directory record that happens to span 2 sectors.
//!
//! Directory enumeration is recursive: we start at the root extent
//! and depth-walk every sub-directory, accumulating `(path, FileEntry)`
//! pairs into a flat list. The list is materialised once at backend
//! open time so the `entries()` and `extract_entry` API contracts are
//! cheap (a clone or HashMap lookup).
//!
//! UDF
//! ---
//! UDF is **not** supported in v1.0 (deferred to v1.1, see
//! `docs/01-plan/schema.md` §1.1.1). UDF disk images use a different
//! anchor/partition layout and have no usable ISO9660 volume
//! descriptor at sector 16. When a caller hands us a UDF-only `.iso`
//! file `iso9660::mount` returns an error which we surface as
//! [`SpanzipError::UnsupportedFormat`] with a "UDF" hint string so
//! the UI can show the right message.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use gpt_disk_io::{
    gpt_disk_types::{BlockSize, Lba},
    BlockIo,
};
use iso9660::{
    directory::iterator::DirectoryIterator, mount, FileEntry, Iso9660Error, VolumeInfo,
};

use crate::backends::ArchiveBackend;
use crate::entry::{Entry, HostOs};
use crate::error::{Result, SpanzipError};
use crate::format::{CompressionMethod, EncryptionMethod};

// 2048 bytes is the canonical CD/DVD sector size and matches every
// ISO9660 image we care about. Mixed-block-size optical media are
// out of scope for v1.0.
const SECTOR_SIZE: usize = 2048;

/// `BlockIo` adapter that fronts a regular file with the 2048-byte
/// sector view `iso9660` expects.
///
/// We keep a one-sector cache so the directory iterator -- which
/// frequently re-visits the same LBA when records straddle a sector
/// boundary -- doesn't pay a full seek + read per record. This is
/// not a hot loop today (extract_all materialises the index once)
/// but the cache costs ~2 KB of RAM and shaves a measurable amount
/// of syscalls on large ISOs.
struct FileBlockIo {
    file: File,
    num_blocks: u64,
    cache_lba: Option<u64>,
    cache_buf: [u8; SECTOR_SIZE],
}

impl FileBlockIo {
    fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        // Round up rather than down — a partial trailing sector still
        // counts so we don't refuse to read it.
        let num_blocks = (len + (SECTOR_SIZE as u64) - 1) / (SECTOR_SIZE as u64);
        Ok(Self {
            file,
            num_blocks,
            cache_lba: None,
            cache_buf: [0u8; SECTOR_SIZE],
        })
    }
}

impl BlockIo for FileBlockIo {
    type Error = io::Error;

    fn block_size(&self) -> BlockSize {
        // 2048 is the canonical CD/DVD sector size; gpt_disk_types
        // only exposes BS_512 / BS_4096 as constants so we
        // construct the value via `new`. Unwrap is safe -- 2048 is
        // a non-zero u32.
        BlockSize::new(SECTOR_SIZE as u32).expect("2048 is a valid BlockSize")
    }

    fn num_blocks(&mut self) -> std::result::Result<u64, Self::Error> {
        Ok(self.num_blocks)
    }

    fn read_blocks(
        &mut self,
        start_lba: Lba,
        dst: &mut [u8],
    ) -> std::result::Result<(), Self::Error> {
        let lba = start_lba.0;
        // Single-sector fast path uses the cache.
        if dst.len() == SECTOR_SIZE && self.cache_lba == Some(lba) {
            dst.copy_from_slice(&self.cache_buf);
            return Ok(());
        }
        let offset = lba.checked_mul(SECTOR_SIZE as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "lba overflow")
        })?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(dst)?;
        // Refresh the cache only on aligned single-sector reads.
        if dst.len() == SECTOR_SIZE {
            self.cache_lba = Some(lba);
            self.cache_buf.copy_from_slice(dst);
        }
        Ok(())
    }

    fn write_blocks(
        &mut self,
        _start_lba: Lba,
        _src: &[u8],
    ) -> std::result::Result<(), Self::Error> {
        // Read-only by construction: the dispatcher already returns
        // FeatureDisabled for ISO creation modes, so this shouldn't
        // be reachable. Keep the impl honest with an explicit error
        // rather than `unreachable!()` so a future refactor surfaces
        // the misuse cleanly.
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "ISO backend is read-only",
        ))
    }

    fn flush(&mut self) -> std::result::Result<(), Self::Error> {
        Ok(())
    }
}

/// One materialised entry — the path inside the archive plus the
/// underlying ISO9660 record. We keep both because:
///
/// * the public [`Entry`] needs the archive-relative path string,
/// * `read_file` needs the original `FileEntry` (for the extent
///   coordinates), and looking it up by path again would mean
///   re-walking the directory tree.
#[derive(Clone)]
struct IsoEntry {
    archive_path: String,
    is_directory: bool,
    file: FileEntry,
}

pub(crate) struct IsoBackend {
    block_io: RefCell<FileBlockIo>,
    volume: VolumeInfo,
    /// Flat list of every entry in the volume, walked once at open.
    /// Order is depth-first preorder so directories appear before their
    /// contents -- matches the rest of the codebase's `entries()`
    /// expectations.
    flat: Vec<IsoEntry>,
    /// Path -> index into `flat` for O(1) `extract_entry` lookups.
    by_path: HashMap<String, usize>,
}

impl IsoBackend {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let mut block_io = FileBlockIo::open(path).map_err(SpanzipError::Io)?;
        // mount expects a *relative* sector to start scanning at; for
        // the standard "ISO at offset 0" case that's always 0. Hybrid
        // ISOs that embed the volume at a non-zero offset aren't in
        // scope for v1.0 -- those would need a separate detect path.
        let volume = mount(&mut block_io, 0).map_err(map_iso_err)?;

        let mut flat: Vec<IsoEntry> = Vec::new();
        let mut by_path: HashMap<String, usize> = HashMap::new();
        walk_directory(
            &mut block_io,
            volume.root_extent_lba,
            volume.root_extent_len,
            String::new(),
            &mut flat,
            &mut by_path,
        )
        .map_err(map_iso_err)?;

        Ok(Self {
            block_io: RefCell::new(block_io),
            volume,
            flat,
            by_path,
        })
    }

    /// Volume label (debug/UI helper). Not exposed via the public
    /// `Archive` API yet -- v1.1 if Settings/Info needs it.
    #[allow(dead_code)]
    pub(crate) fn volume_label(&self) -> String {
        let raw = &self.volume.volume_id;
        // Trim trailing spaces (ISO9660 pads identifiers with 0x20)
        // and any stray nul bytes.
        let trimmed_end = raw.iter().rposition(|&b| b != 0x20 && b != 0).map_or(0, |i| i + 1);
        String::from_utf8_lossy(&raw[..trimmed_end]).into_owned()
    }
}

/// Translate an `iso9660` error into our domain error type.
/// We surface the most actionable variant we can — IoError stays
/// IO, parse errors become BackendError so the higher layer can
/// log them, and the "no PVD found" case is mapped to
/// UnsupportedFormat with a UDF hint because that's by far the
/// most common cause for a plain `.iso` to fail to mount.
fn map_iso_err(e: Iso9660Error) -> SpanzipError {
    match e {
        Iso9660Error::IoError | Iso9660Error::ReadFailed => {
            SpanzipError::Io(io::Error::new(io::ErrorKind::Other, "iso9660 io"))
        }
        // Mount-level failures usually mean the file isn't an
        // ISO9660 volume at all. UDF is the most common false-
        // positive here (the .iso extension is shared); surface
        // a hint string so the UI can render the v1.1 message.
        Iso9660Error::InvalidSignature
        | Iso9660Error::UnsupportedVersion
        | Iso9660Error::NotFound => SpanzipError::UnsupportedFormat(Some(
            "ISO9660 volume descriptor missing — UDF? (v1.1)".into(),
        )),
        other => SpanzipError::BackendError(format!("iso9660: {other:?}")),
    }
}

/// Recursively walk an ISO9660 directory, appending every encountered
/// entry to `flat`. Skips the canonical `.` / `..` records.
fn walk_directory(
    block_io: &mut FileBlockIo,
    extent_lba: u32,
    extent_len: u32,
    prefix: String,
    flat: &mut Vec<IsoEntry>,
    by_path: &mut HashMap<String, usize>,
) -> std::result::Result<(), Iso9660Error> {
    let iter = DirectoryIterator::new(block_io, extent_lba, extent_len);

    // Collect first so the recursive call doesn't fight the
    // iterator's `&mut block_io` borrow.
    let mut children: Vec<FileEntry> = Vec::new();
    for item in iter {
        let entry = item?;
        // ISO9660 always emits self/parent records as the first two
        // entries of every directory. Skip them.
        if entry.name == "." || entry.name == ".." || entry.name == "\u{0}" || entry.name == "\u{1}" {
            continue;
        }
        children.push(entry);
    }

    for entry in children {
        let archive_path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{}/{}", prefix.trim_end_matches('/'), entry.name)
        };
        let is_dir = entry.flags.directory;
        let idx = flat.len();
        flat.push(IsoEntry {
            archive_path: archive_path.clone(),
            is_directory: is_dir,
            file: entry.clone(),
        });
        by_path.insert(archive_path.clone(), idx);

        if is_dir {
            walk_directory(
                block_io,
                entry.extent_lba,
                entry.data_length,
                archive_path,
                flat,
                by_path,
            )?;
        }
    }
    Ok(())
}

fn iso_to_entry(e: &IsoEntry) -> Entry {
    Entry {
        path: e.archive_path.clone(),
        is_directory: e.is_directory,
        is_symlink: false,
        // ISO9660 doesn't store an "uncompressed" vs "compressed"
        // distinction — the on-disk data length IS both.
        uncompressed_size: u64::from(e.file.data_length),
        compressed_size: u64::from(e.file.data_length),
        compression: CompressionMethod::Store,
        encryption: EncryptionMethod::None,
        crc32: None,
        // Per-record timestamps live in the directory record but we
        // don't surface them yet (would require a `Date` parse round-
        // trip via `iso9660::utils`). Optional metadata; the rest
        // of the codebase tolerates `None` here.
        modified: None,
        accessed: None,
        created: None,
        attributes: 0,
        comment: None,
        host_os: HostOs::Unknown,
    }
}

impl ArchiveBackend for IsoBackend {
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>> {
        let snapshot: Vec<Entry> = self.flat.iter().map(iso_to_entry).collect();
        Ok(Box::new(snapshot.into_iter().map(Ok)))
    }

    fn extract_entry(&self, entry_path: &str, out: &mut dyn std::io::Write) -> Result<u64> {
        let idx = *self
            .by_path
            .get(entry_path)
            .ok_or_else(|| SpanzipError::EntryNotFound(entry_path.to_string()))?;
        let iso_entry = &self.flat[idx];
        if iso_entry.is_directory {
            // Directories carry no data of their own — the wrapper
            // creates the directory itself before calling us.
            return Ok(0);
        }
        let mut block_io = self.block_io.borrow_mut();
        let bytes = iso9660::read_file_vec(&mut *block_io, &iso_entry.file)
            .map_err(map_iso_err)?;
        out.write_all(&bytes)?;
        Ok(bytes.len() as u64)
    }

    fn open_entry_stream(&self, entry_path: &str) -> Result<Box<dyn Read + Send + '_>> {
        let mut buf = Vec::new();
        self.extract_entry(entry_path, &mut buf)?;
        Ok(Box::new(io::Cursor::new(buf)))
    }
}

// Silence unused-warning when the `volume_label` helper isn't called
// in the test-build configuration.
#[allow(dead_code)]
fn _force_use_pathbuf(_: PathBuf) {}
