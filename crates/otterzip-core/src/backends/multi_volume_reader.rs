//! Virtual `Read + Seek` adapter over a contiguous sequence of files.
//!
//! Used by [`super::zip::ZipBackend`] to read spanned / split ZIP
//! archives (APPNOTE.TXT §8) without having to teach the `zip` crate
//! about multi-disk reading. The trick: present the concatenation
//! `vol1.z01 || vol1.z02 || … || vol1.zip` to the `ZipArchive`
//! constructor as a single seekable byte stream — the EOCD lives at
//! the end of the last file, the central directory references local
//! headers via absolute byte positions in this virtual stream, and
//! everything downstream just works as if it were one file.
//!
//! ## Limitations
//!
//! * Real APPNOTE-spanned archives whose central directory entries
//!   reference `disk_number_start > 0` will trip the `zip` crate's
//!   single-disk check. We patch those out in
//!   [`super::zip::ZipBackend::open_multi`] when needed — the volumes
//!   themselves stay byte-identical on disk; only the in-memory
//!   `ZipArchive` view is rewritten.
//! * No parallel-extract path: the underlying file handles can't be
//!   cheaply duplicated per worker thread. Multi-volume reads fall
//!   back to the serial `extract_all` loop, which is fine for the
//!   one-archive-at-a-time UX we ship today.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Windows `FILE_FLAG_SEQUENTIAL_SCAN`. Hints to the NTFS cache
/// manager that the file will be read front-to-back so it can
/// prefetch aggressively and free pages eagerly behind us. Decisive
/// on large spanned-ZIP volumes where Bandizip / 7-Zip already
/// benefit from the same flag. See
/// <https://devblogs.microsoft.com/oldnewthing/20221130-00/?p=107505>.
#[cfg(windows)]
const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;

/// Concatenates a sequence of files into one virtual byte stream.
///
/// Positions are absolute over the whole concatenation: byte 0 is the
/// first byte of `volumes[0]`, byte `sizes[0]` is the first byte of
/// `volumes[1]`, and so on up to `total`.
pub(crate) struct MultiVolumeReader {
    /// Per-volume open file handles, in disk order. We keep all of
    /// them open for the duration of the read — typical split sets are
    /// under 100 volumes, well below the per-process FD limit.
    files: Vec<File>,
    /// Per-volume sizes, parallel to `files`.
    sizes: Vec<u64>,
    /// `cumulative[i]` = first virtual byte position belonging to
    /// `files[i]`. `cumulative[files.len()]` = total.
    cumulative: Vec<u64>,
    /// Total virtual length = sum of `sizes`.
    total: u64,
    /// Current virtual read position.
    virtual_pos: u64,
    /// Index of the file whose seek cursor currently matches
    /// `virtual_pos`. Tracked so back-to-back reads inside one volume
    /// don't pay a redundant `File::seek` syscall.
    active: usize,
    /// Per-file cursor cache (in raw bytes). When the next read's
    /// computed `file_offset` matches the cached cursor for that
    /// file, we skip the `File::seek` syscall entirely — the file's
    /// own kernel-side position is already where we need it. The
    /// cache is invalidated by external `Seek` calls.
    file_cursors: Vec<u64>,
}

