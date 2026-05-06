//! Debian package (`ar` container) extract backend.
//!
//! PR-F8 (Phase 7+ option Y). DEB packages are BSD `ar(1)`
//! archives containing exactly three members:
//!
//! 1. `debian-binary` — text version marker (typically "2.0\n").
//! 2. `control.tar.{gz,xz,zst}` — package metadata tarball.
//! 3. `data.tar.{gz,xz,zst}` — actual file payload tarball.
//!
//! v1.0 surfaces those three members verbatim. Auto-flattening the
//! inner tarballs is a v1.1 ergonomic improvement -- users who want
//! the package contents can re-open `data.tar.xz` with SpanZIP.
//!
//! This backend hand-rolls the ar parser (~80 LoC) rather than
//! pulling in an extra crate -- the format is small enough (60-byte
//! fixed-width member headers) that a dedicated dependency would
//! cost more than the parser itself.
//!
//! Spec references
//! ---------------
//! * BSD ar / SVR4 ar header layout: 60 bytes per member, ASCII
//!   numerics in fixed-width fields, content padded to even length.
//! * Debian package format: `man 5 deb` -- requires the three
//!   members in order, but we don't enforce ordering since some
//!   tooling (especially older dpkg) emits them in non-canonical
//!   order. The detect step already verified the `!<arch>\n`
//!   magic; here we just walk members until EOF.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::backends::ArchiveBackend;
use crate::entry::{Entry, HostOs};
use crate::error::{Result, SpanzipError};
use crate::format::{CompressionMethod, EncryptionMethod};

const AR_MAGIC: &[u8; 8] = b"!<arch>\n";
/// ar member header is fixed-width 60 bytes.
const AR_HEADER_LEN: u64 = 60;
/// Member content is followed by a single 0x0A pad byte iff the
/// content length is odd.
const AR_END_MARKER: &[u8; 2] = b"`\n";

#[derive(Clone, Debug)]
struct DebMember {
    /// Member name with the trailing `/` (BSD ar terminator) stripped.
    name: String,
    /// Byte offset into the source file where the content begins
    /// (one byte past the end of the 60-byte header).
    content_offset: u64,
    /// Logical (non-padded) content length.
    content_len: u64,
}

pub(crate) struct DebBackend {
    path: PathBuf,
    members: Vec<DebMember>,
    cache: RefCell<Option<Vec<Entry>>>,
}

impl DebBackend {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let mut f = BufReader::new(File::open(path).map_err(SpanzipError::Io)?);

        // Verify magic. The detect layer already routed us here, but
        // re-verify so a hand-crafted call into `IsoBackend::open`
        // doesn't read past the end of a non-ar file.
        let mut magic = [0u8; 8];
        f.read_exact(&mut magic).map_err(SpanzipError::Io)?;
        if &magic != AR_MAGIC {
            return Err(SpanzipError::UnsupportedFormat(Some(
                "deb: missing `!<arch>\\n` magic".into(),
            )));
        }

        let total_len = f.get_ref().metadata().map_err(SpanzipError::Io)?.len();
        let mut members: Vec<DebMember> = Vec::new();
        let mut pos = 8u64; // first member header starts right after magic

        // Defensive: any bytes past the magic that don't form a full
        // 60-byte header are truncation. dpkg refuses these too, so
        // surfacing a clean error here matches existing tooling
        // semantics (and keeps us from silently extracting nothing).
        if pos < total_len && pos + AR_HEADER_LEN > total_len {
            return Err(SpanzipError::BackendError(
                "deb: truncated ar member header".into(),
            ));
        }

        while pos + AR_HEADER_LEN <= total_len {
            f.seek(SeekFrom::Start(pos)).map_err(SpanzipError::Io)?;
            let mut header = [0u8; 60];
            f.read_exact(&mut header).map_err(SpanzipError::Io)?;

            // Validate the end marker so we fail fast on truncated /
            // misaligned files rather than emitting garbage entries.
            if &header[58..60] != AR_END_MARKER {
                return Err(SpanzipError::BackendError(
                    "deb: ar member header missing 0x60 0x0A end marker".into(),
                ));
            }

            let name = parse_ar_name(&header[0..16]);
            let size_str = std::str::from_utf8(&header[48..58])
                .map_err(|_| SpanzipError::BackendError("deb: non-ASCII size field".into()))?;
            let size: u64 = size_str
                .trim()
                .parse()
                .map_err(|_| SpanzipError::BackendError("deb: malformed size field".into()))?;

            let content_offset = pos + AR_HEADER_LEN;
            members.push(DebMember {
                name,
                content_offset,
                content_len: size,
            });

            // Advance past the content, with the 1-byte pad on odd
            // lengths.
            let mut next = content_offset + size;
            if size % 2 == 1 {
                next += 1;
            }
            // Defensive: if a malformed size pushes us past EOF, stop
            // walking instead of reading garbage.
            if next > total_len {
                break;
            }
            pos = next;
        }

        Ok(Self {
            path: path.to_path_buf(),
            members,
            cache: RefCell::new(None),
        })
    }
}

/// BSD ar member names are 16 bytes, space-padded, terminated by
/// `/` (the trailing slash is part of the format, not part of the
/// name). Long names use the GNU `/N` extension which we don't
/// model -- DEB never uses it (the three canonical names all fit).
fn parse_ar_name(field: &[u8]) -> String {
    let trimmed_end = field
        .iter()
        .rposition(|&b| b != b' ' && b != 0)
        .map_or(0, |i| i + 1);
    let raw = &field[..trimmed_end];
    let no_slash = if raw.last() == Some(&b'/') {
        &raw[..raw.len() - 1]
    } else {
        raw
    };
    String::from_utf8_lossy(no_slash).into_owned()
}

fn member_to_entry(m: &DebMember) -> Entry {
    Entry {
        path: m.name.clone(),
        is_directory: false,
        is_symlink: false,
        uncompressed_size: m.content_len,
        compressed_size: m.content_len,
        // ar members are stored without per-member compression --
        // any compression is inside the embedded tarballs.
        compression: CompressionMethod::Store,
        encryption: EncryptionMethod::None,
        crc32: None,
        modified: None,
        accessed: None,
        created: None,
        attributes: 0,
        comment: None,
        host_os: HostOs::Unix,
    }
}

impl ArchiveBackend for DebBackend {
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>> {
        if self.cache.borrow().is_none() {
            let v: Vec<Entry> = self.members.iter().map(member_to_entry).collect();
            *self.cache.borrow_mut() = Some(v);
        }
        let snapshot = self.cache.borrow().as_ref().unwrap().clone();
        Ok(Box::new(snapshot.into_iter().map(Ok)))
    }

    fn extract_entry(&self, entry_path: &str, out: &mut dyn std::io::Write) -> Result<u64> {
        let m = self
            .members
            .iter()
            .find(|m| m.name == entry_path)
            .ok_or_else(|| SpanzipError::EntryNotFound(entry_path.to_string()))?;

        let mut f = File::open(&self.path).map_err(SpanzipError::Io)?;
        f.seek(SeekFrom::Start(m.content_offset))
            .map_err(SpanzipError::Io)?;
        let mut limited = (&mut f).take(m.content_len);
        let written = io::copy(&mut limited, out).map_err(SpanzipError::Io)?;
        Ok(written)
    }

    fn open_entry_stream(&self, entry_path: &str) -> Result<Box<dyn Read + Send + '_>> {
        let mut buf = Vec::new();
        self.extract_entry(entry_path, &mut buf)?;
        Ok(Box::new(io::Cursor::new(buf)))
    }
}
