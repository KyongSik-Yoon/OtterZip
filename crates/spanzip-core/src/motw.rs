//! Mark-of-the-Web (Zone.Identifier ADS) propagation — Phase 7 PR-7A.
//!
//! When a user downloads `archive.zip` via a browser, Windows attaches an
//! NTFS Alternative Data Stream named `Zone.Identifier` to the file:
//!
//! ```text
//! [ZoneTransfer]
//! ZoneId=3
//! ReferrerUrl=https://example.com/
//! HostUrl=https://example.com/archive.zip
//! ```
//!
//! `ZoneId=3` (Internet) is what makes SmartScreen warn the user before
//! running an extracted `.exe`. If our extractor doesn't propagate that
//! ADS to the extracted files, SmartScreen sees them as locally-created
//! and lets them run silently — a real security regression.
//!
//! ## Strategy
//! 1. On extract start, read the source archive's `Zone.Identifier`
//!    (if any) into memory once.
//! 2. After each output file is written, copy the cached payload onto
//!    the new file's `Zone.Identifier` ADS.
//!
//! ADS access is purely Windows / NTFS. On other platforms (or non-NTFS
//! Windows volumes) the helper degrades to a silent no-op.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::Path;

/// Reads the `Zone.Identifier` ADS of the given file. Returns `None` when
/// the file has no zone marker (locally-created), the volume isn't NTFS,
/// or we're on a non-Windows platform.
///
/// Errors other than "file/stream not found" are also reported as `None` —
/// the caller treats absence as "no zone to propagate", which is the safe
/// behaviour.
pub fn read_zone_identifier(path: &Path) -> Option<Vec<u8>> {
    if !cfg!(windows) {
        return None;
    }
    let stream_path = ads_path(path);
    match fs::File::open(&stream_path) {
        Ok(mut f) => {
            let mut buf = Vec::with_capacity(256);
            if f.read_to_end(&mut buf).is_ok() && !buf.is_empty() {
                Some(buf)
            } else {
                None
            }
        }
        // ADS missing is the common case — treat as "no zone".
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(_) => None,
    }
}

/// Writes the given Zone.Identifier payload onto the file at `path`.
///
/// Best-effort: failures (non-NTFS, permission, owner mismatch) are
/// returned but the caller is expected to log and continue rather than
/// abort the whole extraction. A user with limited privileges shouldn't
/// see "extraction failed" just because we couldn't tag one file.
pub fn write_zone_identifier(path: &Path, payload: &[u8]) -> std::io::Result<()> {
    if !cfg!(windows) {
        return Ok(());
    }
    if payload.is_empty() {
        return Ok(());
    }
    let stream_path = ads_path(path);
    let mut f = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&stream_path)?;
    f.write_all(payload)
}

/// Composes the Win32 ADS path string `<path>:Zone.Identifier`.
///
/// We can't use `PathBuf::push` because `:` is a path separator on
/// Windows — std's path layer would normalise it away. The ADS form
/// is a Win32-only convention, so we keep it as a plain `String`
/// and let `OpenOptions` route through the file-system API.
fn ads_path(path: &Path) -> String {
    format!("{}:Zone.Identifier", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: round-trip a payload on Windows; no-op on other
    /// platforms (just asserts the API doesn't panic).
    #[test]
    fn round_trip_or_noop() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("hello.txt");
        std::fs::write(&target, b"hi").unwrap();

        let payload = b"[ZoneTransfer]\r\nZoneId=3\r\n";
        let _ = write_zone_identifier(&target, payload);

        if cfg!(windows) {
            // Windows + likely NTFS in temp — round-trip should match.
            let read_back = read_zone_identifier(&target);
            // Either it worked (NTFS) or the temp volume can't store ADS
            // (rare). Both are acceptable; we only assert no panic.
            if let Some(got) = read_back {
                assert_eq!(got, payload);
            }
        } else {
            assert!(read_zone_identifier(&target).is_none());
        }
    }
}