impl MultiVolumeReader {
    /// Open every volume in `paths` and prepare them for sequential /
    /// random access. The first volume's cursor is positioned at 0;
    /// remaining volumes are left at 0 (Read implementation re-seeks
    /// when crossing a boundary, so the initial state is moot).
    pub(crate) fn open(paths: &[PathBuf]) -> io::Result<Self> {
        if paths.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "MultiVolumeReader: at least one volume required",
            ));
        }
        let mut files = Vec::with_capacity(paths.len());
        let mut sizes = Vec::with_capacity(paths.len());
        let mut cumulative = Vec::with_capacity(paths.len() + 1);
        let mut acc: u64 = 0;
        cumulative.push(0);
        for p in paths {
            let f = open_for_sequential_read(p)?;
            let len = f.metadata()?.len();
            files.push(f);
            sizes.push(len);
            acc = acc.checked_add(len).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "MultiVolumeReader: total volume bytes overflow u64",
                )
            })?;
            cumulative.push(acc);
        }
        let file_cursors = vec![0u64; files.len()];
        Ok(Self {
            files,
            sizes,
            cumulative,
            total: acc,
            virtual_pos: 0,
            active: 0,
            file_cursors,
        })
    }

    /// Total byte length presented by the virtual stream.
    pub(crate) fn total_size(&self) -> u64 {
        self.total
    }

    /// Virtual offset of the first byte belonging to volume `disk_index`
    /// (0-based, matching ZIP's `disk_number_start` field). Used by the
    /// spanned-ZIP patcher to translate per-disk byte positions into
    /// absolute positions in the virtual stream.
    ///
    /// Out-of-range indices clamp to the end of the stream so callers
    /// dealing with malformed disk references degrade to "no entry
    /// found" rather than panic.
    pub(crate) fn disk_origin(&self, disk_index: usize) -> u64 {
        let idx = disk_index.min(self.cumulative.len() - 1);
        self.cumulative[idx]
    }

    /// Translate a virtual byte position to `(file_index, offset_within_file)`.
    /// Positions equal to `total` map to `(files.len() - 1, sizes[last])` so
    /// callers reading at EOF land cleanly inside the last file's tail.
    fn locate(&self, pos: u64) -> (usize, u64) {
        if pos >= self.total {
            let last = self.files.len() - 1;
            return (last, self.sizes[last]);
        }
        // Binary-search over cumulative offsets. Volume counts are
        // small (rarely > 100) so a tight linear scan would also work,
        // but binary search keeps the asymptotics clean.
        match self.cumulative.binary_search(&pos) {
            Ok(i) if i < self.files.len() => (i, 0),
            Ok(i) => (i - 1, pos - self.cumulative[i - 1]),
            Err(i) => (i - 1, pos - self.cumulative[i - 1]),
        }
    }
}

impl Read for MultiVolumeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.virtual_pos >= self.total {
            return Ok(0);
        }
        let (idx, file_offset) = self.locate(self.virtual_pos);
        self.active = idx;
        let file = &mut self.files[idx];
        // Skip the syscall when the file's kernel cursor is already at
        // `file_offset` — typical case for back-to-back sequential
        // reads inside one volume (the BufReader wrapper above batches
        // these into 64 KiB chunks, so the savings are concentrated at
        // volume boundaries + occasional EOCD probes).
        if self.file_cursors[idx] != file_offset {
            file.seek(SeekFrom::Start(file_offset))?;
            self.file_cursors[idx] = file_offset;
        }

        // Cap the read at "bytes remaining in this volume" so we never
        // try to satisfy a single read() call across two files — the
        // caller will simply issue another read() for the next chunk.
        let remaining_in_volume = self.sizes[idx].saturating_sub(file_offset);
        let cap = remaining_in_volume.min(out.len() as u64) as usize;
        let n = file.read(&mut out[..cap])?;
        self.virtual_pos = self.virtual_pos.saturating_add(n as u64);
        self.file_cursors[idx] = self.file_cursors[idx].saturating_add(n as u64);
        Ok(n)
    }
}

impl Seek for MultiVolumeReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::Current(d) => self.virtual_pos as i128 + d as i128,
            SeekFrom::End(d) => self.total as i128 + d as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "MultiVolumeReader: seek before start",
            ));
        }
        let target = target as u64;
        // Clamp to [0, total] — matching the contract of single-file
        // Seek which permits seeking past EOF but reads after that
        // return 0 bytes. The `zip` crate's central-directory scan
        // seeks near EOF then walks backward looking for the EOCD
        // magic, so end-seek must always succeed.
        self.virtual_pos = target.min(self.total);
        Ok(self.virtual_pos)
    }
}

/// Open a volume with the platform's "sequential streaming" hint set.
/// On Windows this adds `FILE_FLAG_SEQUENTIAL_SCAN`; on every other
/// platform it falls back to a plain `File::open`. The hint is
/// adviso­ry — the OS chooses whether to honour it — but on NTFS it
/// measurably increases prefetcher throughput for large reads, which
/// is the exact workload spanned-ZIP extraction produces.
///
/// Reused by [`super::zip::ZipBackend`] for single-volume archives —
/// the prefetch benefit applies equally to a single big `.zip` and to
/// each `.zNN` chunk in a spanned set.
pub(crate) fn open_for_sequential_read(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        File::open(path)
    }
}

/// Diagnostic helper used by error messages.
#[allow(dead_code)]
pub(crate) fn describe(paths: &[impl AsRef<Path>]) -> String {
    let names: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            p.as_ref()
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
        })
        .collect();
    format!("[{}]", names.join(", "))
}
